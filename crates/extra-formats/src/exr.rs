//! Repack single-part scanline images in their serialized sample representation.
//! No arithmetic on HALF/FLOAT samples, and the header except compression is copied verbatim.
use super::zlib::deflate;
use super::{MAX_DECODED, zlib::inflate_exact};
use std::collections::BTreeSet;

pub(super) const MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];
const MAX_HEIGHT: usize = 1_000_000;
fn take<'a>(tail: &mut &'a [u8], len: usize) -> Result<&'a [u8], String> {
    let (field, rest) = tail.split_at_checked(len).ok_or("Truncated OpenEXR file")?;
    *tail = rest;
    Ok(field)
}
fn name<'a>(tail: &mut &'a [u8], max: usize) -> Result<&'a [u8], String> {
    let len = tail
        .iter()
        .take(max + 1)
        .position(|v| *v == 0)
        .ok_or("Unterminated OpenEXR name")?;
    Ok(&take(tail, len + 1)?[..len])
}
fn i32le(b: &[u8]) -> i32 {
    i32::from_le_bytes(b[..4].try_into().unwrap())
}
fn channels(mut value: &[u8], max_name: usize) -> Result<Option<usize>, String> {
    let mut bytes = 0usize;
    let mut names = BTreeSet::new();
    loop {
        let n = name(&mut value, max_name)?;
        if n.is_empty() {
            break;
        }
        if !names.insert(n) || names.len() > 1024 {
            return Err("Invalid OpenEXR channel list".into());
        }
        let desc = take(&mut value, 16)?;
        bytes += match i32le(desc) {
            0 | 2 => 4,
            1 => 2,
            _ => return Ok(None),
        };
        if desc[4] > 1 || desc[5..8] != [0; 3] || i32le(&desc[8..]) != 1 || i32le(&desc[12..]) != 1
        {
            return Ok(None);
        }
    }
    if bytes == 0 || !value.is_empty() {
        return Err("Malformed OpenEXR channel list".into());
    }
    Ok(Some(bytes))
}
#[derive(Debug)]
struct Layout {
    header_len: usize,
    mode_at: usize,
    mode: u8,
    height: usize,
    row_bytes: usize,
    ymin: i32,
    descending: bool,
}
fn dimensions(value: &[u8]) -> Result<(usize, usize, i32), String> {
    if value.len() != 16 {
        return Err("Invalid OpenEXR window".into());
    }
    let xmin = i64::from(i32le(value));
    let ymin = i32le(&value[4..]);
    let w = usize::try_from(i64::from(i32le(&value[8..])) - xmin + 1)
        .ok()
        .filter(|v| *v > 0)
        .ok_or("Invalid OpenEXR width")?;
    let h = usize::try_from(i64::from(i32le(&value[12..])) - i64::from(ymin) + 1)
        .ok()
        .filter(|v| *v > 0)
        .ok_or("Invalid OpenEXR height")?;
    Ok((w, h, ymin))
}
fn layout(input: &[u8]) -> Result<Option<Layout>, String> {
    let mut tail = input;
    if take(&mut tail, 4)? != MAGIC {
        return Err("Invalid OpenEXR signature".into());
    }
    let version = i32le(take(&mut tail, 4)?);
    if !matches!(version, 2 | 1026) {
        return Ok(None);
    }
    let max_name = if version == 2 { 31 } else { 255 };
    let mut channel_size = None;
    let mut window = None;
    let mut compression = None;
    let mut descending = None;
    let mut names = BTreeSet::new();
    loop {
        let attr = name(&mut tail, max_name)?;
        if attr.is_empty() {
            break;
        }
        if names.len() >= 1024 || !names.insert(attr) {
            return Err("Invalid OpenEXR attribute list".into());
        }
        let typ = name(&mut tail, max_name)?;
        let size = usize::try_from(i32le(take(&mut tail, 4)?))
            .map_err(|_| "Negative OpenEXR attribute length")?;
        let at = input.len() - tail.len();
        let value = take(&mut tail, size)?;
        match (attr, typ) {
            (b"type", b"string") if value == b"scanlineimage" => {}
            (b"channels", b"chlist") => {
                channel_size = channels(value, max_name)?;
                if channel_size.is_none() {
                    return Ok(None);
                }
            }
            (b"dataWindow", b"box2i") => {
                window = Some(dimensions(value)?);
            }
            (b"displayWindow", b"box2i") => {
                dimensions(value)?;
            }
            (b"compression", b"compression") if size == 1 => {
                if value[0] > 3 {
                    return Ok(None);
                }
                compression = Some((at, value[0]));
            }
            (b"lineOrder", b"lineOrder") if size == 1 => {
                descending = Some(match value[0] {
                    0 => false,
                    1 => true,
                    _ => return Ok(None),
                });
            }
            (b"pixelAspectRatio" | b"screenWindowWidth", b"float") if size == 4 => {}
            (b"screenWindowCenter", b"v2f") if size == 8 => {}
            // Copy only recognized descriptive attributes, whose values carry no file offsets.
            (b"chromaticities", b"chromaticities") if size == 32 => {}
            (b"adoptedNeutral", b"v2f") if size == 8 => {}
            (
                b"xDensity" | b"whiteLuminance" | b"utcOffset" | b"longitude" | b"latitude"
                | b"altitude" | b"focus" | b"expTime" | b"aperture" | b"isoSpeed",
                b"float",
            ) if size == 4 => {}
            (
                b"renderingTransform"
                | b"lookModTransform"
                | b"owner"
                | b"comments"
                | b"capDate"
                | b"wrapmodes"
                | b"software"
                | b"view"
                | b"colorInteropID"
                | b"cameraMake"
                | b"cameraModel"
                | b"cameraSerialNumber"
                | b"lensMake"
                | b"lensModel"
                | b"lensSerialNumber",
                b"string",
            ) => {}
            (b"envmap", b"envmap") if size == 1 => {}
            (b"keyCode", b"keycode") if size == 28 => {}
            (b"timeCode", b"timecode") if size == 8 => {}
            (b"framesPerSecond", b"rational") if size == 8 => {}
            (b"multiView", b"stringvector") => {}
            (b"worldToCamera" | b"worldToNDC", b"m44f") if size == 64 => {}
            (b"originalDataWindow", b"box2i") if size == 16 => {}
            (b"imageCounter", b"int") if size == 4 => {}
            (b"preview", b"preview") => {}
            _ => return Ok(None),
        }
    }
    let (width, height, ymin) = window.ok_or("Missing OpenEXR dataWindow")?;
    let row_bytes = channel_size
        .ok_or("Missing OpenEXR channels")?
        .checked_mul(width)
        .ok_or("OpenEXR row overflow")?;
    if height > MAX_HEIGHT
        || !row_bytes
            .checked_mul(height)
            .is_some_and(|n| n <= MAX_DECODED)
    {
        return Err("OpenEXR exceeds decoded size limits".into());
    }
    let (mode_at, mode) = compression.ok_or("Missing OpenEXR compression")?;
    for required in [
        b"displayWindow".as_slice(),
        b"pixelAspectRatio",
        b"screenWindowCenter",
        b"screenWindowWidth",
    ] {
        if !names.contains(required) {
            return Err("Missing required OpenEXR attribute".into());
        }
    }
    if input[..input.len() - tail.len()]
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"c2pa"))
    {
        return Ok(None);
    }
    Ok(Some(Layout {
        height,
        row_bytes,
        ymin,
        mode_at,
        mode,
        descending: descending.ok_or("Missing OpenEXR line order")?,
        header_len: input.len() - tail.len(),
    }))
}
fn zip_encode(raw: &[u8], thorough: bool) -> Vec<u8> {
    let mut ordered: Vec<u8> = Vec::with_capacity(raw.len());
    ordered.extend(raw.iter().step_by(2));
    ordered.extend(raw.iter().skip(1).step_by(2));
    for i in (1..ordered.len()).rev() {
        ordered[i] = ordered[i].wrapping_sub(ordered[i - 1]).wrapping_add(128);
    }
    deflate(&ordered, thorough)
}
fn rle_decode(mut payload: &[u8], count: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(count);
    while !payload.is_empty() {
        let marker = take(&mut payload, 1)?[0] as i8;
        let n = if marker < 0 {
            usize::from(marker.unsigned_abs())
        } else {
            marker as usize + 1
        };
        if n > count - out.len() {
            return Err("OpenEXR run crosses block boundary".into());
        }
        if marker < 0 {
            out.extend_from_slice(take(&mut payload, n)?);
        } else {
            let v = take(&mut payload, 1)?[0];
            out.resize(out.len() + n, v);
        }
    }
    if out.len() != count {
        return Err("Incomplete OpenEXR run data".into());
    }
    Ok(out)
}
fn decompress_block(mode: u8, blob: &[u8], raw: &mut [u8]) -> Result<(), String> {
    let mut ordered = if mode == 1 {
        rle_decode(blob, raw.len())?
    } else {
        inflate_exact(blob, raw.len())?
    };
    for i in 1..ordered.len() {
        ordered[i] = ordered[i].wrapping_add(ordered[i - 1]).wrapping_sub(128);
    }
    let (even, odd) = ordered.split_at(ordered.len().div_ceil(2));
    for (pixel, &x) in raw.iter_mut().step_by(2).zip(even) {
        *pixel = x;
    }
    for (pixel, &x) in raw.iter_mut().skip(1).step_by(2).zip(odd) {
        *pixel = x;
    }
    Ok(())
}
fn block_rows(mode: u8) -> usize {
    if mode == 3 { 16 } else { 1 }
}
fn decode(input: &[u8], l: &Layout) -> Result<Option<Vec<u8>>, String> {
    let nrows = block_rows(l.mode);
    let blocks = l.height.div_ceil(nrows);
    let mut tail = &input[l.header_len..];
    let offsets = take(&mut tail, blocks * 8)?;
    let mut raw = vec![0u8; l.height * l.row_bytes];
    for physical in 0..blocks {
        let id = if l.descending {
            blocks - 1 - physical
        } else {
            physical
        };
        let start = id * nrows;
        let n = (l.height - start).min(nrows) * l.row_bytes;
        let pos = u64::from_le_bytes(offsets[id * 8..][..8].try_into().unwrap());
        // The offsets must describe contiguous chunks in the declared physical order.
        if pos != (input.len() - tail.len()) as u64 {
            return Ok(None);
        }
        let y = i32le(take(&mut tail, 4)?);
        if i64::from(y) != i64::from(l.ymin) + start as i64 {
            return Err("OpenEXR chunk coordinate mismatch".into());
        }
        let len = usize::try_from(i32le(take(&mut tail, 4)?))
            .map_err(|_| "Invalid OpenEXR chunk length")?;
        let payload = take(&mut tail, len)?;
        let chunk = &mut raw[start * l.row_bytes..][..n];
        if len == n {
            chunk.copy_from_slice(payload);
        } else if l.mode != 0 && len < n && len > 0 {
            decompress_block(l.mode, payload, chunk)?;
        } else {
            return Err("Invalid OpenEXR sample payload size".into());
        }
    }
    if !tail.is_empty() {
        return Ok(None);
    }
    Ok(Some(raw))
}
fn encode_zip16(input: &[u8], l: &Layout, raw: &[u8], thorough: bool) -> Option<Vec<u8>> {
    let mode = 3;
    let nrows = block_rows(mode);
    let blocks = l.height.div_ceil(nrows);
    let mut out = input[..l.header_len].to_vec();
    out[l.mode_at] = mode;
    out.resize(out.len() + blocks * 8, 0);
    // The preceding input and output hold both sides of a cache entry already.
    // Reuse identical scanline blocks without retaining another raster copy.
    let mut recent =
        std::collections::VecDeque::<(std::ops::Range<usize>, std::ops::Range<usize>)>::new();
    for physical in 0..blocks {
        let id = if l.descending {
            blocks - 1 - physical
        } else {
            physical
        };
        let row = id * nrows;
        let rows = (l.height - row).min(nrows);
        let source = row * l.row_bytes..(row + rows) * l.row_bytes;
        let pixels = &raw[source.clone()];
        let cached = recent
            .iter()
            .find(|(src, _)| &raw[src.clone()] == pixels)
            .map(|(_, encoded)| encoded.clone());
        let off = (out.len() as u64).to_le_bytes();
        out[l.header_len + id * 8..][..8].copy_from_slice(&off);
        let y = i32::try_from(i64::from(l.ymin) + row as i64).ok()?;
        out.extend(y.to_le_bytes());
        let position = out.len();
        out.extend([0; 4]);
        let start = out.len();
        if let Some(range) = cached {
            out.extend_from_within(range);
        } else {
            let chunk = zip_encode(pixels, thorough);
            out.extend_from_slice(if chunk.len() < pixels.len() {
                &chunk
            } else {
                pixels
            });
            if recent.len() == 8 {
                recent.pop_front();
            }
            recent.push_back((source, start..out.len()));
        }
        let count = (out.len() - start) as i32;
        out[position..start].copy_from_slice(&count.to_le_bytes());
        if out.len() >= input.len() {
            return None;
        }
    }
    Some(out)
}
pub(super) fn optimize(input: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let Some(src) = layout(input)? else {
        return Ok(None);
    };
    let Some(raw) = decode(input, &src)? else {
        return Ok(None);
    };
    if let Some(candidate) = encode_zip16(input, &src, &raw, thorough) {
        let Some(dst) = layout(&candidate)? else {
            return Ok(None);
        };
        if decode(&candidate, &dst)?.is_some_and(|samples| samples == raw) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attr(buf: &mut Vec<u8>, name: &str, typ: &str, value: &[u8]) {
        buf.extend(name.as_bytes());
        buf.push(0);
        buf.extend(typ.as_bytes());
        buf.push(0);
        buf.extend((value.len() as u32).to_le_bytes());
        buf.extend(value);
    }
    fn raw_file(desc: bool, extra: bool) -> (Vec<u8>, Vec<u8>) {
        let height = 19usize;
        let width = 127usize;
        let mut b = MAGIC.to_vec();
        b.extend(2u32.to_le_bytes());
        let mut descs = Vec::new();
        for (n, t) in [("f", 2i32), ("h", 1), ("u", 0)] {
            descs.extend(n.as_bytes());
            descs.push(0);
            descs.extend(t.to_le_bytes());
            descs.extend([0; 4]);
            descs.extend(1u32.to_le_bytes());
            descs.extend(1u32.to_le_bytes());
        }
        descs.push(0);
        attr(&mut b, "channels", "chlist", &descs);
        attr(&mut b, "compression", "compression", &[0]);
        let window = [-2i32, -5, width as i32 - 3, height as i32 - 6]
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect::<Vec<_>>();
        attr(&mut b, "displayWindow", "box2i", &window);
        attr(&mut b, "dataWindow", "box2i", &window);
        attr(&mut b, "lineOrder", "lineOrder", &[u8::from(desc)]);
        attr(&mut b, "pixelAspectRatio", "float", &1f32.to_le_bytes());
        attr(&mut b, "screenWindowCenter", "v2f", &[0; 8]);
        attr(&mut b, "screenWindowWidth", "float", &1f32.to_le_bytes());
        attr(&mut b, "comments", "string", b"stored verbatim");
        if extra {
            attr(&mut b, "privateOffset", "int", &[0; 4]);
        }
        b.push(0);
        let table = b.len();
        b.resize(b.len() + height * 8, 0);
        let mut raw = Vec::new();
        for y in 0..height {
            for _ in 0..width {
                raw.extend((0x7f800001u32 + y as u32).to_le_bytes());
            }
            for _ in 0..width {
                raw.extend((0xfc01u16 + y as u16).to_le_bytes());
            }
            for _ in 0..width {
                raw.extend((0xcafef00du32 + y as u32).to_le_bytes());
            }
        }
        let row = width * 10;
        for i in 0..height {
            let y = if desc { height - i - 1 } else { i };
            let off = (b.len() as u64).to_le_bytes();
            b[table + y * 8..][..8].copy_from_slice(&off);
            b.extend((y as i32 - 5).to_le_bytes());
            b.extend((row as i32).to_le_bytes());
            b.extend(&raw[y * row..][..row]);
        }
        (b, raw)
    }
    #[test]
    fn repacks_raw_samples_and_retains_header_for_both_line_orders() {
        for desc in [false, true] {
            let (input, raw) = raw_file(desc, false);
            let l = layout(&input).unwrap().unwrap();
            assert_eq!(decode(&input, &l).unwrap().unwrap(), raw);
            let out = optimize(&input, true).unwrap().unwrap();
            assert!(out.len() < input.len());
            let l = layout(&out).unwrap().unwrap();
            assert_eq!(l.mode, 3);
            assert_eq!(decode(&out, &l).unwrap().unwrap(), raw);
            assert_eq!(&input[..l.mode_at], &out[..l.mode_at]);
            assert_eq!(
                &input[l.mode_at + 1..l.header_len],
                &out[l.mode_at + 1..l.header_len]
            );
        }
    }
    #[test]
    fn foreign_metadata_gaps_trailers_and_unsupported_storage_are_conserved() {
        assert!(optimize(&raw_file(false, true).0, true).unwrap().is_none());
        let (mut b, _) = raw_file(false, false);
        let l = layout(&b).unwrap().unwrap();
        b.push(33);
        assert!(optimize(&b, true).unwrap().is_none());
        b.pop();
        b[l.header_len] += 1;
        assert!(optimize(&b, true).unwrap().is_none());
        b[l.header_len] -= 1;
        b[l.mode_at] = 4;
        assert!(optimize(&b, true).unwrap().is_none());
        b[l.mode_at] = 0;
        for ver in [514i32, 2050, 4098] {
            b[4..8].copy_from_slice(&ver.to_le_bytes());
            assert!(optimize(&b, true).unwrap().is_none());
        }
    }
    #[test]
    fn run_packets_respect_the_declared_boundaries() {
        let packets = [0xfd, 1, 2, 3, 4, 17];
        assert_eq!(
            rle_decode(&packets, 8).unwrap(),
            [1, 2, 3, 17, 17, 17, 17, 17]
        );
        for (packet, len) in [
            (&packets[..], 7),
            (&packets[..], 9),
            (&[17][..], 18),
            (&[0x80, 3][..], 128),
        ] {
            assert!(rle_decode(packet, len).is_err());
        }
    }
    #[test]
    fn damaged_zip_fails_validation() {
        let (b, _) = raw_file(false, false);
        let mut packed = optimize(&b, false).unwrap().unwrap();
        *packed.last_mut().unwrap() ^= 51;
        assert!(optimize(&packed, false).is_err());
    }
}
