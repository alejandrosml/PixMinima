use super::{
    MAX_DECODED, MAX_INPUT,
    packets::{Encoding, Packetizer},
};

const SIGNATURE: &[u8; 18] = b"TRUEVISION-XFILE.\0";
fn le16(b: &[u8]) -> usize {
    usize::from(u16::from_le_bytes([b[0], b[1]]))
}
fn le32(b: &[u8]) -> usize {
    u32::from_le_bytes(b[..4].try_into().unwrap()) as usize
}

#[derive(Clone, Copy)]
struct Header {
    start: usize,
    width: usize,
    pixels: usize,
    bpp: usize,
    base: u8,
    rle: bool,
    cmap_start: usize,
    cmap_len: usize,
}
fn header(b: &[u8]) -> Result<Header, String> {
    if b.len() < 18 || b.len() > MAX_INPUT {
        return Err("Invalid TGA file size".into());
    }
    let (base, rle) = match b[2] {
        1..=3 => (b[2], false),
        9..=11 => (b[2] - 8, true),
        _ => return Err("Unsupported TGA image type".into()),
    };
    let width = le16(&b[12..]);
    let height = le16(&b[14..]);
    let depth = b[16];
    if width == 0
        || height == 0
        || b[1] > 1
        || b[17] >> 6 == 3
        || (b[17] & 15) > 8
        || !match base {
            1 => matches!(depth, 8 | 16),
            2 => matches!(depth, 15 | 16 | 24 | 32),
            3 => matches!(depth, 8 | 16),
            _ => false,
        }
    {
        return Err("Unsupported TGA pixel layout".into());
    }
    let cmap_start = le16(&b[3..]);
    let cmap_len = le16(&b[5..]);
    let cmap_depth = b[7];
    if b[1] == 1 {
        if cmap_len == 0
            || !matches!(cmap_depth, 15 | 16 | 24 | 32)
            || cmap_start + cmap_len > 65536
        {
            return Err("Invalid TGA color map".into());
        }
    } else if base == 1 || cmap_start != 0 || cmap_len != 0 || cmap_depth != 0 {
        return Err("Inconsistent TGA color-map header".into());
    }
    let bpp = usize::from(depth).div_ceil(8);
    let pixels = width
        .checked_mul(height)
        .filter(|&n| n <= MAX_DECODED / bpp)
        .ok_or("TGA exceeds the decoded size limit")?;
    let start = 18 + usize::from(b[0]) + cmap_len * usize::from(cmap_depth).div_ceil(8);
    if start > b.len() {
        return Err("Truncated TGA identifier or color map".into());
    }
    Ok(Header {
        start,
        width,
        pixels,
        bpp,
        base,
        rle,
        cmap_start,
        cmap_len,
    })
}

// Walk twice, first validating the encoded byte count without allocating a raster.
fn walk(b: &[u8], h: Header, mut out: Option<&mut Vec<u8>>) -> Result<usize, String> {
    let mut at = h.start;
    let mut left = h.pixels;
    while left > 0 {
        let (count, repeat) = if h.rle {
            let &packet = b.get(at).ok_or("Truncated TGA packet header")?;
            at += 1;
            (usize::from(packet & 127) + 1, packet & 128 != 0)
        } else {
            (left, false)
        };
        if count > left {
            return Err("TGA packet exceeds image boundaries".into());
        }
        let n = if repeat { h.bpp } else { count * h.bpp };
        let data = b.get(at..at + n).ok_or("Truncated TGA pixel packet")?;
        if h.base == 1 {
            for p in data.chunks_exact(h.bpp) {
                let index = if h.bpp == 1 {
                    usize::from(p[0])
                } else {
                    le16(p)
                };
                if !(h.cmap_start..h.cmap_start + h.cmap_len).contains(&index) {
                    return Err("TGA palette index out of range".into());
                }
            }
        }
        if let Some(v) = out.as_deref_mut() {
            if repeat {
                for _ in 0..count {
                    v.extend_from_slice(data);
                }
            } else {
                v.extend_from_slice(data);
            }
        }
        left -= count;
        at += n;
    }
    Ok(at)
}
fn footer(b: &[u8]) -> Option<&[u8]> {
    (b.len() >= 26 && b.ends_with(SIGNATURE)).then(|| &b[b.len() - 26..])
}
pub(super) fn recognized(b: &[u8]) -> bool {
    if footer(b).is_some() {
        return true;
    }
    header(b).is_ok_and(|h| walk(b, h, None) == Ok(b.len()))
}
pub(super) fn optimize(b: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let h = header(b)?;
    let end = walk(b, h, None)?;
    let trailing = &b[end..];
    if let Some(f) = footer(b) {
        if end > b.len() - 26 {
            return Err("TGA pixel stream overlaps its footer".into());
        }
        let boundary = b.len() - 26;
        for pos in [le32(f), le32(&f[4..])] {
            if pos != 0 && !(end..boundary).contains(&pos) {
                return Err("Invalid TGA extension or developer offset".into());
            }
        }
        // Extension/developer data may contain absolute offsets into pixels or
        // ancillary resources; preserve this entire file without relocating them.
        if trailing.len() != 26 || f[..8] != [0; 8] {
            return Ok(None);
        }
    } else if !trailing.is_empty() {
        return Ok(None);
    }
    let mut raster = Vec::with_capacity(h.pixels * h.bpp);
    walk(b, h, Some(&mut raster))?;
    let mut packed = Vec::new();
    let mut packets = Packetizer::default();
    for row in raster.chunks_exact(h.width * h.bpp) {
        packets.append(row, h.bpp, Encoding::Targa, &mut packed);
    }
    let (payload, kind) = if packed.len() < raster.len() {
        (packed.as_slice(), h.base + 8)
    } else {
        (raster.as_slice(), h.base)
    };
    if payload.len() >= end - h.start {
        return Ok(None);
    }
    let mut result = b[..h.start].to_vec();
    result[2] = kind;
    result.extend_from_slice(payload);
    result.extend_from_slice(trailing);
    let hh = header(&result)?;
    let mut decoded = Vec::with_capacity(raster.len());
    let data_end = walk(&result, hh, Some(&mut decoded))?;
    if decoded != raster || result[data_end..] != *trailing {
        return Ok(None);
    }
    Ok(Some(result))
}
