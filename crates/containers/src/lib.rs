#![forbid(unsafe_code)]

mod archives;
mod bmff;
mod heif;
mod icns;
mod icons;
mod jxl;
mod vector;

use miniz_oxide::inflate::{
    TINFLStatus,
    core::{DecompressorOxide, decompress, inflate_flags},
};

pub fn optimize(input: &[u8], thorough: bool) -> Result<(&'static str, Vec<u8>), String> {
    if input.starts_with(b"icns") {
        return Ok(("ICNS", icns::optimize(input, thorough)?));
    }
    if jxl::detect(input) {
        return Ok(("JXL", jxl::optimize(input, thorough)?));
    }
    if let Some(kind) = heif::detect(input) {
        return Ok((kind, heif::optimize(input)));
    }
    if input.starts_with(&[0, 0, 1, 0]) || input.starts_with(&[0, 0, 2, 0]) {
        return icons::optimize(input, thorough);
    }
    if input.starts_with(b"PK\x03\x04") {
        return archives::optimize(input, thorough);
    }
    if input.starts_with(&[0x1f, 0x8b, 8]) {
        return vector::gzip_svg(input, thorough);
    }
    let xml = input.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(input);
    if xml.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<') {
        return Ok(("SVG", vector::svg(input)?));
    }
    Err("Unrecognized or unsupported image format".into())
}

fn u16le(input: &[u8], at: usize) -> Result<usize, String> {
    let a: [u8; 2] = input
        .get(at..at + 2)
        .ok_or("Truncated 16-bit field")?
        .try_into()
        .unwrap();
    Ok(u16::from_le_bytes(a) as usize)
}

fn u32le(input: &[u8], at: usize) -> Result<usize, String> {
    let a: [u8; 4] = input
        .get(at..at + 4)
        .ok_or("Truncated 32-bit field")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(a) as usize)
}

fn set32(buf: &mut [u8], at: usize, val: usize) -> Result<(), String> {
    let val = u32::try_from(val).map_err(|_| "Value outside format limits")?;
    buf.get_mut(at..at + 4)
        .ok_or("Truncated offset")?
        .copy_from_slice(&val.to_le_bytes());
    Ok(())
}

const fn crc_table() -> [u32; 256] {
    let mut tab = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut n = i as u32;
        let mut j = 0;
        while j < 8 {
            n = if n & 1 == 1 {
                0xedb88320 ^ (n >> 1)
            } else {
                n >> 1
            };
            j += 1;
        }
        tab[i] = n;
        i += 1;
    }
    tab
}
const CRC: [u32; 256] = crc_table();
fn crc32(input: &[u8]) -> u32 {
    !input
        .iter()
        .fold(!0u32, |v, b| CRC[((v as u8) ^ b) as usize] ^ (v >> 8))
}

fn inflate_exact(input: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    if expected > 96 * 1024 * 1024 {
        return Err("Expanded member exceeds the 96 MiB limit".into());
    }
    let mut out = vec![0; expected.saturating_add(1)];
    let mut state = Box::<DecompressorOxide>::default();
    let (status, consumed, written) = decompress(
        &mut state,
        input,
        &mut out,
        0,
        inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    if status != TINFLStatus::Done || consumed != input.len() || written != expected {
        return Err("Invalid DEFLATE stream, trailing bytes, or unexpected size".into());
    }
    out.truncate(written);
    Ok(out)
}

fn smaller_deflate(source: &[u8], original: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    let mut best = original.to_vec();
    let levels: &[u8] = if thorough { &[6, 9, 10] } else { &[9] };
    for &level in levels {
        let cand = miniz_oxide::deflate::compress_to_vec(source, level);
        if cand.len() < best.len() {
            best = cand;
        }
    }
    if thorough
        && source.len() <= 4 * 1024 * 1024
        && let Ok(zlib) = pixminima_png::compress_zlib_thorough(source)
    {
        let raw = &zlib[2..zlib.len() - 4];
        if raw.len() < best.len() {
            best = raw.to_vec();
        }
    }
    if best != original && inflate_exact(&best, source.len())? != source {
        return Err("Recompression failed its round-trip check".into());
    }
    Ok(best)
}

#[cfg(test)]
mod tests;
