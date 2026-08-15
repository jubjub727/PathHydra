//! Durable storage boundary for PathHydra.
//!
//! This crate owns durable encodings, column families, atomic updates, cache
//! rebuilding, and RocksDB-specific errors. Storage behaviour is introduced in
//! a later implementation plan.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_loads() {}
}
