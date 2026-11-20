use super::{crc32, inflate_exact, set32, smaller_deflate, u16le, u32le};
use std::collections::BTreeSet;

const CENTRAL: &[u8] = b"PK\x01\x02";
const LOCAL: &[u8] = b"PK\x03\x04";
const END: &[u8] = b"PK\x05\x06";

fn end_record(input: &[u8]) -> Option<usize> {
    let last = input.len().checked_sub(22)?;
    (last.saturating_sub(65535)..=last)
        .rev()
        .find(|&i| input[i..].starts_with(END) && u16le(input, i + 20).ok() == Some(last - i))
}

fn known_extra(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let (Ok(id), Ok(n)) = (u16le(bytes, 0), u16le(bytes, 2)) else {
            return false;
        };
        if !matches!(id, 0x5455 | 0x000a | 0x7875 | 0x7855) {
            return false;
        }
        let Some(rest) = bytes.get(4 + n..) else {
            return false;
        };
        bytes = rest;
    }
    true
}

struct Entry {
    central: std::ops::Range<usize>,
    header: std::ops::Range<usize>,
    data: std::ops::Range<usize>,
    size: usize,
    method: usize,
    crc: u32,
}

/// Archive member bytes, ordering, names, method, flags, timestamps, comments
/// and attributes are preserved. Only a strictly smaller DEFLATE stream wins.
pub(super) fn optimize(input: &[u8], thorough: bool) -> Result<(&'static str, Vec<u8>), String> {
    let e = end_record(input).ok_or("Invalid ZIP end record")?;
    let count = u16le(input, e + 10)?;
    let central_start = u32le(input, e + 16)?;
    if count == 0
        || count > 16_384
        || u16le(input, e + 4)? != 0
        || u16le(input, e + 6)? != 0
        || u16le(input, e + 8)? != count
        || central_start.checked_add(u32le(input, e + 12)?) != Some(e)
    {
        return Err("Unsupported multi-disk, oversized or extended ZIP directory".into());
    }
    let mut entries = Vec::with_capacity(count);
    let mut names = BTreeSet::new();
    let mut kind = None;
    let mut keep_layout = false;
    let mut pos = central_start;
    for _ in 0..count {
        if !input.get(pos..).is_some_and(|s| s.starts_with(CENTRAL)) {
            return Err("Invalid ZIP directory signature".into());
        }
        let cs = u32le(input, pos + 20)?;
        let size = u32le(input, pos + 24)?;
        let n = u16le(input, pos + 28)?;
        let x = u16le(input, pos + 30)?;
        let c = u16le(input, pos + 32)?;
        let off = u32le(input, pos + 42)?;
        let next = pos + 46 + n + x + c;
        if next > e || !input.get(off..).is_some_and(|s| s.starts_with(LOCAL)) {
            return Err("Invalid ZIP directory entry".into());
        }
        let name = &input[pos + 46..pos + 46 + n];
        if !names.insert(name) {
            return Err("Ambiguous duplicate ZIP entry".into());
        }
        if name.starts_with(b"META-INF/")
            && (name.ends_with(b".SF")
                || name.ends_with(b".RSA")
                || name.ends_with(b".DSA")
                || name.ends_with(b"signatures.xml"))
        {
            keep_layout = true;
        }
        let flags = u16le(input, pos + 8)?;
        let method = u16le(input, pos + 10)?;
        let ln = u16le(input, off + 26)?;
        let lx = u16le(input, off + 28)?;
        let start = off + 30 + ln + lx;
        let stop = start.checked_add(cs).ok_or("ZIP member size overflow")?;
        if stop > central_start
            || u16le(input, pos + 34)? != 0
            || u16le(input, off + 6)? != flags
            || u16le(input, off + 8)? != method
            || input.get(off + 30..off + 30 + ln) != Some(name)
        {
            return Err("Conflicting ZIP headers".into());
        }
        if flags & !0x0806 != 0 || !matches!(method, 0 | 8) {
            keep_layout = true;
        }
        if flags & 8 == 0
            && (cs != u32le(input, off + 18)?
                || size != u32le(input, off + 22)?
                || u32le(input, pos + 16)? != u32le(input, off + 14)?)
        {
            return Err("Conflicting ZIP sizes or checksums".into());
        }
        if !known_extra(&input[pos + 46 + n..pos + 46 + n + x])
            || !known_extra(&input[off + 30 + ln..start])
        {
            keep_layout = true;
        }
        if name == b"mimetype" && method == 0 && cs == size && flags & 1 == 0 {
            kind = match &input[start..stop] {
                b"image/openraster" => Some("ORA"),
                b"application/x-krita" => Some("KRA"),
                _ => None,
            };
        }
        entries.push(Entry {
            central: pos..next,
            header: off..start,
            data: start..stop,
            size,
            method,
            crc: u32le(input, pos + 16)? as u32,
        });
        pos = next;
    }
    let kind = kind.ok_or("ZIP input is not a recognized ORA or KRA image")?;
    if pos != e {
        return Err("Invalid ZIP central directory size".into());
    }
    if keep_layout {
        return Ok((kind, input.to_vec()));
    }
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by_key(|&i| entries[i].header.start);
    let mut cursor = 0;
    let mut total = 0usize;
    for &i in &order {
        let entry = &entries[i];
        if entry.header.start != cursor {
            return Ok((kind, input.to_vec()));
        }
        cursor = entry.data.end;
        total = total
            .checked_add(entry.size)
            .ok_or("Expanded archive size overflow")?;
    }
    if cursor != central_start || total > 256 * 1024 * 1024 {
        return Ok((kind, input.to_vec()));
    }
    let mut out = Vec::with_capacity(input.len());
    let mut locations = vec![(0, 0); count];
    for i in order {
        let entry = &entries[i];
        let payload = &input[entry.data.clone()];
        let replacement;
        let best = if entry.method == 8 {
            if entry.size > 96 * 1024 * 1024 {
                return Ok((kind, input.to_vec()));
            }
            let source = inflate_exact(payload, entry.size)?;
            if crc32(&source) != entry.crc {
                return Err("Invalid ZIP member checksum".into());
            }
            replacement = smaller_deflate(&source, payload, thorough)?;
            replacement.as_slice()
        } else {
            if entry.size != payload.len() || crc32(payload) != entry.crc {
                return Err("Invalid stored ZIP member".into());
            }
            payload
        };
        let start = out.len();
        out.extend_from_slice(&input[entry.header.clone()]);
        set32(&mut out, start + 18, best.len())?;
        out.extend_from_slice(best);
        locations[i] = (start, best.len());
    }
    let directory = out.len();
    for (entry, (start, size)) in entries.iter().zip(locations) {
        let p = out.len();
        out.extend_from_slice(&input[entry.central.clone()]);
        set32(&mut out, p + 20, size)?;
        set32(&mut out, p + 42, start)?;
    }
    let p = out.len();
    out.extend_from_slice(&input[e..]);
    set32(&mut out, p + 16, directory)?;
    Ok((kind, out))
}
