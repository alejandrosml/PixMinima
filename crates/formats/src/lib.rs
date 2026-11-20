//! In-format recompression. Each engine validates the representation it changes;
//! complex containers without a verified transformation pass through unchanged.
#![forbid(unsafe_code)]

mod bmp;
mod gif;
mod jpeg;
mod quality;
mod tiff;
mod webp;

const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_PIXELS: usize = 32 * 1024 * 1024;
const MAX_ANIMATION_PIXELS: usize = 192 * 1024 * 1024;

pub fn detect(input: &[u8]) -> Option<&'static str> {
    if input.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("JPEG")
    } else if input.starts_with(b"GIF87a") || input.starts_with(b"GIF89a") {
        Some("GIF")
    } else if input.starts_with(b"RIFF") && input.get(8..12) == Some(b"WEBP") {
        Some("WebP")
    } else if input.starts_with(b"II\x2a\0") || input.starts_with(b"MM\0\x2a") {
        Some("TIFF")
    } else if input.starts_with(b"BM") {
        Some("BMP")
    } else {
        None
    }
}

/// Returns the original bytes if the optimizer cannot produce a strictly smaller
/// verified representation. Errors leave handling of the original to the caller.
pub fn optimize(input: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    if input.len() > MAX_BYTES {
        return Err("File exceeds the 256 MiB engine limit".into());
    }
    let candidate = match detect(input) {
        Some("JPEG") => jpeg::optimize(input, thorough)?,
        Some("GIF") => gif::optimize(input, thorough)?,
        Some("WebP") => webp::optimize(input, thorough)?,
        Some("BMP") => bmp::optimize(input, thorough)?,
        Some("TIFF") => tiff::optimize(input, thorough)?,
        _ => return Err("Unrecognized image format".into()),
    };
    if candidate.len() < input.len() {
        Ok(candidate)
    } else {
        Ok(input.to_vec())
    }
}

/// Search additional coefficient-preserving JPEG encodings. Every format keeps
/// the verified thorough result if a smaller verified candidate is unavailable.
pub fn optimize_maximum(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() > MAX_BYTES {
        return Err("File exceeds the 256 MiB engine limit".into());
    }
    if detect(input) == Some("JPEG") {
        jpeg::optimize_maximum(input)
    } else {
        optimize(input, true)
    }
}

/// Opt-in bounded-error candidates. The lossless entry point never calls this path.
pub fn optimize_lossy(input: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    if input.len() > MAX_BYTES {
        return Err("File exceeds the 256 MiB engine limit".into());
    }
    match detect(input) {
        Some("JPEG") => jpeg::optimize_lossy(input, thorough),
        Some("WebP") => webp::optimize_lossy(input, thorough),
        Some("GIF") => gif::optimize_lossy(input, thorough),
        _ => Ok(None),
    }
}

fn take<'a>(src: &'a [u8], p: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = p.checked_add(len).ok_or("Invalid image offset")?;
    let s = src.get(*p..end).ok_or("Truncated image data")?;
    *p = end;
    Ok(s)
}

// A decoder can prefetch whole bytes beyond its end code. Hold back the final
// byte until needed so even a one-byte opaque suffix prevents rewriting.
fn decode_lzw(
    mut decoder: weezl::decode::Decoder,
    input: &[u8],
    expected: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut output = vec![0; expected + 1];
    let (mut consumed, mut written) = (0, 0);
    let prefix = input.len().saturating_sub(1);
    let mut end = prefix;
    loop {
        let step = decoder.decode_bytes(&input[consumed..end], &mut output[written..]);
        consumed += step.consumed_in;
        written += step.consumed_out;
        if written > expected {
            return Err("LZW output exceeds the image bounds".into());
        }
        match step.status {
            Ok(weezl::LzwStatus::Done) if written == expected => {
                if consumed != input.len() {
                    return Ok(None);
                }
                output.truncate(written);
                return Ok(Some(output));
            }
            Ok(weezl::LzwStatus::Ok) if step.consumed_in != 0 || step.consumed_out != 0 => {}
            Ok(weezl::LzwStatus::NoProgress) if end < input.len() && consumed == end => {
                end = input.len();
            }
            // Some TIFF writers stop after the final sample without emitting
            // an end code. Readers display the image, but keep its exact stream.
            Ok(weezl::LzwStatus::NoProgress) if consumed == input.len() && written == expected => {
                return Ok(None);
            }
            _ => return Err("Invalid LZW stream or sample count".into()),
        }
    }
}

fn le16(bytes: &[u8]) -> usize {
    u16::from_le_bytes([bytes[0], bytes[1]]) as usize
}
fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_headers_and_deterministic_garbage_do_not_panic() {
        let seeds: &[&[u8]] = &[
            b"GIF89a",
            b"RIFF\0\0\0\0WEBP",
            b"II\x2a\0",
            b"MM\0\x2a",
            b"BM",
            b"\xff\xd8\xff",
        ];
        let mut state = 8u32;
        for prefix in seeds {
            for len in 0..258 {
                let mut data = prefix.to_vec();
                for _ in 0..len {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    data.push((state >> 24) as u8);
                }
                if let Ok(candidate) = optimize(&data, true) {
                    assert!(candidate.len() <= data.len());
                }
            }
        }
    }
}
