use super::MAX_DECODED;
use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
use zune_qoi::QoiEncoder;

const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
const OPAQUE_BLACK: [u8; 4] = [0, 0, 0, 255];

fn hash(p: [u8; 4]) -> usize {
    let [r, g, b, a] = p.map(usize::from);
    (r * 3 + g * 5 + b * 7 + a * 11) % 64
}
fn read_n<'a>(stream: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
    let (head, rest) = stream.split_at_checked(n).ok_or("Truncated QOI packet")?;
    *stream = rest;
    Ok(head)
}
// Independent, strict reader: the channels field is informational and cannot be
// used to drop hidden alpha. Runs must fit, and the end marker must terminate the file.
pub(super) fn decode(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() < 22
        || &input[..4] != b"qoif"
        || !matches!(input[12], 3 | 4)
        || input[13] > 1
        || !input.ends_with(&END)
    {
        return Err("Invalid QOI header or end marker".into());
    }
    let w = u32::from_be_bytes(input[4..8].try_into().unwrap()) as usize;
    let h = u32::from_be_bytes(input[8..12].try_into().unwrap()) as usize;
    let n = w
        .checked_mul(h)
        .filter(|&v| v > 0 && v <= MAX_DECODED / 4)
        .ok_or("QOI dimensions exceed the decoded size limit")?;
    let mut stream = &input[14..input.len() - 8];
    let mut out = Vec::with_capacity(n * 4);
    let mut index = [[0; 4]; 64];
    let mut p = OPAQUE_BLACK;
    while out.len() < n * 4 {
        let op = read_n(&mut stream, 1)?[0];
        let mut run = 1;
        match op {
            0xfe => p[..3].copy_from_slice(read_n(&mut stream, 3)?),
            0xff => p.copy_from_slice(read_n(&mut stream, 4)?),
            0x00..=0x3f => p = index[usize::from(op)],
            0x40..=0x7f => {
                for (v, shift) in p[..3].iter_mut().zip([4, 2, 0]) {
                    *v = v.wrapping_add((op >> shift) & 3).wrapping_sub(2);
                }
            }
            0x80..=0xbf => {
                let b = read_n(&mut stream, 1)?[0];
                let g = (op & 63).wrapping_sub(32);
                p[0] = p[0].wrapping_add(g).wrapping_add(b >> 4).wrapping_sub(8);
                p[1] = p[1].wrapping_add(g);
                p[2] = p[2].wrapping_add(g).wrapping_add(b & 15).wrapping_sub(8);
            }
            _ => run = usize::from(op & 63) + 1,
        }
        if run > (n * 4 - out.len()) / 4 {
            return Err("QOI run exceeds the declared dimensions".into());
        }
        index[hash(p)] = p;
        for _ in 0..run {
            out.extend(p);
        }
    }
    if !stream.is_empty() {
        return Err("Extra data after the QOI raster".into());
    }
    Ok(out)
}

pub(super) fn optimize(input: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let pixels = decode(input)?;
    let width = u32::from_be_bytes(input[4..8].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(input[8..12].try_into().unwrap()) as usize;
    let options = EncoderOptions::new(width, height, ColorSpace::RGBA, BitDepth::Eight);
    let mut output = Vec::new();
    if QoiEncoder::new(&pixels, options)
        .encode(&mut output)
        .is_err()
    {
        return Ok(None);
    }
    if output.len() >= input.len() {
        return Ok(None);
    }
    output[..14].copy_from_slice(&input[..14]);
    if decode(&output).is_ok_and(|candidate| candidate == pixels) {
        Ok(Some(output))
    } else {
        Ok(None)
    }
}
