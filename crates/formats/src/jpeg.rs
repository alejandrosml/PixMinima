use super::{MAX_PIXELS, take};

#[derive(Default)]
struct Frame<'a> {
    metadata: Vec<(u8, &'a [u8])>,
    quantization: Vec<&'a [u8]>,
    layout: Option<&'a [u8]>,
    keep_original: bool,
    tables: Vec<(std::ops::Range<usize>, &'a [u8])>,
}

// Follow entropy-coded scans instead of looking for a raw FF D9. EXIF often
// contains a complete embedded thumbnail and its own EOI.
fn inspect(data: &[u8]) -> Result<Frame<'_>, String> {
    let mut f = Frame::default();
    let mut p = 2;
    let mut entropy = false;
    let mut scans = 0;
    let mut segments = 0;
    loop {
        if entropy {
            let offset = data
                .get(p..)
                .and_then(|s| s.iter().position(|&b| b == 0xff))
                .ok_or("JPEG scan has no end marker")?;
            p += offset;
        }
        let marker_start = p;
        if take(data, &mut p, 1)?[0] != 0xff {
            return Err("Invalid JPEG marker".into());
        }
        while data.get(p) == Some(&0xff) {
            p += 1;
        }
        let marker = take(data, &mut p, 1)?[0];
        if entropy && (marker == 0 || (0xd0..=0xd7).contains(&marker)) {
            continue;
        }
        entropy = false;
        segments += 1;
        if segments > 65_536 {
            return Err("Too many JPEG segments".into());
        }
        if marker == 0xd9 {
            if scans == 0 || f.layout.is_none() {
                return Err("JPEG has no image scan".into());
            }
            // MPO, gain maps and Motion Photos can append additional objects.
            f.keep_original |= p != data.len();
            return Ok(f);
        }
        if matches!(marker, 0 | 1 | 0xd0..=0xd8) {
            return Err("Unexpected JPEG marker".into());
        }
        let n = take(data, &mut p, 2)?;
        let size = usize::from(u16::from_be_bytes([n[0], n[1]]))
            .checked_sub(2)
            .ok_or("Invalid JPEG segment length")?;
        let payload = take(data, &mut p, size)?;
        match marker {
            0xda => {
                entropy = true;
                scans += 1;
            }
            0xdb => f.quantization.push(payload),
            0xc0..=0xc2 => {
                if f.layout.replace(payload).is_some() {
                    return Err("Multiple JPEG frame headers".into());
                }
            }
            // Other coding modes are valid, but not supported by the coefficient codec.
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                f.layout = Some(payload);
                f.keep_original = true;
            }
            0xe0..=0xef | 0xfe => {
                f.metadata.push((marker, payload));
                // These common self-contained formats do not refer to other top-level objects.
                // Leave unknown vendor segments and signed/compound containers alone.
                let ordinary = match marker {
                    0xe0 => payload.starts_with(b"JFIF\0") || payload.starts_with(b"JFXX\0"),
                    0xe1 => {
                        payload.starts_with(b"Exif\0\0")
                            || (payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0")
                                && ![
                                    b"MotionPhoto".as_slice(),
                                    b"MicroVideo",
                                    b"hdr-gain-map",
                                    b"Container:Directory",
                                    b"C2PA",
                                ]
                                .iter()
                                .any(|word| payload.windows(word.len()).any(|s| s == *word)))
                    }
                    0xe2 => payload.starts_with(b"ICC_PROFILE\0"),
                    0xec => adobe_ducky(payload),
                    0xee => payload.starts_with(b"Adobe"),
                    0xfe => true,
                    _ => false,
                };
                f.keep_original |= !ordinary;
            }
            0xc4 => f.tables.push((marker_start..p, payload)),
            0xdd => {}
            _ => f.keep_original = true,
        }
    }
}

// Photoshop's Save-for-Web APP12 stores self-contained quality, comments,
// and copyright records, each prefixed by its byte length.
fn adobe_ducky(payload: &[u8]) -> bool {
    let Some(mut rest) = payload.strip_prefix(b"Ducky") else {
        return false;
    };
    while rest.len() >= 2 {
        let tag = u16::from_be_bytes([rest[0], rest[1]]);
        rest = &rest[2..];
        if tag == 0 {
            return rest.is_empty();
        }
        if !matches!(tag, 1..=3) || rest.len() < 2 {
            return false;
        }
        let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
        let Some(tail) = rest.get(2 + len..) else {
            return false;
        };
        rest = tail;
    }
    false
}

