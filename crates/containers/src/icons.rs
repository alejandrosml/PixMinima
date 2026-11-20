use super::{set32, u16le, u32le};
use std::collections::BTreeMap;

pub(super) fn optimize(input: &[u8], thorough: bool) -> Result<(&'static str, Vec<u8>), String> {
    let kind = if u16le(input, 2)? == 1 { "ICO" } else { "CUR" };
    let count = u16le(input, 4)?;
    let table_end = 6 + 16 * count;
    if count == 0 || count > 2048 || table_end > input.len() {
        return Err(format!("Invalid {kind} directory"));
    }
    let mut images = BTreeMap::<(usize, usize), Vec<usize>>::new();
    for i in 0..count {
        let entry = 6 + 16 * i;
        let size = u32le(input, entry + 8)?;
        let offset = u32le(input, entry + 12)?;
        if size == 0
            || offset < table_end
            || offset.checked_add(size).is_none_or(|end| end > input.len())
        {
            return Err(format!("Invalid {kind} image extent"));
        }
        images.entry((offset, size)).or_default().push(entry);
    }
    let mut out = input[..table_end].to_vec();
    let mut cursor = table_end;
    for ((offset, size), entries) in images {
        if offset < cursor {
            return Err(format!("Overlapping {kind} resources"));
        }
        if input[cursor..offset].iter().any(|b| *b != 0) {
            return Ok((kind, input.to_vec()));
        }
        out.extend_from_slice(&input[cursor..offset]);
        let payload = &input[offset..offset + size];
        let optimized;
        let bytes = if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
            optimized = pixminima_png::optimize_preserving_header(payload, thorough)?;
            if payload.get(16..29) == optimized.get(16..29) && optimized.len() < size {
                &optimized
            } else {
                payload
            }
        } else {
            payload
        };
        let start = out.len();
        out.extend_from_slice(bytes);
        for entry in entries {
            set32(&mut out, entry + 8, bytes.len())?;
            set32(&mut out, entry + 12, start)?;
        }
        cursor = offset + size;
    }
    if cursor != input.len() {
        return Ok((kind, input.to_vec()));
    }
    Ok((kind, out))
}
