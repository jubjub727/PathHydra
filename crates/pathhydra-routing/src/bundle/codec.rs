use super::BundleError;

pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), BundleError> {
    put_u32(
        out,
        u32::try_from(value.len())
            .map_err(|_| BundleError::Invalid("policy identifier is too long".into()))?,
    );
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Decoder<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], BundleError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| BundleError::Invalid("byte offset overflow".into()))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| BundleError::Invalid("truncated field".into()))?;
        self.offset = end;
        Ok(value)
    }
    pub(crate) fn u32(&mut self) -> Result<u32, BundleError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("checked width"),
        ))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, BundleError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("checked width"),
        ))
    }
    pub(crate) fn checksum(&mut self) -> Result<[u8; 32], BundleError> {
        Ok(self.take(32)?.try_into().expect("checked width"))
    }
    pub(crate) fn string(&mut self) -> Result<Box<str>, BundleError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| BundleError::Invalid("string length does not fit platform".into()))?;
        let bytes = self.take(length)?;
        Ok(std::str::from_utf8(bytes)
            .map_err(|_| BundleError::Invalid("policy identifier is not UTF-8".into()))?
            .into())
    }
    pub(crate) fn finish(self) -> Result<(), BundleError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(BundleError::Invalid("manifest has trailing bytes".into()))
        }
    }
}

pub(crate) fn checksum(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}
