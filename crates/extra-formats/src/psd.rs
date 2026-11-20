//! Repack only the final merged image. Header, resource, layer and mask bytes are
//! copied as one untouched prefix, so private Photoshop records are not rewritten.
use super::zlib::inflate_exact;
use super::{
    MAX_DECODED,
    packets::{Encoding, Packetizer},
};
#[cfg(test)]
use miniz_oxide::deflate::compress_to_vec_zlib;

pub(super) fn detect(b: &[u8]) -> Option<&'static str> {
    if b.get(..4) != Some(b"8BPS") {
        return None;
    }
    match b.get(4..6) {
        Some([0, 1]) => Some("PSD"),
        Some([0, 2]) => Some("PSB"),
        _ => None,
    }
}
fn take<'a>(tail: &mut &'a [u8], len: usize) -> Result<&'a [u8], String> {
    let (field, rest) = tail
        .split_at_checked(len)
        .ok_or("Truncated Photoshop section")?;
    *tail = rest;
    Ok(field)
}
fn u16be(b: &[u8]) -> usize {
    usize::from(u16::from_be_bytes(b[..2].try_into().unwrap()))
}
fn u32be(b: &[u8]) -> usize {
    u32::from_be_bytes(b[..4].try_into().unwrap()) as usize
}
fn skip_section(tail: &mut &[u8], wide: bool) -> Result<(), String> {
    let len = if wide {
        usize::try_from(u64::from_be_bytes(take(tail, 8)?.try_into().unwrap()))
            .map_err(|_| "Photoshop section length overflow")?
    } else {
        u32be(take(tail, 4)?)
    };
    take(tail, len)?;
    Ok(())
}
struct Composite<'a> {
    prefix: &'a [u8],
    tail: &'a [u8],
    rows: usize,
    stride: usize,
    depth: usize,
    psb: bool,
}
fn parse(b: &[u8]) -> Result<Composite<'_>, String> {
    let psb = detect(b).ok_or("Unknown Photoshop file version")? == "PSB";
    let mut tail = b;
    let hdr = take(&mut tail, 26)?;
    let channels = u16be(&hdr[12..]);
    let height = u32be(&hdr[14..]);
    let width = u32be(&hdr[18..]);
    let depth = u16be(&hdr[22..]);
    let limit = if psb { 300_000 } else { 30_000 };
    if hdr[6..12] != [0; 6]
        || !(1..=56).contains(&channels)
        || !(1..=limit).contains(&width)
        || !(1..=limit).contains(&height)
        || !matches!(depth, 1 | 8 | 16 | 32)
        || !matches!(u16be(&hdr[24..]), 0 | 1 | 2 | 3 | 4 | 7 | 8 | 9)
    {
        return Err("Unsupported Photoshop header or dimensions".into());
    }
    let stride = width
        .checked_mul(depth)
        .ok_or("Photoshop scanline overflow")?
        .div_ceil(8);
    let rows = height
        .checked_mul(channels)
        .ok_or("Photoshop row count overflow")?;
    if !rows.checked_mul(stride).is_some_and(|n| n <= MAX_DECODED) {
        return Err("Photoshop composite exceeds the decoded size limit".into());
    }
    skip_section(&mut tail, false)?;
    skip_section(&mut tail, false)?;
    skip_section(&mut tail, psb)?;
    Ok(Composite {
        prefix: &b[..b.len() - tail.len()],
        tail,
        rows,
        stride,
        depth,
        psb,
    })
}
fn unpack(row: &[u8], stride: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let end = out.len() + stride;
    let mut packet = row;
    while !packet.is_empty() {
        let h = take(&mut packet, 1)?[0];
        let n = match h {
            0..=127 => usize::from(h) + 1,
            128 => continue,
            _ => 257 - usize::from(h),
        };
        if n > end - out.len() {
            return Err("Photoshop PackBits row overflow".into());
        }
        if h < 128 {
            out.extend_from_slice(take(&mut packet, n)?);
        } else {
            let value = take(&mut packet, 1)?[0];
            out.resize(out.len() + n, value);
        }
    }
    if out.len() != end {
        return Err("Short Photoshop PackBits row".into());
    }
    Ok(())
}
fn decode(c: &Composite<'_>, bits: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut b = bits;
    if b.is_empty() {
        return Ok(None);
    }
    let method = u16be(take(&mut b, 2)?);
    let len = c.stride * c.rows;
    let raw = match method {
        0 => {
            if b.len() != len {
                if b.is_empty() || b.len() > len {
                    return Ok(None);
                }
                return Err("Truncated Photoshop composite".into());
            }
            b.to_vec()
        }
        1 => {
            let word = if c.psb { 4 } else { 2 };
            let table = take(&mut b, c.rows * word)?;
            let mut data = Vec::with_capacity(len);
            for entry in table.chunks_exact(word) {
                let n = if c.psb { u32be(entry) } else { u16be(entry) };
                unpack(take(&mut b, n)?, c.stride, &mut data)?;
            }
            if !b.is_empty() {
                return Ok(None);
            }
            data
        }
        2 => inflate_exact(b, len)?,
        3 if c.depth != 1 => {
            let mut predicted = inflate_exact(b, len)?;
            let mut row32 = vec![0; if c.depth == 32 { c.stride } else { 0 }];
            for row in predicted.chunks_exact_mut(c.stride) {
                match c.depth {
                    16 => {
                        let mut prev = 0u16;
                        for value in row.as_chunks_mut::<2>().0 {
                            prev = prev.wrapping_add(u16::from_be_bytes(*value));
                            *value = prev.to_be_bytes();
                        }
                    }
                    8 | 32 => {
                        for i in 1..row.len() {
                            row[i] = row[i].wrapping_add(row[i - 1]);
                        }
                        if c.depth == 32 {
                            let width = c.stride / 4;
                            for (x, p) in row32.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                                for i in 0..4 {
                                    p[i] = row[i * width + x];
                                }
                            }
                            row.copy_from_slice(&row32);
                        }
                    }
                    _ => unreachable!(),
                }
            }
            predicted
        }
        _ => return Ok(None),
    };
    Ok(Some(raw))
}
fn packed(c: &Composite<'_>, rast: &[u8]) -> Option<Vec<u8>> {
    let word = if c.psb { 4 } else { 2 };
    if c.rows * word + 2 >= c.tail.len() {
        return None;
    }
    let mut out = vec![0; 2 + c.rows * word];
    out[1] = 1;
    let mut packets = Packetizer::default();
    for (i, row) in rast.chunks_exact(c.stride).enumerate() {
        let prior = out.len();
        packets.append(row, 1, Encoding::Photoshop, &mut out);
        let count = out.len() - prior;
        let off = 2 + i * word;
        if c.psb {
            out[off..off + 4].copy_from_slice(&u32::try_from(count).ok()?.to_be_bytes());
        } else {
            out[off..off + 2].copy_from_slice(&u16::try_from(count).ok()?.to_be_bytes());
        }
        if out.len() >= c.tail.len() {
            return None;
        }
    }
    Some(out)
}
pub(super) fn optimize(input: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let c = parse(input)?;
    if c.prefix.windows(4).any(|w| w.eq_ignore_ascii_case(b"c2pa")) {
        return Ok(None);
    }
    let Some(raw) = decode(&c, c.tail)? else {
        return Ok(None);
    };
    let mut best = packed(&c, &raw);
    // ImageIO and Pillow can open raw and PackBits composites but can fail to
    // open a valid ZIP composite. Preserve the original decoder requirements.
    if matches!(u16be(c.tail), 2 | 3) {
        let zip = super::zlib::deflate(&raw, thorough);
        if 2 + zip.len() < best.as_ref().map_or(c.tail.len(), Vec::len) {
            let mut out = vec![0, 2];
            out.extend(zip);
            best = Some(out);
        }
    }
    if let Some(tail) = best {
        if decode(&c, &tail)?.is_none_or(|other| other != raw) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(c.prefix.len() + tail.len());
        out.extend(c.prefix);
        out.extend(tail);
        Ok(Some(out))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(psb: bool, width: u32, depth: u16, data: &[u8]) -> Vec<u8> {
        let mut buf = b"8BPS".to_vec();
        buf.extend((if psb { 2u16 } else { 1u16 }).to_be_bytes());
        buf.extend([0; 6]);
        buf.extend(1u16.to_be_bytes());
        buf.extend(2u32.to_be_bytes());
        buf.extend(width.to_be_bytes());
        buf.extend(depth.to_be_bytes());
        buf.extend((if depth == 1 { 0u16 } else { 1u16 }).to_be_bytes());
        for (i, payload) in [b"".as_slice(), b"8BIMopaque", b"LayersMasksXYZ"]
            .iter()
            .enumerate()
        {
            if i == 2 && psb {
                buf.extend((payload.len() as u64).to_be_bytes());
            } else {
                buf.extend((payload.len() as u32).to_be_bytes());
            }
            buf.extend(*payload);
        }
        buf.extend(data);
        buf
    }
    #[test]
    fn packs_standard_and_large_document_composites_without_touching_sections() {
        for psb in [false, true] {
            for depth in [1u16, 8, 16, 32] {
                let mut bytes = vec![0, 0];
                bytes.extend(vec![0x88; 64 * usize::from(depth) / 8 * 2]);
                let input = file(psb, 64, depth, &bytes);
                let candidate = optimize(&input, true).unwrap().unwrap();
                let a = parse(&input).unwrap();
                let b = parse(&candidate).unwrap();
                assert_eq!(a.prefix, b.prefix);
                assert_eq!(u16be(b.tail), 1);
                assert_eq!(decode(&b, b.tail).unwrap().unwrap(), &bytes[2..]);
            }
        }
    }
    #[test]
    fn zip_prediction_16_and_32_retain_unsigned_bits_and_float_payloads() {
        for (depth, raw, predicted) in [
            (
                8,
                vec![0x80, 0x01, 0x77, 0x21],
                vec![0x80, 0x81, 0x77, 0xaa],
            ),
            (
                16,
                vec![0xff, 0xfe, 0, 1, 0, 1, 0xff, 0xff],
                vec![0xff, 0xfe, 0, 3, 0, 1, 0xff, 0xfe],
            ),
            (
                32,
                vec![
                    0x7f, 0xc0, 0x11, 0x32, 0x80, 0, 0, 0, 0, 0, 0, 0, 0x7f, 0xfa, 1, 2,
                ],
                vec![
                    0x7f, 1, 0x40, 0x40, 0x11, 0xef, 0x32, 0xce, 0, 0x7f, 0x81, 0xfa, 6, 1, 0xff, 2,
                ],
            ),
        ] {
            let mut method = vec![0, 3];
            method.extend(compress_to_vec_zlib(&predicted, 6));
            let b = file(true, 2, depth, &method);
            let c = parse(&b).unwrap();
            assert_eq!(decode(&c, c.tail).unwrap(), Some(raw));
        }
    }
    #[test]
    fn preserves_unknown_composites_and_private_trailers_rejects_corruption() {
        for tail in [&[0, 42, 0, 9][..], &[0, 0], &[], &[0, 0, 4, 3, 0xaa]] {
            let b = file(false, 1, 8, tail);
            assert!(optimize(&b, true).unwrap().is_none());
        }
        for tail in [
            &[0, 2, 0xff][..],
            &[0, 1, 0, 2, 0, 2, 0xff, 1, 0x80, 1],
            &[0, 0, 1],
        ] {
            assert!(optimize(&file(false, 1, 8, tail), true).is_err());
        }
    }
}
