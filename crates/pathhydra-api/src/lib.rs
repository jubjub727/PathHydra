//! Owned, bounded consumer boundary for PathHydra.

mod codec;
mod convert;
mod dto;
mod error;
mod facade;
mod limits;

pub use codec::{
    ApiCodecError, CanonicalDocument, CanonicalDto, CodecLimit, MalformedKind, decode,
    decode_and_reencode, decode_strict_canonical, encode,
};
pub use dto::*;
pub use error::{ApiError, ApiErrorCategory};
pub use facade::{PathHydra, PathHydraOpenConfig, RequestHandle, RequestIdAllocation};
pub use limits::ApiLimits;