fn emit_tables(out: &mut Vec<u8>, payload: &mut Vec<u8>) {
    if payload.is_empty() {
        return;
    }
    out.extend([0xff, 0xc4]);
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.append(payload);
}

// The entropy bits stay byte-identical. JPEG allows multiple tables per DHT
// and keeps table slots alive across scans until the next definition.
fn compact_tables(data: &[u8], frame: &Frame<'_>) -> Result<Vec<u8>, String> {
    let mut active: [Option<&[u8]>; 8] = [None; 8];
    let mut out = Vec::with_capacity(data.len());
    let mut pending = Vec::new();
    let mut cursor = 0;
    for (range, mut tables) in frame.tables.iter().map(|(r, t)| (r, *t)) {
        if cursor != range.start {
            emit_tables(&mut out, &mut pending);
            out.extend_from_slice(&data[cursor..range.start]);
        }
        while !tables.is_empty() {
            let header = tables.get(..17).ok_or("Incomplete JPEG Huffman table")?;
            let class = header[0] >> 4;
            let id = header[0] & 15;
            if class > 1 || id > 3 {
                return Err("Unsupported JPEG Huffman table ID".into());
            }
            let len = 17 + header[1..].iter().map(|&b| usize::from(b)).sum::<usize>();
            let table = tables.get(..len).ok_or("Truncated JPEG Huffman alphabet")?;
            let slot = &mut active[usize::from(class * 4 + id)];
            if *slot != Some(table) {
                if pending.len() + len > 65533 {
                    emit_tables(&mut out, &mut pending);
                }
                pending.extend_from_slice(table);
                *slot = Some(table);
            }
            tables = &tables[len..];
        }
        cursor = range.end;
    }
    emit_tables(&mut out, &mut pending);
    out.extend_from_slice(&data[cursor..]);
    Ok(out)
}

fn decode(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = jpeg_decoder::Decoder::new(data);
    decoder.set_max_decoding_buffer_size(MAX_PIXELS * 4);
    decoder
        .decode()
        .map_err(|e| format!("JPEG verification: {e}"))
}

pub(crate) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    encode(
        data,
        if thorough {
            smallpeg::optimize_jpeg_with
        } else {
            smallpeg::optimize_jpeg_baseline_with
        },
    )
}

pub(crate) fn optimize_maximum(data: &[u8]) -> Result<Vec<u8>, String> {
    let ordinary = optimize(data, true)?;
    if let Ok(candidate) = encode(data, smallpeg::optimize_jpeg_maximum_with)
        && candidate.len() < ordinary.len()
    {
        return Ok(candidate);
    }
    Ok(ordinary)
}

fn encode(
    data: &[u8],
    optimizer: fn(&[u8], &smallpeg::Limits) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let before = inspect(data)?;
    if before.keep_original {
        return Ok(data.to_vec());
    }
    let Some(info) = smallpeg::jpeg_info(data) else {
        return Ok(data.to_vec());
    };
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS as u64 {
        return Err("JPEG exceeds pixel limit".into());
    }
    let limits = smallpeg::Limits::default()
        .max_pixels(MAX_PIXELS as u64)
        .max_scans(96);
    let Some(proposed) = optimizer(data, &limits) else {
        return Ok(data.to_vec());
    };
    let mut candidate = compact_tables(data, &before)?;
    let alternative = compact_tables(&proposed, &inspect(&proposed)?)?;
    if alternative.len() < candidate.len() {
        candidate = alternative;
    }
    if candidate.len() >= data.len() {
        return Ok(data.to_vec());
    }
    let after = inspect(&candidate)?;
    if before.metadata != after.metadata
        || before.quantization != after.quantization
        || before.layout != after.layout
    {
        return Err("JPEG metadata or quantization verification failed".into());
    }
    // The emitter works on unmodified quantized DCT coefficients. Verify its
    // output as pixels with a separate decoder compiled in portable, safe Rust.
    if !smallpeg::same_coefficients(data, &candidate, &limits) {
        return Err("JPEG quantized coefficients changed".into());
    }
    if decode(data)? != decode(&candidate)? {
        return Err("JPEG pixels changed".into());
    }
    Ok(candidate)
}

