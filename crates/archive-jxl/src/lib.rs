//! Optional standard JPEG XL archival. Only complete byte-identical JPEG
//! reconstructions qualify. The ordinary optimizer never changes the format.
#![forbid(unsafe_code)]

pub const AVAILABLE: bool = cfg!(feature = "archive");

#[cfg(not(feature = "archive"))]
pub fn archive_jpeg(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("The optional JPEG XL archive backend is not enabled".into())
}
#[cfg(not(feature = "archive"))]
pub fn restore_jpeg(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("The optional JPEG XL archive backend is not enabled".into())
}

#[cfg(feature = "archive")]
mod codec;
#[cfg(feature = "archive")]
mod limits;
#[cfg(feature = "archive")]
pub use codec::{archive_jpeg, restore_jpeg};

#[cfg(all(test, feature = "archive"))]
mod tests;

#[cfg(all(test, not(feature = "archive")))]
#[test]
fn disabled_backend_returns_an_explicit_error() {
    assert!(archive_jpeg(b"").is_err());
    assert!(restore_jpeg(b"").is_err());
}
