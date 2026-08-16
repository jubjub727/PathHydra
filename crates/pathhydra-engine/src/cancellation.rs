use std::{
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};

use crate::EngineError;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(u64);

impl RequestId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationOutcome {
    Signalled,
    AlreadySignalled,
    NotActive,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CancelAllOutcome {
    pub active: usize,
    pub newly_signalled: usize,
}

pub(crate) struct RequestRegistry {
    active: Mutex<HashMap<RequestId, Arc<AtomicBool>>>,
}
impl RequestRegistry {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
        }
    }
    pub fn register_flag(
        &self,
        id: RequestId,
        flag: Arc<AtomicBool>,
    ) -> Result<RequestRegistration<'_>, EngineError> {
        let mut active = self.active.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "active request registry",
        })?;
        if active.contains_key(&id) {
            return Err(EngineError::DuplicateActiveRequest(id));
        }
        active.insert(id, Arc::clone(&flag));
        Ok(RequestRegistration {
            registry: self,
            id,
            flag,
        })
    }
    pub fn cancel(&self, id: RequestId) -> Result<CancellationOutcome, EngineError> {
        let active = self.active.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "active request registry",
        })?;
        Ok(match active.get(&id) {
            None => CancellationOutcome::NotActive,
            Some(flag) if flag.swap(true, Ordering::AcqRel) => {
                CancellationOutcome::AlreadySignalled
            }
            Some(_) => CancellationOutcome::Signalled,
        })
    }

    pub fn cancel_all(&self) -> Result<CancelAllOutcome, EngineError> {
        let active = self.active.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "active request registry",
        })?;
        let newly_signalled = active
            .values()
            .filter(|flag| !flag.swap(true, Ordering::AcqRel))
            .count();
        Ok(CancelAllOutcome {
            active: active.len(),
            newly_signalled,
        })
    }
}
pub(crate) struct RequestRegistration<'a> {
    registry: &'a RequestRegistry,
    id: RequestId,
    flag: Arc<AtomicBool>,
}
impl RequestRegistration<'_> {
    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }
    #[cfg(feature = "cuda")]
    pub fn flag_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }
}
impl Drop for RequestRegistration<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.active.lock() {
            active.remove(&self.id);
        }
    }
}
