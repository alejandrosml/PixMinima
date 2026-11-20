use super::*;

fn tagged_qoi(header_channels: u8, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut q = b"qoif".to_vec();
    q.extend((pixels.len() as u32).to_be_bytes());
    q.extend(1u32.to_be_bytes());
    q.extend([header_channels, 1]);
    for &p in pixels {
        q.push(255);
        q.extend(p);
    }
    q.extend([0, 0, 0, 0, 0, 0, 0, 1]);
    q
}
#[test]
fn qoi_retains_informational_header_and_alpha_hidden_rgb() {
    let pixels: Vec<_> = (0..64)
        .map(|i| {
            if i % 8 == 0 {
                [215, 15, 10, 0]
            } else {
                [12, 13, 14, 255]
            }
        })
        .collect();
    for channels in [3, 4] {
        let input = tagged_qoi(channels, &pixels);
        let smaller = optimize(&input, true).unwrap();
        assert_eq!(detect(&smaller), Some("QOI"));
        assert!(smaller.len() < input.len());
        assert_eq!(smaller[..14], input[..14]);
        assert_eq!(super::qoi::decode(&smaller).unwrap(), pixels.concat());
    }
}
#[test]
fn qoi_checks_runs_trailers_and_extra_chunks() {
    let input = tagged_qoi(4, &[[0, 0, 0, 255]]);
    let mut bad = input[..14].to_vec();
    bad.push(0xc1);
    bad.extend([0, 0, 0, 0, 0, 0, 0, 1]);
    assert!(optimize(&bad, false).is_err());
    let mut trailing = input.clone();
    trailing.push(0);
    assert!(optimize(&trailing, false).is_err());
    let mut extra = input.clone();
    extra.insert(extra.len() - 8, 0);
    assert!(optimize(&extra, false).is_err());
    let mut sequence = input[..14].to_vec();
    sequence[7] = 3;
    sequence.extend([0xc0, 0xff, 10, 20, 30, 255, 53]);
    sequence.extend([0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(
        super::qoi::decode(&sequence).unwrap(),
        [[0, 0, 0, 255], [10, 20, 30, 255], [0, 0, 0, 255]].concat()
    );
}
fn tga_header(kind: u8, depth: u8, w: u16, h: u16) -> Vec<u8> {
    let mut hdr = vec![0; 18];
    hdr[2] = kind;
    hdr[12..14].copy_from_slice(&w.to_le_bytes());
    hdr[14..16].copy_from_slice(&h.to_le_bytes());
    hdr[16] = depth;
    hdr[17] = if depth == 32 { 0x38 } else { 0x30 };
    hdr
}
fn footer() -> Vec<u8> {
    let mut x = vec![0; 8];
    x.extend(b"TRUEVISION-XFILE.\0");
    x
}
#[test]
fn tga_orientation_rgba_identifier_and_footer_survive() {
    let mut b = tga_header(2, 32, 130, 2);
    b[0] = 3;
    b.extend(b"who");
    for row in [42, 89] {
        for _ in 0..130 {
            b.extend([22, row, 7, 0]);
        }
    }
    b.extend(footer());
    let small = optimize(&b, false).unwrap();
    assert!(small.len() < b.len());
    assert_eq!(small[2], 10);
    assert_eq!(&small[3..21], &b[3..21]);
    // Two non-crossing repeat packets cover each row. Both 2+128 and
    // 128+2 are optimal; their order does not affect the stored samples.
    for (packets, y) in small[21..41].as_chunks::<10>().0.iter().zip([42, 89]) {
        assert!(packets[0] >= 128 && packets[5] >= 128);
        assert_eq!(
            usize::from(packets[0] & 127) + usize::from(packets[5] & 127) + 2,
            130
        );
        assert_eq!(&packets[1..5], &[22, y, 7, 0]);
        assert_eq!(&packets[6..10], &[22, y, 7, 0]);
    }
    assert!(small.ends_with(&footer()));
    assert_eq!(optimize(&small, true).unwrap(), small);
}
#[test]
fn tga_palette_origins_and_metadata_offsets() {
    let mut b = tga_header(1, 8, 100, 1);
    b[1] = 1;
    b[3] = 13;
    b[5] = 2;
    b[7] = 24;
    b.extend([0, 128, 10, 15, 16, 17]);
    b.extend([14; 100]);
    assert!(optimize(&b, false).unwrap().len() < b.len());
    let mut invalid = b.clone();
    invalid[24] = 3;
    assert_eq!(detect(&invalid), None);
    let offset = b.len() as u32;
    b.extend([2, 0, 0]);
    let mut f = footer();
    f[..4].copy_from_slice(&offset.to_le_bytes());
    b.extend(f);
    assert_eq!(optimize(&b, true).unwrap(), b);
}
#[test]
fn tga_variants_accept_indexed_16_gray_16_and_truecolor_15_16_24() {
    for (kind, depth) in [(1, 16), (3, 8), (3, 16), (2, 15), (2, 16), (2, 24)] {
        let mut input = tga_header(kind, depth, 10, 6);
        if kind == 1 {
            input[1] = 1;
            input[5] = 2;
            input[7] = 16;
            input.extend([1, 2, 3, 4]);
        }
        input.extend(vec![0u8; usize::from(depth).div_ceil(8) * 60]);
        let optimized = optimize(&input, true).unwrap();
        assert!(optimized.len() < input.len());
        assert_eq!(optimized[2], kind + 8);
        assert_eq!(optimized[16], depth);
    }
}
#[test]
fn pbm_odd_width_resets_each_row_and_retains_header_comment() {
    let b = b"P1\n# original comment\n9 2\n1 0 1 0 1 0 1 0 1\n0 1 0 1 0 1 0 1 0\n";
    let result = optimize(b, true).unwrap();
    assert_eq!(result, b"P4\n# original comment\n9 2\n\xaa\x80\x55\x00");
}
#[test]
fn pgm_16bit_is_big_endian_and_ppm_preserves_channels() {
    let p = b"P2\n4 1\n1023\n0 00015 256 1023\n";
    assert_eq!(
        optimize(p, true).unwrap(),
        b"P5\n4 1\n1023\n\0\0\0\x0f\x01\0\x03\xff"
    );
    let p = b"P3\n2 1\n255\n001 002 003 010 020 030\n";
    assert_eq!(
        optimize(p, true).unwrap(),
        b"P6\n2 1\n255\n\x01\x02\x03\x0a\x14\x1e"
    );
}
#[test]
fn netpbm_preserves_inline_comments_sequences_and_raw_samples() {
    let b = b"P1\n3 1\n1 0 # after sample two\n1\n";
    assert_eq!(optimize(b, true).unwrap(), b);
    let b = b"P5\n1 1\n255\n#P6\n1 1\n255\n\x0a\x20\x23";
    assert_eq!(optimize(b, false).unwrap(), b);
    for b in [
        b"P2\n1 1\n7\n8".as_slice(),
        b"P3\n2 1\n9\n1 1",
        b"P5\n1 1\n7\n\xfe",
    ] {
        assert!(optimize(b, true).is_err());
    }
}
