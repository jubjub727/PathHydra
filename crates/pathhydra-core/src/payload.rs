/// Opaque caller-owned bytes stored with a node.
///
/// PathHydra does not interpret the bytes. Empty and non-UTF-8 payloads are
/// valid.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodePayload(Box<[u8]>);

/// Maximum node payload accepted by the durable store (16 MiB).
///
/// This bounds allocation and RocksDB record size while leaving payload
/// contents entirely caller-defined.
pub const MAX_NODE_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

impl NodePayload {
    #[must_use]
    pub fn new(value: impl Into<Box<[u8]>>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_boxed_bytes(self) -> Box<[u8]> {
        self.0
    }
}

impl AsRef<[u8]> for NodePayload {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl From<Vec<u8>> for NodePayload {
    fn from(value: Vec<u8>) -> Self {
        Self::new(value.into_boxed_slice())
    }
}

impl From<Box<[u8]>> for NodePayload {
    fn from(value: Box<[u8]>) -> Self {
        Self::new(value)
    }
}

impl From<&[u8]> for NodePayload {
    fn from(value: &[u8]) -> Self {
        Self::new(value.to_vec().into_boxed_slice())
    }
}

impl<const N: usize> From<[u8; N]> for NodePayload {
    fn from(value: [u8; N]) -> Self {
        Self::from(Vec::from(value))
    }
}
