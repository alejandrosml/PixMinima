#![forbid(unsafe_code)]

mod exr;
mod netpbm;
mod packets;
mod psd;
mod qoi;
mod tga;
mod zlib;

const MAX_DECODED: usize = 128 * 1024 * 1024;
const MAX_INPUT: usize = 256 * 1024 * 1024;

/// Identify a supported image from its bytes. Untagged TGA requires a structurally
/// complete image, as it has no leading magic number.
pub fn detect(input: &[u8]) -> Option<&'static str> {
    if input.starts_with(&exr::MAGIC) {
        return Some("EXR");
    }
    if let Some(kind) = psd::detect(input) {
        return Some(kind);
    }
    if input.starts_with(b"qoif") {
        return Some("QOI");
    }
    if let Some(kind) = netpbm::detect(input) {
        return Some(kind);
    }
    if tga::recognized(input) {
        return Some("TGA");
    }
    None
}

/// Produce a smaller verified representation, retaining the input in case of a tie.
pub fn optimize(input: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    if input.len() > MAX_INPUT {
        return Err("Image exceeds the 256 MiB input limit".into());
    }
    let candidate = match detect(input) {
        Some("EXR") => exr::optimize(input, thorough)?,
        Some("PSD" | "PSB") => psd::optimize(input, thorough)?,
        Some("QOI") => qoi::optimize(input)?,
        Some("TGA") => tga::optimize(input)?,
        Some("PBM" | "PGM" | "PPM") => netpbm::optimize(input)?,
        _ => return Err("Unsupported image format".into()),
    };
    Ok(candidate
        .filter(|b| b.len() < input.len())
        .unwrap_or_else(|| input.to_vec()))
}

#[cfg(test)]
mod tests;
