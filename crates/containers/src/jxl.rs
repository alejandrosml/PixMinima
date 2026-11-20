//! Lossless container changes. The JPEG XL codestream itself stays byte-identical.
use super::bmff::{atom, boxes};
use std::io::Read;

const MAGIC: &[u8] = b"\0\0\0\x0cJXL \x0d\x0a\x87\x0a";
const STANDARD: &[u8] = b"jxl \0\0\0\0jxl ";

pub(super) fn detect(data: &[u8]) -> bool {
    data.starts_with(MAGIC) || data.starts_with(&[0xff, 0x0a])
}

fn compress_metadata(kind: [u8; 4], data: &[u8], thorough: bool) -> Option<Vec<u8>> {
    // Bound workspace use for optional content compression.
    if !(32..=4 * 1024 * 1024).contains(&data.len()) {
        return None;
    }
    let options = brotli::enc::BrotliEncoderParams {
        quality: if thorough { 10 } else { 5 },
        lgwin: 20,
        size_hint: data.len(),
        ..Default::default()
    };
    let mut payload = kind.to_vec();
    brotli::BrotliCompress(&mut &data[..], &mut payload, &options).ok()?;
    if payload.len() >= data.len() {
        return None;
    }
    let mut decoded = Vec::new();
    brotli::Decompressor::new(&payload[4..], 16 * 1024)
        .take(data.len() as u64 + 1)
        .read_to_end(&mut decoded)
        .ok()?;
    (decoded == data).then(|| atom(*b"brob", &payload))
}

pub(super) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    if data.starts_with(&[0xff, 0x0a]) {
        return Ok(data.to_vec());
    }
    let parts = boxes(data, 0..data.len()).ok_or("Invalid JPEG XL box boundaries")?;
    if parts.len() < 3 || parts[1].kind != *b"ftyp" || data[parts[1].payload.clone()] != *STANDARD {
        return Ok(data.to_vec());
    }
    // Unknown boxes, frame indexes, provenance and compressed opaque metadata can
    // carry file offsets or whole-file signatures. Preserve their complete layout.
    if parts[2..].iter().any(|b| {
        !matches!(
            &b.kind,
            b"jxlc" | b"jxlp" | b"jbrd" | b"Exif" | b"xml " | b"jxll"
        )
    }) {
        return Ok(data.to_vec());
    }
    for p in &parts[2..] {
        let body = &data[p.payload.clone()];
        if p.kind == *b"xml "
            && [b"xmldsig".as_slice(), b"c2pa", b"xpacket end=\"r\""]
                .iter()
                .any(|needle| {
                    body.windows(needle.len())
                        .any(|w| w.eq_ignore_ascii_case(needle))
                })
        {
            return Ok(data.to_vec());
        }
        if p.kind == *b"Exif"
            && body
                .windows(2)
                .any(|w| w == [0x92, 0x7c] || w == [0x7c, 0x92])
        {
            return Ok(data.to_vec());
        }
    }
    let indices: Vec<_> = parts
        .iter()
        .enumerate()
        .filter_map(|(i, b)| matches!(&b.kind, b"jxlp" | b"jxlc").then_some(i))
        .collect();
    let first = *indices.first().ok_or("Missing JPEG XL codestream")?;
    let last = *indices.last().unwrap();
    // Metadata interleaved with fragments may be intentionally placed for streaming.
    if indices.len() != last - first + 1 {
        return Ok(data.to_vec());
    }
    let mut stream = Vec::new();
    for (number, &i) in indices.iter().enumerate() {
        let b = &parts[i];
        let body = &data[b.payload.clone()];
        if b.kind == *b"jxlc" {
            if indices.len() != 1 {
                return Err("Mixed JPEG XL codestream boxes".into());
            }
            stream.extend_from_slice(body);
        } else {
            let id = u32::from_be_bytes(
                body.get(..4)
                    .ok_or("Invalid JPEG XL fragment")?
                    .try_into()
                    .unwrap(),
            );
            if id & 0x7fff_ffff != number as u32 || (id >> 31 == 1) != (i == last) {
                return Err("Invalid JPEG XL fragment sequence".into());
            }
            stream.extend_from_slice(&body[4..]);
        }
    }
    if !stream.starts_with(&[0xff, 0x0a]) {
        return Err("Invalid JPEG XL codestream signature".into());
    }
    if indices.len() + 2 == parts.len() {
        return Ok(stream);
    }
    let mut out = data[..parts[1].whole.end].to_vec();
    for (i, b) in parts.iter().enumerate().skip(2) {
        if i == first {
            out.extend_from_slice(&atom(*b"jxlc", &stream));
        } else if i <= last && i > first {
            continue;
        } else if matches!(&b.kind, b"xml " | b"Exif")
            && let Some(smaller) = compress_metadata(b.kind, &data[b.payload.clone()], thorough)
        {
            out.extend_from_slice(&smaller);
        } else {
            out.extend_from_slice(&data[b.whole.clone()]);
        }
    }
    Ok(if out.len() < data.len() {
        out
    } else {
        data.to_vec()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjacent_fragments_preserve_codestream_and_metadata_and_opaque_boxes() {
        let mut data = MAGIC.to_vec();
        data.extend(atom(*b"ftyp", STANDARD));
        let mut first = 0u32.to_be_bytes().to_vec();
        first.extend([0xff, 0x0a, 14]);
        let mut second = (1u32 << 31 | 1).to_be_bytes().to_vec();
        second.extend([15, 16]);
        data.extend(atom(*b"jxlp", &first));
        data.extend(atom(*b"jxlp", &second));
        assert_eq!(optimize(&data, false).unwrap(), [0xff, 0x0a, 14, 15, 16]);
        let xml = b"<x>caption </x>".repeat(128);
        data.extend(atom(*b"xml ", &xml));
        let optimized = optimize(&data, false).unwrap();
        let parsed = boxes(&optimized, 0..optimized.len()).unwrap();
        assert_eq!(parsed[3].kind, *b"brob");
        assert_eq!(
            &optimized[parsed[2].payload.clone()],
            [0xff, 0x0a, 14, 15, 16]
        );
        data.extend(atom(*b"jumb", b"signed"));
        assert_eq!(optimize(&data, true).unwrap(), data);
    }
}
