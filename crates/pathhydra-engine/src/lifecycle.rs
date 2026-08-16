use std::{
    fmt,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineLifecycleState {
    Running,
    Closing,
    ShutDown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActiveOperationCounts {
    pub routes: usize,
    pub mutations: usize,
    pub checkpoints: usize,
    pub maintenance: usize,
}

impl ActiveOperationCounts {
    fn total(self) -> Option<usize> {
        self.routes
            .checked_add(self.mutations)?
            .checked_add(self.checkpoints)?
            .checked_add(self.maintenance)
    }

    fn increment(&mut self, kind: OperationKind) -> bool {
        let slot = self.slot_mut(kind);
        let Some(next) = slot.checked_add(1) else {
            return false;
        };
        *slot = next;
        true
    }

    fn decrement(&mut self, kind: OperationKind) {
        let slot = self.slot_mut(kind);
        *slot = slot.saturating_sub(1);
    }

    fn slot_mut(&mut self, kind: OperationKind) -> &mut usize {
        match kind {
            OperationKind::Route => &mut self.routes,
            OperationKind::Mutation => &mut self.mutations,
            OperationKind::Checkpoint => &mut self.checkpoints,
            OperationKind::Maintenance => &mut self.maintenance,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleSnapshot {
    pub state: EngineLifecycleState,
    pub active: ActiveOperationCounts,
    pub shutdown_active_before: Option<ActiveOperationCounts>,
    pub rejected_after_close: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrainOutcome {
    pub drained: bool,
    pub remaining: ActiveOperationCounts,
    pub duration: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleError {
    Closing,
    ShutDown,
    CounterOverflow,
    MaintenanceBusy { maximum_workers: usize },
    LockPoisoned,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Closing => "the graph engine is closing",
            Self::ShutDown => "the graph engine is shut down",
            Self::CounterOverflow => "the active-operation counter overflowed",
            Self::MaintenanceBusy { .. } => "the bounded maintenance worker is busy",
            Self::LockPoisoned => "the engine lifecycle lock is poisoned",
        })?;
        if let Self::MaintenanceBusy { maximum_workers } = self {
            write!(formatter, " (maximum workers: {maximum_workers})")?;
        }
        Ok(())
    }
}

impl std::error::Error for LifecycleError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind {
    Route,
    Mutation,
    Checkpoint,
    Maintenance,
}

#[derive(Debug)]
struct State {
    lifecycle: EngineLifecycleState,
    active: ActiveOperationCounts,
    shutdown_active_before: Option<ActiveOperationCounts>,
    rejected_after_close: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            lifecycle: EngineLifecycleState::Running,
            active: ActiveOperationCounts::default(),
            shutdown_active_before: None,
            rejected_after_close: 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct LifecycleController {
    state: Mutex<State>,
    drained: Condvar,
    maximum_maintenance_workers: usize,
}

impl LifecycleController {
    pub fn new(maximum_maintenance_workers: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            drained: Condvar::new(),
            maximum_maintenance_workers,
        }
    }
    pub fn begin_route(&self) -> Result<LifecyclePermit<'_>, LifecycleError> {
        self.begin(OperationKind::Route)
    }

    pub fn begin_mutation(&self) -> Result<LifecyclePermit<'_>, LifecycleError> {
        self.begin(OperationKind::Mutation)
    }

    pub fn begin_checkpoint(&self) -> Result<LifecyclePermit<'_>, LifecycleError> {
        self.begin(OperationKind::Checkpoint)
    }

    pub fn begin_maintenance(&self) -> Result<LifecyclePermit<'_>, LifecycleError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        match state.lifecycle {
            EngineLifecycleState::Running => {}
            EngineLifecycleState::Closing => {
                state.rejected_after_close = state.rejected_after_close.saturating_add(1);
                return Err(LifecycleError::Closing);
            }
            EngineLifecycleState::ShutDown => {
                state.rejected_after_close = state.rejected_after_close.saturating_add(1);
                return Err(LifecycleError::ShutDown);
            }
        }
        if state.active.maintenance >= self.maximum_maintenance_workers {
            return Err(LifecycleError::MaintenanceBusy {
                maximum_workers: self.maximum_maintenance_workers,
            });
        }
        if !state.active.increment(OperationKind::Maintenance) {
            return Err(LifecycleError::CounterOverflow);
        }
        Ok(LifecyclePermit {
            controller: self,
            kind: OperationKind::Maintenance,
        })
    }

    fn begin(&self, kind: OperationKind) -> Result<LifecyclePermit<'_>, LifecycleError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        match state.lifecycle {
            EngineLifecycleState::Running => {}
            EngineLifecycleState::Closing => {
                state.rejected_after_close = state.rejected_after_close.saturating_add(1);
                return Err(LifecycleError::Closing);
            }
            EngineLifecycleState::ShutDown => {
                state.rejected_after_close = state.rejected_after_close.saturating_add(1);
                return Err(LifecycleError::ShutDown);
            }
        }
        if !state.active.increment(kind) {
            return Err(LifecycleError::CounterOverflow);
        }
        Ok(LifecyclePermit {
            controller: self,
            kind,
        })
    }

    /// Stops new admitted work. Repeated calls preserve the original closing
    /// boundary and return the current state.
    pub fn close_admission(&self) -> Result<EngineLifecycleState, LifecycleError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        if state.lifecycle == EngineLifecycleState::Running {
            state.shutdown_active_before = Some(state.active);
            state.lifecycle = EngineLifecycleState::Closing;
        }
        Ok(state.lifecycle)
    }

    pub fn wait_for_drain(&self, maximum: Duration) -> Result<DrainOutcome, LifecycleError> {
        let started = Instant::now();
        let mut state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        loop {
            let total = state
                .active
                .total()
                .ok_or(LifecycleError::CounterOverflow)?;
            if total == 0 {
                return Ok(DrainOutcome {
                    drained: true,
                    remaining: state.active,
                    duration: started.elapsed(),
                });
            }
            let remaining = maximum.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(DrainOutcome {
                    drained: false,
                    remaining: state.active,
                    duration: started.elapsed(),
                });
            }
            let (next, timeout) = self
                .drained
                .wait_timeout(state, remaining)
                .map_err(|_| LifecycleError::LockPoisoned)?;
            state = next;
            if timeout.timed_out() {
                return Ok(DrainOutcome {
                    drained: state
                        .active
                        .total()
                        .ok_or(LifecycleError::CounterOverflow)?
                        == 0,
                    remaining: state.active,
                    duration: started.elapsed(),
                });
            }
        }
    }

    pub fn mark_shut_down(&self) -> Result<(), LifecycleError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        if state
            .active
            .total()
            .ok_or(LifecycleError::CounterOverflow)?
            != 0
        {
            return Err(LifecycleError::Closing);
        }
        state.lifecycle = EngineLifecycleState::ShutDown;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<LifecycleSnapshot, LifecycleError> {
        let state = self
            .state
            .lock()
            .map_err(|_| LifecycleError::LockPoisoned)?;
        Ok(LifecycleSnapshot {
            state: state.lifecycle,
            active: state.active,
            shutdown_active_before: state.shutdown_active_before,
            rejected_after_close: state.rejected_after_close,
        })
    }
}

