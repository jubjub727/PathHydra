use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::operations::{OperationError, ResourceKind};

#[derive(Clone, Debug)]
pub struct PathTarget<'a> {
    pub root: &'a Path,
    pub path: &'a Path,
    pub must_exist: bool,
    pub must_be_fresh: bool,
}

#[derive(Clone, Debug, Default)]
pub struct OperationalPathRequest<'a> {
    pub database: Option<PathTarget<'a>>,
    pub routing_images: Option<PathTarget<'a>>,
    pub checkpoint: Option<PathTarget<'a>>,
    pub restore: Option<PathTarget<'a>>,
    pub scratch: Option<PathTarget<'a>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedOperationalPaths {
    paths: BTreeMap<ResourceKind, PathBuf>,
}

impl ResolvedOperationalPaths {
    #[must_use]
    pub fn get(&self, kind: ResourceKind) -> Option<&Path> {
        self.paths.get(&kind).map(PathBuf::as_path)
    }
}

pub fn validate_operational_paths(
    request: &OperationalPathRequest<'_>,
) -> Result<ResolvedOperationalPaths, OperationError> {
    let mut paths = BTreeMap::new();
    for (kind, target) in [
        (ResourceKind::Database, request.database.as_ref()),
        (ResourceKind::RoutingImages, request.routing_images.as_ref()),
        (ResourceKind::Checkpoint, request.checkpoint.as_ref()),
        (ResourceKind::Restore, request.restore.as_ref()),
        (ResourceKind::Scratch, request.scratch.as_ref()),
    ] {
        if let Some(target) = target {
            paths.insert(kind, resolve_target(kind, target)?);
        }
    }
    let entries: Vec<_> = paths.iter().collect();
    for (index, (left_kind, left)) in entries.iter().enumerate() {
        for (right_kind, right) in entries.iter().skip(index + 1) {
            if left == right || left.starts_with(right) || right.starts_with(left) {
                return Err(OperationError::UnsafePathRelationship {
                    first: **left_kind,
                    second: **right_kind,
                });
            }
        }
    }
    Ok(ResolvedOperationalPaths { paths })
}

fn resolve_target(kind: ResourceKind, target: &PathTarget<'_>) -> Result<PathBuf, OperationError> {
    let root = canonical_existing(target.root).map_err(|reason| OperationError::InvalidPath {
        resource: kind,
        reason,
    })?;
    if root.parent().is_none() {
        return Err(OperationError::InvalidPath {
            resource: kind,
            reason: "caller-selected root must not be a filesystem root".to_owned(),
        });
    }
    let path = absolute_lexical(target.path).map_err(|reason| OperationError::InvalidPath {
        resource: kind,
        reason,
    })?;
    let resolved =
        resolve_existing_ancestors(&path).map_err(|reason| OperationError::InvalidPath {
            resource: kind,
            reason,
        })?;
    if resolved == root || !resolved.starts_with(&root) {
        return Err(OperationError::InvalidPath {
            resource: kind,
            reason: "target must be a strict descendant of its caller-selected root".to_owned(),
        });
    }
    if target.must_exist && !resolved.is_dir() {
        return Err(OperationError::InvalidPath {
            resource: kind,
            reason: "required directory does not exist".to_owned(),
        });
    }
    if target.must_be_fresh && resolved.exists() {
        if !resolved.is_dir() {
            return Err(OperationError::InvalidPath {
                resource: kind,
                reason: "fresh destination exists but is not a directory".to_owned(),
            });
        }
        let mut entries = fs::read_dir(&resolved).map_err(|error| OperationError::Io {
            operation: "read destination directory",
            reason: error.to_string(),
        })?;
        if entries
            .next()
            .transpose()
            .map_err(|error| OperationError::Io {
                operation: "read destination directory",
                reason: error.to_string(),
            })?
            .is_some()
        {
            return Err(OperationError::DestinationNotEmpty { resource: kind });
        }
    }
    Ok(resolved)
}

fn canonical_existing(path: &Path) -> Result<PathBuf, String> {
    let absolute = absolute_lexical(path)?;
    if !absolute.is_dir() {
        return Err("caller-selected root does not exist or is not a directory".to_owned());
    }
    fs::canonicalize(absolute)
        .map(normalize_canonical)
        .map_err(|error| error.to_string())
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, String> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut output = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return Err("path escapes its filesystem prefix".to_owned());
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    Ok(output)
}

fn resolve_existing_ancestors(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map(normalize_canonical)
            .map_err(|error| error.to_string());
    }
    let mut missing = Vec::new();
    let mut ancestor = path;
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| "target has no existing ancestor".to_owned())?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "target has no existing ancestor".to_owned())?;
    }
    let mut resolved = fs::canonicalize(ancestor)
        .map(normalize_canonical)
        .map_err(|error| error.to_string())?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

pub(crate) fn normalize_canonical(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next()
            && let Prefix::VerbatimDisk(drive) = prefix.kind()
        {
            let mut normalized = PathBuf::from(format!("{}:", char::from(drive)));
            for component in components {
                normalized.push(component.as_os_str());
            }
            return normalized;
        }
    }
    path
}
