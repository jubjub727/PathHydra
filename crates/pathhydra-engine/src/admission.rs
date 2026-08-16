use std::{error::Error, fmt, sync::Mutex};

use crate::EngineError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionRejection {
    TooManyDestinations {
        requested: usize,
        maximum: usize,
    },
    RequestBytes {
        requested: usize,
        maximum: usize,
    },
    ConcurrentRoutes {
        maximum: usize,
    },
    TotalBytes {
        requested_total: usize,
        maximum: usize,
    },
    ArithmeticOverflow,
}

impl fmt::Display for AdmissionRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyDestinations { requested, maximum } => write!(
                formatter,
                "route has {requested} destinations; maximum is {maximum}"
            ),
            Self::RequestBytes { requested, maximum } => write!(
                formatter,
                "route reserves {requested} bytes; total limit is {maximum}"
            ),
            Self::ConcurrentRoutes { maximum } => {
                write!(formatter, "maximum {maximum} concurrent routes are active")
            }
            Self::TotalBytes {
                requested_total,
                maximum,
            } => write!(
                formatter,
                "route would reserve {requested_total} bytes; total limit is {maximum}"
            ),
            Self::ArithmeticOverflow => formatter.write_str("route admission arithmetic overflow"),
        }
    }
}
impl Error for AdmissionRejection {}

#[derive(Default)]
struct State {
    active: usize,
    peak_active: usize,
    reserved: usize,
    peak_reserved: usize,
    admissions: u64,
    rejections: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AdmissionSnapshot {
    pub active: usize,
    pub peak_active: usize,
    pub reserved: usize,
    pub peak_reserved: usize,
    pub admissions: u64,
    pub rejections: u64,
}

pub(crate) struct AdmissionController {
    maximum_active: usize,
    maximum_bytes: usize,
    state: Mutex<State>,
}

impl AdmissionController {
    pub fn new(maximum_active: usize, maximum_bytes: usize) -> Self {
        Self {
            maximum_active,
            maximum_bytes,
            state: Mutex::new(State::default()),
        }
    }

    pub fn acquire_slot(&self) -> Result<AdmissionPermit<'_>, EngineError> {
        let mut state = self.state.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "route admission",
        })?;
        if state.active >= self.maximum_active {
            state.rejections = state.rejections.saturating_add(1);
            return Err(EngineError::Admission(
                AdmissionRejection::ConcurrentRoutes {
                    maximum: self.maximum_active,
                },
            ));
        }
        state.active += 1;
        state.peak_active = state.peak_active.max(state.active);
        Ok(AdmissionPermit {
            controller: self,
            bytes: 0,
            admitted: false,
        })
    }

    pub fn reject_destinations(
        &self,
        requested: usize,
        maximum: usize,
    ) -> Result<EngineError, EngineError> {
        let mut state = self.state.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "route admission",
        })?;
        state.rejections = state.rejections.saturating_add(1);
        Ok(EngineError::Admission(
            AdmissionRejection::TooManyDestinations { requested, maximum },
        ))
    }

    fn reserve(&self, bytes: usize) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "route admission",
        })?;
        if bytes > self.maximum_bytes {
            state.rejections = state.rejections.saturating_add(1);
            return Err(EngineError::Admission(AdmissionRejection::RequestBytes {
                requested: bytes,
                maximum: self.maximum_bytes,
            }));
        }
        let total = state.reserved.checked_add(bytes).ok_or_else(|| {
            state.rejections = state.rejections.saturating_add(1);
            EngineError::Admission(AdmissionRejection::ArithmeticOverflow)
        })?;
        if total > self.maximum_bytes {
            state.rejections = state.rejections.saturating_add(1);
            return Err(EngineError::Admission(AdmissionRejection::TotalBytes {
                requested_total: total,
                maximum: self.maximum_bytes,
            }));
        }
        state.reserved = total;
        state.peak_reserved = state.peak_reserved.max(total);
        state.admissions = state.admissions.saturating_add(1);
        Ok(())
    }

    pub fn snapshot(&self) -> Result<AdmissionSnapshot, EngineError> {
        let state = self.state.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "route admission",
        })?;
        Ok(AdmissionSnapshot {
            active: state.active,
            peak_active: state.peak_active,
            reserved: state.reserved,
            peak_reserved: state.peak_reserved,
            admissions: state.admissions,
            rejections: state.rejections,
        })
    }
}

pub(crate) struct AdmissionPermit<'a> {
    controller: &'a AdmissionController,
    bytes: usize,
    admitted: bool,
}

impl AdmissionPermit<'_> {
    pub fn reserve(&mut self, bytes: usize) -> Result<(), EngineError> {
        self.controller.reserve(bytes)?;
        self.bytes = bytes;
        self.admitted = true;
        Ok(())
    }
}

impl Drop for AdmissionPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.controller.state.lock() {
            state.active = state.active.saturating_sub(1);
            if self.admitted {
                state.reserved = state.reserved.saturating_sub(self.bytes);
            }
        }
    }
}