pub(crate) fn optimize_lossy(data: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let before = inspect(data)?;
    if before.keep_original {
        return Ok(None);
    }
    let Some(info) = smallpeg::jpeg_info(data) else {
        return Ok(None);
    };
    if !matches!(info.components, 1 | 3)
        || u64::from(info.width) * u64::from(info.height) > MAX_PIXELS as u64
    {
        return Ok(None);
    }
    let limits = smallpeg::Limits::default()
        .max_pixels(MAX_PIXELS as u64)
        .max_scans(96);
    let Some(proposed) = smallpeg::requantize_jpeg_with(data, &limits, thorough) else {
        return Ok(None);
    };
    let encoded = inspect(&proposed)?;
    if before.metadata != encoded.metadata || before.layout != encoded.layout {
        return Ok(None);
    }
    let compact = compact_tables(&proposed, &encoded)?;
    if compact.len() >= data.len() {
        return Ok(None);
    }
    let a = decode(data)?;
    let b = decode(&compact)?;
    if !super::quality::close(&a, &b, info.width as usize, usize::from(info.components)) {
        return Ok(None);
    }
    Ok(Some(compact))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn striped_jpeg() -> Vec<u8> {
        include_bytes!("../tests/fixtures/striped.jpg").to_vec()
    }

    #[test]
    fn huffman_tables_repacked_and_comment_retained() {
        let image = striped_jpeg();
        let first = inspect(&image).unwrap().tables[0].0.clone();
        let mut input = image[..first.start].to_vec();
        input.extend_from_slice(b"\xff\xfe\x00\x09comment");
        for _ in 0..8 {
            input.extend_from_slice(&image[first.clone()]);
        }
        input.extend_from_slice(&image[first.start..]);
        let result = optimize(&input, true).unwrap();
        assert!(result.len() < input.len());
        assert_eq!(decode(&result).unwrap(), decode(&input).unwrap());
        assert_eq!(
            inspect(&result).unwrap().metadata,
            inspect(&input).unwrap().metadata
        );
    }
    #[test]
    fn progressive_partition_preserves_color_coefficients_and_samples() {
        let source = include_bytes!("../tests/fixtures/progressive-search.jpg");
        let deep = optimize(source, true).unwrap();
        let fast = optimize(source, false).unwrap();
        assert!(deep.len() < fast.len());
        assert!(smallpeg::same_coefficients(
            source,
            &deep,
            &smallpeg::Limits::default()
        ));
        assert_eq!(decode(source).unwrap(), decode(&deep).unwrap());
        let maximum = optimize_maximum(source).unwrap();
        assert!(maximum.len() <= deep.len());
        assert!(smallpeg::same_coefficients(
            source,
            &maximum,
            &smallpeg::Limits::default()
        ));
        assert_eq!(decode(source).unwrap(), decode(&maximum).unwrap());
    }

    #[test]
    fn pictures_with_attachments_are_ignored() {
        let mut image = striped_jpeg();
        image.extend_from_slice(b"attached second object");
        assert_eq!(optimize(&image, true).unwrap(), image);
        let image = striped_jpeg();
        let mut private = image[..2].to_vec();
        private.extend_from_slice(b"\xff\xe2\x00\x06MPF\0");
        private.extend(&image[2..]);
        assert_eq!(optimize(&private, true).unwrap(), private);
        let mut signed = image[..2].to_vec();
        signed.extend_from_slice(b"\xff\xeb\x00\x06JUMB");
        signed.extend(&image[2..]);
        assert_eq!(optimize(&signed, true).unwrap(), signed);
    }

    #[test]
    fn lossy_requantization_keeps_markers_and_is_idempotent() {
        let jpeg = include_bytes!("../tests/fixtures/requantize.jpg");
        let mut source = jpeg[..2].to_vec();
        source.extend_from_slice(b"\xff\xfe\x00\x09comment");
        source.extend_from_slice(&jpeg[2..]);
        let candidate = optimize_lossy(&source, true).unwrap().unwrap();
        assert!(candidate.len() < source.len());
        assert_eq!(
            inspect(&source).unwrap().metadata,
            inspect(&candidate).unwrap().metadata
        );
        let a = decode(&source).unwrap();
        let b = decode(&candidate).unwrap();
        assert_ne!(a, b);
        assert!(super::super::quality::close(&a, &b, 96, 3));
        assert!(optimize_lossy(&candidate, true).unwrap().is_none());
        assert!(smallpeg::same_coefficients(
            &source,
            &optimize(&source, true).unwrap(),
            &smallpeg::Limits::default()
        ));
        source.extend_from_slice(b"opaque companion data");
        assert!(optimize_lossy(&source, false).unwrap().is_none());
    }
}
