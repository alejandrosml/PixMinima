//! JPEG coefficient transcoding. Adapted from smallpeg 0.15.0.
#![forbid(unsafe_code)]
mod component;
mod huffman;
mod meta;
mod optimize;
mod progressive;
mod stuffing;
mod symbols;
pub use optimize::{
    JpegInfo, Limits, jpeg_info, optimize_jpeg, optimize_jpeg_baseline,
    optimize_jpeg_baseline_with, optimize_jpeg_maximum_with, optimize_jpeg_with,
};
pub use optimize::{requantize_jpeg_with, same_coefficients};

fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}
