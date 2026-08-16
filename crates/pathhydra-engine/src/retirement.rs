use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, Weak},
};

use pathhydra_routing::RoutingBundle;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetirementSnapshot {
    pub bundle_count: usize,
    pub bundle_bytes: u64,
    pub maximum_bundle_count: usize,
    pub maximum_bundle_bytes: u64,
    pub limit_exceeded: bool,
    pub last_cleanup_failure: Option<String>,
}

struct RetiredBundle {
    path: PathBuf,
    bytes: u64,
    lease: Weak<RoutingBundle>,
}

struct RetirementState {
    bundles: Vec<RetiredBundle>,
    last_cleanup_failure: Option<String>,
    #[cfg(test)]
    fail_next_delete: bool,
}

pub(crate) struct RetirementManager {
    root: PathBuf,
    maximum_count: usize,
    maximum_bytes: u64,
    state: Mutex<RetirementState>,
}

impl RetirementManager {
    pub fn new(root: PathBuf, maximum_count: usize, maximum_bytes: u64) -> Self {
        Self {
            root,
            maximum_count,
            maximum_bytes,
            state: Mutex::new(RetirementState {
                bundles: Vec::new(),
                last_cleanup_failure: None,
                #[cfg(test)]
                fail_next_delete: false,
            }),
        }
    }

    pub fn retire(&self, bundle: &std::sync::Arc<RoutingBundle>) {
        let path = bundle.root().to_path_buf();
        if !is_exact_child(&self.root, &path) {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            if state.bundles.iter().any(|entry| entry.path == path) {
                return;
            }
            state.bundles.push(RetiredBundle {
                path,
                bytes: bundle.total_bytes(),
                lease: std::sync::Arc::downgrade(bundle),
            });
        }
    }

    pub fn reap(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let bundles = std::mem::take(&mut state.bundles);
        let mut retained = Vec::with_capacity(bundles.len());
        for entry in bundles {
            if entry.lease.strong_count() != 0 {
                retained.push(entry);
                continue;
            }
            #[cfg(test)]
            let injected = std::mem::take(&mut state.fail_next_delete);
            #[cfg(not(test))]
            let injected = false;
            let deleted = if injected {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected Windows-style open-handle retirement failure",
                ))
            } else {
                fs::remove_dir_all(&entry.path)
            };
            match deleted {
                Ok(()) => state.last_cleanup_failure = None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    state.last_cleanup_failure = None;
                }
                Err(error) => {
                    state.last_cleanup_failure = Some(error.to_string());
                    retained.push(entry);
                }
            }
        }
        state.bundles = retained;
    }

    pub fn snapshot(&self) -> RetirementSnapshot {
        let Ok(state) = self.state.lock() else {
            return RetirementSnapshot {
                maximum_bundle_count: self.maximum_count,
                maximum_bundle_bytes: self.maximum_bytes,
                limit_exceeded: true,
                last_cleanup_failure: Some("retirement state lock is poisoned".to_owned()),
                ..RetirementSnapshot::default()
            };
        };
        let bytes = state
            .bundles
            .iter()
            .fold(0_u64, |total, entry| total.saturating_add(entry.bytes));
        RetirementSnapshot {
            bundle_count: state.bundles.len(),
            bundle_bytes: bytes,
            maximum_bundle_count: self.maximum_count,
            maximum_bundle_bytes: self.maximum_bytes,
            limit_exceeded: state.bundles.len() > self.maximum_count || bytes > self.maximum_bytes,
            last_cleanup_failure: state.last_cleanup_failure.clone(),
        }
    }

    #[cfg(test)]
    pub fn fail_next_delete(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.fail_next_delete = true;
        }
    }
}

fn is_exact_child(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root) && path.file_name().is_some()
}
