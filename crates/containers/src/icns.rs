//! Apple icon family members retain their order, type, dimensions and color semantics.
use std::collections::BTreeMap;

pub(super) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    fn size(header: &[u8]) -> Option<usize> {
        Some(u32::from_be_bytes(header.get(4..8)?.try_into().ok()?) as usize)
    }
    if size(data) != Some(data.len()) {
        return Err("Invalid ICNS file length".into());
    }
    let mut entries = Vec::new();
    let mut pos = 8;
    while pos < data.len() {
        let header = data.get(pos..pos + 8).ok_or("Truncated ICNS entry")?;
        let kind: [u8; 4] = header[..4].try_into().unwrap();
        let n = size(header).unwrap();
        if n < 8 || n > data.len() - pos || entries.len() >= 4096 {
            return Err("Invalid ICNS entry size or count".into());
        }
        if !matches!(
            &kind,
            b"TOC "
                | b"info"
                | b"ic04"
                | b"ic05"
                | b"ic06"
                | b"ic07"
                | b"ic08"
                | b"ic09"
                | b"ic10"
                | b"ic11"
                | b"ic12"
                | b"ic13"
                | b"ic14"
                | b"icp4"
                | b"icp5"
                | b"icp6"
                | b"ICON"
                | b"ICN#"
                | b"icm#"
                | b"icm4"
                | b"icm8"
                | b"ics#"
                | b"ics4"
                | b"ics8"
                | b"icl4"
                | b"icl8"
                | b"ich#"
                | b"ich4"
                | b"ich8"
                | b"is32"
                | b"il32"
                | b"ih32"
                | b"it32"
                | b"s8mk"
                | b"l8mk"
                | b"h8mk"
                | b"t8mk"
                | b"icnV"
        ) {
            return Ok(data.to_vec());
        }
        entries.push((kind, pos..pos + n));
        pos += n;
    }
    let mut original_toc = Vec::new();
    let mut new_toc = Vec::new();
    let mut cache = BTreeMap::<&[u8], Vec<u8>>::new();
    let mut out = data[..8].to_vec();
    let mut toc = None;
    for (kind, range) in entries {
        let payload = &data[range.start + 8..range.end];
        if kind == *b"TOC " {
            if toc.is_some() {
                return Ok(data.to_vec());
            }
            let start = out.len();
            out.extend_from_slice(&data[range.clone()]);
            toc = Some((start + 8..out.len(), payload));
            continue;
        }
        let pixels = if matches!(
            &kind,
            b"ic04"
                | b"ic05"
                | b"ic06"
                | b"ic07"
                | b"ic08"
                | b"ic09"
                | b"ic10"
                | b"ic11"
                | b"ic12"
                | b"ic13"
                | b"ic14"
                | b"icp4"
                | b"icp5"
                | b"icp6"
        ) && payload.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            use std::collections::btree_map::Entry;
            let value = match cache.entry(payload) {
                Entry::Occupied(hit) => hit.into_mut(),
                Entry::Vacant(slot) => slot.insert(pixminima_png::optimize_preserving_header(
                    payload, thorough,
                )?),
            };
            if value.len() < payload.len() && value.get(16..29) == payload.get(16..29) {
                value.as_slice()
            } else {
                payload
            }
        } else {
            payload
        };
        let start = out.len();
        out.extend_from_slice(&kind);
        out.extend_from_slice(&((pixels.len() + 8) as u32).to_be_bytes());
        original_toc.extend_from_slice(&data[range.start..range.start + 8]);
        new_toc.extend_from_slice(&out[start..]);
        out.extend_from_slice(pixels);
    }
    if let Some((at, original)) = toc {
        if original != original_toc {
            return Ok(data.to_vec());
        }
        out[at].copy_from_slice(&new_toc);
    }
    let size = (out.len() as u32).to_be_bytes();
    out[4..8].copy_from_slice(&size);
    Ok(out)
}