#[derive(Debug)]
pub(crate) struct LifecyclePermit<'a> {
    controller: &'a LifecycleController,
    kind: OperationKind,
}

impl Drop for LifecyclePermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.controller.state.lock() {
            state.active.decrement(self.kind);
            self.controller.drained.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_is_idempotent_and_drain_tracks_each_operation_class() {
        let controller = LifecycleController::new(1);
        let route = controller.begin_route().unwrap();
        let mutation = controller.begin_mutation().unwrap();
        let checkpoint = controller.begin_checkpoint().unwrap();
        let maintenance = controller.begin_maintenance().unwrap();
        assert_eq!(
            controller.close_admission().unwrap(),
            EngineLifecycleState::Closing
        );
        assert_eq!(
            controller.close_admission().unwrap(),
            EngineLifecycleState::Closing
        );
        assert_eq!(
            controller.begin_route().unwrap_err(),
            LifecycleError::Closing
        );
        assert!(!controller.wait_for_drain(Duration::ZERO).unwrap().drained);
        drop((route, mutation, checkpoint, maintenance));
        assert!(
            controller
                .wait_for_drain(Duration::from_secs(1))
                .unwrap()
                .drained
        );
        controller.mark_shut_down().unwrap();
        assert_eq!(
            controller.begin_maintenance().unwrap_err(),
            LifecycleError::ShutDown
        );
        assert_eq!(controller.snapshot().unwrap().rejected_after_close, 2);
    }

    #[test]
    fn maintenance_has_one_caller_executed_worker_and_no_queue() {
        let controller = LifecycleController::new(1);
        let active = controller.begin_maintenance().unwrap();
        assert_eq!(
            controller.begin_maintenance().unwrap_err(),
            LifecycleError::MaintenanceBusy { maximum_workers: 1 }
        );
        drop(active);
        assert!(controller.begin_maintenance().is_ok());
    }
}
