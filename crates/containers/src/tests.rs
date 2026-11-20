use super::*;

fn field32(bytes: &mut [u8], at: usize, n: usize) {
    set32(bytes, at, n).unwrap();
}
fn field16(bytes: &mut [u8], at: usize, n: usize) {
    bytes[at..at + 2].copy_from_slice(&(n as u16).to_le_bytes());
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let at = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&out[at..]).to_be_bytes());
}
fn make_png() -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    png_chunk(
        &mut out,
        b"IHDR",
        &[0, 0, 0, 32, 0, 0, 0, 32, 8, 6, 0, 0, 0],
    );
    // Keep a legitimate nontrivial ICC-independent ancillary chunk.
    png_chunk(&mut out, b"tEXt", b"Author\0PixMinima");
    let mut rows = Vec::new();
    for y in 0..32 {
        rows.push(0);
        for x in 0..32 {
            rows.extend_from_slice(&[x, y, 23, x ^ y]);
        }
    }
    png_chunk(
        &mut out,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&rows, 0),
    );
    png_chunk(&mut out, b"IEND", b"");
    out
}

#[test]
fn ico_and_cursor_keep_hotspots_and_shared_image_offsets() {
    let png = make_png();
    for t in [1u8, 2] {
        let mut input = vec![0, 0, t, 0, 2, 0];
        for _ in 0..2 {
            let mut entry = vec![32, 32, 0, 0, 2, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            field32(&mut entry, 8, png.len());
            field32(&mut entry, 12, 38);
            input.extend(entry);
        }
        input.extend(&png);
        let (_, out) = optimize(&input, false).unwrap();
        assert!(out.len() < input.len());
        assert_eq!(&out[..14], &input[..14]);
        assert_eq!(&out[22..30], &input[22..30]);
        assert_eq!(u32le(&out, 18).unwrap(), u32le(&out, 34).unwrap());
        assert_eq!(u32le(&out, 14).unwrap() + 38, out.len());
        assert!(pixminima_png::optimize(&out[38..], false).is_ok());
    }
}

#[test]
fn svgz_keeps_original_xml_and_gzip_fields_and_checks_trailer() {
    let xml = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>{}</text></svg>",
        "anything".repeat(3000)
    );
    let source = xml.as_bytes();
    let mut input = vec![0x1f, 0x8b, 8, 0x1c, 1, 2, 3, 4, 0, 3, 2, 0, 42, 37];
    input.extend(b"original.svg\x00a comment\x00");
    let offset = input.len();
    input.extend(miniz_oxide::deflate::compress_to_vec(source, 0));
    input.extend(crc32(source).to_le_bytes());
    input.extend((source.len() as u32).to_le_bytes());
    let (_, out) = optimize(&input, false).unwrap();
    assert!(out.len() < input.len());
    assert_eq!(&input[..offset], &out[..offset]);
    assert_eq!(&out[out.len() - 8..], &input[input.len() - 8..]);
    assert_eq!(
        inflate_exact(&out[offset..out.len() - 8], source.len()).unwrap(),
        source
    );
    input[offset] ^= 0x13;
    assert!(optimize(&input, false).is_err());
}

fn create_zip(mime: &[u8], content: &[u8]) -> Vec<u8> {
    let mut file = Vec::new();
    let mut directory = Vec::new();
    for (name, data, method) in [("mimetype", mime, 0), ("stack.xml", content, 8)] {
        let encoded = if method == 8 {
            miniz_oxide::deflate::compress_to_vec(data, 0)
        } else {
            data.to_vec()
        };
        let mut local = vec![0; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        field16(&mut local, 4, 20);
        field16(&mut local, 8, method);
        field32(&mut local, 14, crc32(data) as usize);
        field32(&mut local, 18, encoded.len());
        field32(&mut local, 22, data.len());
        field16(&mut local, 26, name.len());
        let mut central = vec![0; 46];
        central[..4].copy_from_slice(b"PK\x01\x02");
        field16(&mut central, 4, 20);
        central[6..30].copy_from_slice(&local[4..28]);
        field32(&mut central, 42, file.len());
        file.extend(local);
        file.extend(name.as_bytes());
        file.extend(encoded);
        directory.extend(central);
        directory.extend(name.as_bytes());
    }
    let mut end = vec![0; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    field16(&mut end, 8, 2);
    field16(&mut end, 10, 2);
    field32(&mut end, 12, directory.len());
    field32(&mut end, 16, file.len());
    field16(&mut end, 20, 5);
    file.extend(directory);
    file.extend(end);
    file.extend(b"hello");
    file
}

#[test]
fn ora_and_kra_preserve_zip_payloads_index_and_comment() {
    for mime in [b"application/x-krita".as_slice(), b"image/openraster"] {
        let source = "<stack>content</stack>".repeat(400);
        let input = create_zip(mime, source.as_bytes());
        let (_, out) = optimize(&input, false).unwrap();
        assert!(out.len() < input.len());
        assert!(out.ends_with(b"hello"));
        let file = 30 + 8 + mime.len();
        let data = file + 30 + 9;
        let size = u32le(&out, file + 18).unwrap();
        assert_eq!(
            inflate_exact(&out[data..data + size], source.len()).unwrap(),
            source.as_bytes()
        );
        assert_eq!(optimize(&out, false).unwrap().1, out);
    }
}

#[test]
fn icns_updates_valid_table_of_contents_and_preserves_opaque_members() {
    fn entry(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = kind.to_vec();
        v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        v.extend_from_slice(body);
        v
    }
    let pixels = entry(b"ic12", &make_png());
    let legacy = entry(b"ICN#", &[1, 3, 5, 7]);
    let mut toc = pixels[..8].to_vec();
    toc.extend_from_slice(&legacy[..8]);
    let mut body = entry(b"TOC ", &toc);
    body.extend(&pixels);
    body.extend(&legacy);
    let source = entry(b"icns", &body);
    let (_, optimized) = optimize(&source, false).unwrap();
    assert!(optimized.len() < source.len());
    assert!(optimized.ends_with(&legacy));
    assert_eq!(&optimized[16..24], &optimized[32..40]);
    let mut opaque = body;
    opaque.extend(entry(b"uuid", b"references"));
    let orig = entry(b"icns", &opaque);
    assert_eq!(optimize(&orig, true).unwrap().1, orig);
}
