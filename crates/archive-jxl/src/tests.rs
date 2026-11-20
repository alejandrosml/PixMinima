use super::limits::{check_container, check_jpeg};
use super::{archive_jpeg, restore_jpeg};

fn segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xff, marker]);
    out.extend_from_slice(&(2 + payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

fn gray() -> Vec<u8> {
    let mut jpeg = vec![0xff, 0xd8];
    segment(&mut jpeg, 0xfe, b"PixMinima test, zero-byte follows\0");
    let mut qt = [1; 65];
    qt[0] = 0;
    segment(&mut jpeg, 0xdb, &qt);
    segment(&mut jpeg, 0xc0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]);
    for kind in [0, 16] {
        let mut huff = [0; 18];
        huff[0] = kind;
        huff[1] = 1;
        segment(&mut jpeg, 0xc4, &huff);
    }
    segment(&mut jpeg, 0xda, &[1, 1, 0, 0, 63, 0]);
    jpeg.extend_from_slice(&[0x3f, 0xff, 0xd9]);
    jpeg
}

#[test]
fn reconstructs_entire_jpeg_with_comment() {
    let original = gray();
    let jxl = archive_jpeg(&original).unwrap();
    assert!(jxl.starts_with(b"\0\0\0\x0cJXL "));
    assert!(jxl.windows(4).any(|tag| tag == b"jbrd"));
    assert_eq!(restore_jpeg(&jxl).unwrap(), original);
}

#[test]
fn rejects_non_archives_truncations_and_duplicate_streams() {
    assert!(restore_jpeg(&[0xff, 0x0a, 0]).is_err());
    assert!(restore_jpeg(b"file.jxl").is_err());
    let valid = archive_jpeg(&gray()).unwrap();
    for cut in [12, 21, 32, valid.len() - 1] {
        assert!(restore_jpeg(&valid[..cut]).is_err());
    }
    let mut twice = valid;
    twice.extend_from_slice(b"\0\0\0\x0ajxlc\xff\x0a");
    assert!(check_container(&twice).is_err());
}

#[test]
fn rejects_compound_signed_and_oversized_jpegs() {
    let jpeg = gray();
    let mut multi = jpeg.clone();
    multi.extend(&jpeg);
    assert!(check_jpeg(&multi).is_err());
    for (marker, tag) in [(0xe2, b"MPF\0".as_slice()), (0xeb, b"JP".as_slice())] {
        let mut special = jpeg[..2].to_vec();
        segment(&mut special, marker, tag);
        special.extend(&jpeg[2..]);
        assert!(check_jpeg(&special).is_err());
    }
    let mut huge = jpeg;
    let sof = huge.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
    huge[sof + 5..sof + 9].fill(0xff);
    assert!(check_jpeg(&huge).is_err());
}

#[test]
fn checks_brotli_output_against_declared_limit() {
    // An RFC 7932 compressed stream holding the single byte "X".
    let stream = [0x0b, 0x00, 0x80, 0x58, 0x03];
    assert_eq!(super::limits::brotli_size(&stream, 1).unwrap(), 1);
    assert!(super::limits::brotli_size(&stream, 0).is_err());
}

#[test]
fn reads_reference_encoder_and_archives_progressive_color() {
    let jpeg = include_bytes!("fixtures/progressive.jpg");
    let external = include_bytes!("fixtures/libjxl.jxl");
    assert_eq!(restore_jpeg(external).unwrap(), jpeg);
    let encoded = archive_jpeg(jpeg).unwrap();
    assert_eq!(restore_jpeg(&encoded).unwrap(), jpeg);
}

#[test]
fn distinguishes_regular_jxl_images_from_jpeg_archives() {
    let valid = archive_jpeg(&gray()).unwrap();
    let mut p = 0;
    let mut regular = Vec::new();
    while p < valid.len() {
        let size = u32::from_be_bytes(valid[p..p + 4].try_into().unwrap()) as usize;
        if &valid[p + 4..p + 8] != b"jbrd" {
            regular.extend_from_slice(&valid[p..p + size]);
        }
        p += size;
    }
    assert!(restore_jpeg(&regular).is_err());
}
