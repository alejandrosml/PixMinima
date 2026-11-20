use super::*;
use crate::format::{Header, MAGIC, write_chunk};

fn header(w: u32, h: u32, depth: u8, color: u8, interlace: u8) -> Vec<u8> {
    let mut b = w.to_be_bytes().to_vec();
    b.extend(h.to_be_bytes());
    b.extend([depth, color, 0, 0, interlace]);
    b
}
fn png(h: &[u8], before: &[([u8; 4], Vec<u8>)], rows: &[u8]) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    write_chunk(&mut b, b"IHDR", &[], h);
    for (k, d) in before {
        write_chunk(&mut b, k, &[], d);
    }
    let z = compress_to_vec_zlib(rows, 0);
    for part in z.chunks(11) {
        write_chunk(&mut b, b"IDAT", &[], part);
    }
    write_chunk(&mut b, b"IEND", &[], &[]);
    b
}
fn framectl(seq: u32, w: u32, h: u32, x: u32, y: u32) -> Vec<u8> {
    let mut d = Vec::new();
    for u in [seq, w, h, x, y] {
        d.extend(u.to_be_bytes());
    }
    d.extend([0, 1, 0, 10, 1, 1]);
    d
}
fn parse_samples(b: &[u8]) -> (Header, Vec<u8>) {
    let image = Image::parse(b).unwrap();
    let stream = &image.streams[0];
    let r = inflate(&image.compressed(stream), stream.header.passes().unwrap().1).unwrap();
    let s = Samples::decode(stream.header, &r, image.palette).unwrap();
    (stream.header, s.bytes)
}

#[test]
fn crc_correct_for_signature_chunk() {
    assert_eq!(format::crc32(b"IEND"), 0xae426082);
}
#[test]
fn all_filters_roundtrip_adam7_and_native_depths() {
    for interlaced in [false, true] {
        for (color, depth) in [
            (0, 1),
            (0, 2),
            (0, 4),
            (0, 8),
            (0, 16),
            (2, 8),
            (2, 16),
            (4, 8),
            (4, 16),
            (6, 8),
            (6, 16),
        ] {
            let h = Header {
                width: 19,
                height: 17,
                color,
                depth,
                interlaced,
            };
            let (passes, n) = h.passes().unwrap();
            let mut original = Vec::with_capacity(n);
            for pass in &passes {
                for y in 0..pass.rows {
                    original.push(0);
                    original.extend((0..pass.bytes).map(|i| (i * 239 + y * 43) as u8));
                }
            }
            let samples = Samples::decode(h, &original, None).unwrap();
            for s in [
                Strategy::Fixed(0),
                Strategy::Fixed(1),
                Strategy::Fixed(2),
                Strategy::Fixed(3),
                Strategy::Fixed(4),
                Strategy::MinSum,
                Strategy::Runs,
                Strategy::Entropy,
                Strategy::PairEntropy,
            ] {
                let trial = samples.filtered(s);
                assert_eq!(
                    Samples::decode(h, &trial, None).unwrap().bytes,
                    samples.bytes
                );
            }
        }
    }
}
#[test]
fn preserves_metadata_hidden_rgb_and_16bit_samples() {
    let h = header(7, 3, 16, 6, 1);
    let layout = Header {
        width: 7,
        height: 3,
        depth: 16,
        color: 6,
        interlaced: true,
    };
    let (passes, _) = layout.passes().unwrap();
    let mut raw = Vec::new();
    for pass in passes {
        for y in 0..pass.rows {
            raw.push(0);
            for x in 0..pass.width {
                raw.extend([0x27, x as u8, 0x41, y as u8, 0xff, 0xab, 0, 0]);
            }
        }
    }
    let txt = b"Original\0Description\0 verbatim".to_vec();
    let original = png(
        &h,
        &[(*b"tEXt", txt.clone()), (*b"vpAg", vec![3, 4, 5])],
        &raw,
    );
    let best = optimize(&original, true).unwrap();
    assert!(best.len() < original.len());
    assert_eq!(parse_samples(&best), parse_samples(&original));
    let result = Image::parse(&best).unwrap();
    assert_eq!(result.chunks[1].data, txt);
    assert_eq!(result.chunks[2].data, [3, 4, 5]);
}
#[test]
fn indexed_rgba_and_semantic_reduction_preserve_hidden_colors() {
    let mut row = vec![0];
    for i in 0..256 {
        row.extend(if i % 2 == 0 {
            [255, 0, 0, 0]
        } else {
            [1, 0, 255, 0]
        });
    }
    let raw = row.repeat(4);
    let original = png(&header(256, 4, 8, 6, 0), &[], &raw);
    let best = optimize(&original, true).unwrap();
    assert!(best.len() < original.len());
    let image = Image::parse(&best).unwrap();
    if image.header.color == 3 {
        let p = image
            .chunks
            .iter()
            .find(|c| &c.kind == b"PLTE")
            .unwrap()
            .data;
        assert!(
            p.as_chunks::<3>().0.contains(&[255, 0, 0])
                && p.as_chunks::<3>().0.contains(&[1, 0, 255])
        );
        assert_eq!(
            image
                .chunks
                .iter()
                .find(|c| &c.kind == b"tRNS")
                .unwrap()
                .data,
            [0, 0]
        );
    } else {
        assert_eq!(parse_samples(&best), parse_samples(&original));
    }
}
#[test]
fn valid_apng_preserves_disposal_frame_offsets_and_frame_pixels() {
    let mut b = MAGIC.to_vec();
    write_chunk(&mut b, b"IHDR", &[], &header(12, 5, 8, 6, 0));
    write_chunk(&mut b, b"acTL", &[], &[0, 0, 0, 2, 0, 0, 0, 0]);
    let a = framectl(0, 12, 5, 0, 0);
    write_chunk(&mut b, b"fcTL", &[], &a);
    let data0 = vec![0; 5 * (1 + 12 * 4)];
    write_chunk(&mut b, b"IDAT", &[], &compress_to_vec_zlib(&data0, 0));
    let ctl = framectl(1, 3, 2, 2, 1);
    write_chunk(&mut b, b"fcTL", &[], &ctl);
    let data1 = vec![0; 2 * (1 + 3 * 4)];
    let z = compress_to_vec_zlib(&data1, 0);
    write_chunk(&mut b, b"fdAT", &2u32.to_be_bytes(), &z[..6]);
    write_chunk(&mut b, b"tEXt", &[], b"Comment\0inside frame");
    write_chunk(&mut b, b"fdAT", &3u32.to_be_bytes(), &z[6..]);
    write_chunk(&mut b, b"IEND", &[], &[]);
    let best = optimize(&b, false).unwrap();
    assert!(best.len() < b.len());
    let img = Image::parse(&best).unwrap();
    assert_eq!(img.streams.len(), 2);
    assert!(
        img.chunks
            .iter()
            .any(|c| &c.kind == b"fcTL" && c.data == ctl)
    );
    for (s, data) in img.streams.iter().zip([data0, data1]) {
        let decoded = inflate(&img.compressed(s), data.len()).unwrap();
        assert_eq!(
            Samples::decode(s.header, &decoded, None).unwrap().bytes,
            Samples::decode(s.header, &data, None).unwrap().bytes
        );
    }
}

#[test]
fn rejects_crc_truncated_streams_trailing_data_and_invalid_indices() {
    let good = png(&header(1, 1, 8, 0, 0), &[], &[0, 42]);
    for i in 0..good.len() {
        assert!(optimize(&good[..i], false).is_err());
    }
    let mut bad = good.clone();
    bad[16] ^= 1;
    assert!(optimize(&bad, false).is_err());
    let mut bad = good.clone();
    bad.push(0);
    assert!(optimize(&bad, false).is_err());
    assert!(optimize(&png(&header(1, 1, 8, 0, 0), &[], &[5, 9]), false).is_err());
    assert!(
        optimize(
            &png(
                &header(1, 1, 8, 3, 0),
                &[(*b"PLTE", vec![0, 0, 0])],
                &[0, 2]
            ),
            false
        )
        .is_err()
    );
    for kind in [*b"dSIG", *b"paNG", *b"caBX"] {
        let valid = png(&header(1, 1, 8, 0, 0), &[(kind, vec![0; 30])], &[0, 42]);
        assert_eq!(optimize(&valid, true).unwrap(), valid);
        let invalid = png(&header(1, 1, 8, 0, 0), &[(kind, vec![0; 30])], &[9, 42]);
        assert!(optimize(&invalid, true).is_err());
    }
    assert!(
        optimize(
            &png(&header(1, 1, 8, 0, 0), &[(*b"PAUX", vec![0])], &[0, 42]),
            false
        )
        .is_err()
    );
    assert!(
        optimize(
            &png(&header(999_999_999, 999_999_999, 16, 6, 0), &[], &[0]),
            false
        )
        .is_err()
    );
    let mut duplicate = compress_to_vec_zlib(&[0, 42], 1);
    duplicate.extend([1, 2]);
    assert!(inflate(&duplicate, 2).is_err());
    let mut crc = compress_to_vec_zlib(&[0, 42], 1);
    *crc.last_mut().unwrap() ^= 1;
    assert!(inflate(&crc, 2).is_err());
}

#[test]
fn corrupt_accelerator_falls_back_to_cpu() {
    let hdr = header(8, 8, 8, 6, 0);
    let mut r = vec![0; 8 * (1 + 8 * 4)];
    for (y, row) in r.as_chunks_mut::<33>().0.iter_mut().enumerate() {
        for (x, v) in row[1..].iter_mut().enumerate() {
            *v = (y * 29 + x * 89) as u8;
        }
    }
    let original = png(&hdr, &[(*b"vpAg", vec![1])], &r);
    let broken = |bytes: &[u8], _: usize, _: usize| Some(vec![0; bytes.len()]);
    let result = optimize_with_sub(&original, true, Some(&broken)).unwrap();
    assert_eq!(parse_samples(&result), parse_samples(&original));
}

#[test]
fn row_selector_is_called_once_and_rejects_invalid_results() {
    let hdr = header(33, 19, 8, 6, 0);
    let mut original_bytes = Vec::new();
    for y in 0..19 {
        original_bytes.push(0);
        original_bytes.extend((0..33 * 4).map(|i| (i * 39 + y * 251) as u8));
    }
    let input = png(&hdr, &[(*b"sBIT", vec![8, 8, 8, 8])], &original_bytes);
    let cpu = optimize(&input, true).unwrap();
    let n = std::cell::Cell::new(0);
    let valid = |_: &[u8], w: usize, h: usize| {
        assert_eq!((w, h), (33, 19));
        n.set(n.get() + 1);
        Some((0..h).map(|i| (i % 5) as u8).collect())
    };
    let out = optimize_with_rows(&input, true, Some(&valid)).unwrap();
    assert_eq!(n.get(), 1);
    assert_eq!(parse_samples(&input), parse_samples(&out));
    let wrong = |_: &[u8], _: usize, h: usize| Some(vec![7; h]);
    assert_eq!(cpu, optimize_with_rows(&input, true, Some(&wrong)).unwrap());
}

#[test]
fn rejects_invalid_palette_metadata_and_apng_frame_sequences() {
    let h = header(1, 1, 2, 3, 0);
    for invalid in [
        vec![(*b"PLTE", vec![2, 3, 4]), (*b"tRNS", vec![255, 255])],
        vec![(*b"PLTE", vec![2, 3, 4]), (*b"bKGD", vec![1])],
        vec![(*b"PLTE", vec![2, 3, 4]), (*b"hIST", vec![1, 2, 3, 4])],
        vec![(*b"sBIT", vec![0, 8, 8]), (*b"PLTE", vec![2, 3, 4])],
        vec![
            (*b"PLTE", vec![2, 3, 4]),
            (*b"tRNS", vec![255]),
            (*b"tRNS", vec![255]),
        ],
    ] {
        assert!(optimize(&png(&h, &invalid, &[0, 0]), false).is_err());
    }
    let ctl = [0, 0, 0, 1, 0, 0, 0, 0].to_vec();
    for frame in [
        framectl(7, 1, 1, 0, 0),
        framectl(0, 2, 1, 0, 0),
        framectl(0, 1, 1, 1, 0),
    ] {
        let input = png(
            &header(1, 1, 8, 0, 0),
            &[(*b"acTL", ctl.clone()), (*b"fcTL", frame)],
            &[0, 0],
        );
        assert!(optimize(&input, false).is_err());
    }
}

#[test]
fn valid_deflate_probes_roundtrip_flat_and_random_rows() {
    let layout = Header {
        width: 12,
        height: 4,
        depth: 8,
        color: 6,
        interlaced: false,
    };
    let mut data = Vec::new();
    for y in 0..layout.height {
        data.push(0);
        data.extend((0..layout.width * 4).map(|x| if y == 0 { 0 } else { (x * 163 + y) as u8 }));
    }
    let src = Samples::decode(layout, &data, None).unwrap();
    let result = src.filtered(Strategy::DeflateEstimate);
    assert_eq!(
        Samples::decode(layout, &result, None).unwrap().bytes,
        src.bytes
    );
}

#[test]
fn failed_candidates_never_discard_previous_valid_stream() {
    let layout = Header {
        width: 10,
        height: 10,
        depth: 8,
        color: 0,
        interlaced: false,
    };
    let mut raw = (0..110).map(|n| (n * 31) as u8).collect::<Vec<_>>();
    for row in raw.as_chunks_mut::<11>().0 {
        row[0] = 0;
    }
    let samples = Samples::decode(layout, &raw, None).unwrap();
    let original = compress_to_vec_zlib(&raw, 0);
    let mut best = original.clone();
    keep_if_verified(&mut best, vec![1, 2], &raw, &samples);
    keep_if_verified(
        &mut best,
        compress_to_vec_zlib(&[0; 110], 9),
        &raw,
        &samples,
    );
    assert_eq!(best, original);
}

#[test]
fn tiny_image_with_oversized_palette_keeps_every_used_color() {
    let palette: Vec<u8> = (0..256u32)
        .flat_map(|i| [(i + 13) as u8, (2 * i) as u8, (77 * i) as u8])
        .collect();
    let raw: Vec<u8> = (0..10u8)
        .flat_map(|y| std::iter::once(0).chain((0..10).map(move |x| (x + y) % 19)))
        .collect();
    let original = png(
        &header(10, 10, 8, 3, 0),
        &[(*b"PLTE", palette.clone())],
        &raw,
    );
    let optimized = optimize(&original, true).unwrap();
    let img = Image::parse(&optimized).unwrap();
    assert!(optimized.len() < original.len());
    if img.header.color == 3 {
        let pal = img.chunks.iter().find(|c| &c.kind == b"PLTE").unwrap().data;
        let (_, packed) = parse_samples(&optimized);
        let stride = (10 * img.header.depth as usize).div_ceil(8);
        for (y, row) in packed.chunks_exact(stride).enumerate() {
            for x in 0..10 {
                let i = 3 * crate::filters::sample(row, x, img.header.depth);
                let j = 3 * ((x + y) % 19);
                assert_eq!(&pal[i..i + 3], &palette[j..j + 3]);
            }
        }
    }
}
#[test]
fn color_key_reduction_preserves_transparent_colors_and_refuses_collisions() {
    // More than 256 genuine colors rule out a palette. Verify the contiguous
    // 16-bit RGB values are used directly as the transparency key.
    let make = |collision: bool, mid_alpha: bool| {
        let mut rows = Vec::new();
        for y in 0..32 {
            rows.push(0);
            for x in 0..97 {
                let mut p = [
                    x as u16 * 511,
                    y as u16 * 811,
                    (x + y) as u16 * 37,
                    65535u16,
                ];
                if (x + y) % 7 == 0 {
                    p = [777, 43006, 92, 0];
                }
                if x == 2 && y == 3 && collision {
                    p = [777, 43006, 92, 65535];
                }
                if x == 4 && y == 3 && mid_alpha {
                    p[3] = 4589;
                }
                for c in p {
                    rows.extend(c.to_be_bytes());
                }
            }
        }
        png(&header(97, 32, 16, 6, 0), &[], &rows)
    };
    for (collision, mid_alpha) in [(false, false), (true, false), (false, true)] {
        let b = make(collision, mid_alpha);
        let file = Image::parse(&b).unwrap();
        let stream = &file.streams[0];
        let raw = inflate(&file.compressed(stream), stream.header.passes().unwrap().1).unwrap();
        let samples = Samples::decode(file.header, &raw, file.palette).unwrap();
        let reduced = reduce::candidate(&file, &samples, false).unwrap();
        if collision || mid_alpha {
            assert!(reduced.is_none());
        } else {
            let reduced = reduced.unwrap();
            let file = Image::parse(&reduced).unwrap();
            assert_eq!((file.header.color, file.header.depth), (2, 16));
            let key = file.chunks.iter().find(|c| &c.kind == b"tRNS").unwrap();
            assert_eq!(key.data, [3, 9, 167, 254, 0, 92]);
        }
    }
}
#[test]
fn repeated_apng_payloads_keep_separate_frame_controls() {
    let mut b = MAGIC.to_vec();
    write_chunk(&mut b, b"IHDR", &[], &header(8, 8, 8, 6, 0));
    write_chunk(&mut b, b"acTL", &[], &[0, 0, 0, 3, 0, 0, 0, 0]);
    write_chunk(&mut b, b"fcTL", &[], &framectl(0, 8, 8, 0, 0));
    let full = compress_to_vec_zlib(&[0; 8 * (1 + 8 * 4)], 0);
    write_chunk(&mut b, b"IDAT", &[], &full);
    let sub = compress_to_vec_zlib(&[0; 3 * (1 + 3 * 4)], 0);
    for (seq, x) in [(1u32, 0u32), (3, 4)] {
        write_chunk(&mut b, b"fcTL", &[], &framectl(seq, 3, 3, x, x));
        write_chunk(&mut b, b"fdAT", &(seq + 1).to_be_bytes(), &sub);
    }
    write_chunk(&mut b, b"IEND", &[], &[]);
    let calls = std::cell::Cell::new(0);
    let callback = |_: &[u8], _: usize, _: usize| {
        calls.set(calls.get() + 1);
        None
    };
    let result = optimize_with_rows(&b, true, Some(&callback)).unwrap();
    assert_eq!(calls.get(), 2);
    let before = Image::parse(&b).unwrap();
    let after = Image::parse(&result).unwrap();
    for (a, c) in before.chunks.iter().zip(&after.chunks) {
        if a.kind == *b"fcTL" {
            assert_eq!(a.data, c.data);
        }
    }
    assert_eq!(
        after.compressed(&after.streams[1]),
        after.compressed(&after.streams[2])
    );
}

#[test]
fn maximum_keeps_the_thorough_candidate_and_preserves_metadata() {
    let mut raw = Vec::new();
    for y in 0..6 {
        raw.push(0);
        for x in 0..17 {
            let v = (x * 29 + y * 13) as u8;
            raw.extend([v, v ^ 37, 0x90, if x < 8 { 0 } else { 255 }]);
        }
    }
    let data = png(
        &header(17, 6, 8, 6, 0),
        &[
            (*b"sBIT", vec![8, 8, 8, 8]),
            (*b"tEXt", b"owner\0proof".to_vec()),
        ],
        &raw,
    );
    let thorough = optimize(&data, true).unwrap();
    let maximum = optimize_maximum_with_rows(&data, None).unwrap();
    assert!(maximum.len() <= thorough.len());
    assert_eq!(parse_samples(&maximum), parse_samples(&data));
    let a = Image::parse(&data).unwrap();
    let b = Image::parse(&maximum).unwrap();
    for kind in [*b"sBIT", *b"tEXt"] {
        assert_eq!(
            a.chunks.iter().find(|c| c.kind == kind).unwrap().data,
            b.chunks.iter().find(|c| c.kind == kind).unwrap().data
        );
    }
    let signed = png(&header(17, 6, 8, 6, 0), &[(*b"caBX", vec![22; 8])], &raw);
    assert_eq!(optimize_maximum_with_rows(&signed, None).unwrap(), signed);
}

#[test]
fn icon_recompression_keeps_ihdr_even_when_a_palette_is_smaller() {
    let mut raw = Vec::new();
    for y in 0..12 {
        raw.push(0);
        for x in 0..24 {
            raw.extend(if (x + y) % 3 == 0 {
                [21, 40, 0, 255]
            } else {
                [20, 51, 1, 255]
            });
        }
    }
    let data = png(&header(24, 12, 8, 6, 0), &[], &raw);
    let kept = optimize_preserving_header(&data, false).unwrap();
    assert!(kept.len() < data.len());
    assert_eq!(&kept[16..29], &data[16..29]);
    assert_eq!(parse_samples(&data), parse_samples(&kept));
}

#[test]
fn zlib_round_trip_handles_steady_regions_separated_by_edges() {
    // A flat image can leave gaps in the DEFLATE search cache when the encoder
    // skips long runs in the first pass. The next pass must still see its dictionary.
    let mut filtered = Vec::new();
    for row in 0u8..32 {
        filtered.extend(std::iter::repeat_n(row, 1050));
        filtered.extend((0u8..48).map(|v| v.wrapping_mul(row + 1)));
    }
    let compressed = compress_zlib_thorough(&filtered).unwrap();
    assert_eq!(inflate(&compressed, filtered.len()).unwrap(), filtered);
}

#[test]
fn optional_palette_preserves_alpha_text_and_hidden_color_then_stops() {
    let mut raw = Vec::new();
    let mut original = Vec::new();
    for y in 0..64u8 {
        raw.push(0);
        for x in 0..96u8 {
            let pixel = if y < 4 {
                [23, 18, 91, 0]
            } else {
                [
                    20 + x + y / 2,
                    12 + x / 3 + y,
                    40 + x / 2 + y / 3,
                    if y < 30 { 97 } else { 255 },
                ]
            };
            raw.extend(pixel);
            original.push(pixel);
        }
    }
    let text = (*b"tEXt", b"Author\0Fixture".to_vec());
    let mut profile = vec![0; 164];
    profile[..4].copy_from_slice(&164u32.to_be_bytes());
    profile[16..20].copy_from_slice(b"RGB ");
    profile[36..40].copy_from_slice(b"acsp");
    profile[128..132].copy_from_slice(&1u32.to_be_bytes());
    profile[132..136].copy_from_slice(b"wtpt");
    profile[136..140].copy_from_slice(&144u32.to_be_bytes());
    profile[140..144].copy_from_slice(&20u32.to_be_bytes());
    profile[144..148].copy_from_slice(b"XYZ ");
    let mut icc = b"Fixture\0\0".to_vec();
    icc.extend(miniz_oxide::deflate::compress_to_vec_zlib(&profile, 6));
    let metadata = [(*b"iCCP", icc), text];
    let source = png(&header(96, 64, 8, 6, 0), &metadata, &raw);
    let result = optimize_lossy(&source, false)
        .unwrap()
        .expect("smooth gradient can use an accurate palette");
    assert!(result.len() < source.len());
    let image = Image::parse(&result).unwrap();
    assert_eq!(image.header.color, 3);
    for (kind, data) in metadata {
        assert!(
            image
                .chunks
                .iter()
                .any(|c| c.kind == kind && c.data == data)
        );
    }
    let table = image
        .chunks
        .iter()
        .find(|c| &c.kind == b"PLTE")
        .unwrap()
        .data
        .as_chunks::<3>()
        .0;
    let alpha = image
        .chunks
        .iter()
        .find(|c| &c.kind == b"tRNS")
        .unwrap()
        .data;
    let frame = &image.streams[0];
    let decoded = Samples::decode(
        image.header,
        &inflate(&image.compressed(frame), image.header.passes().unwrap().1).unwrap(),
        image.palette,
    )
    .unwrap();
    let mut sse = 0u64;
    let mut rebuilt = Vec::new();
    for (y, row) in decoded
        .bytes
        .chunks_exact(decoded.passes[0].bytes)
        .enumerate()
    {
        rebuilt.push(0);
        for x in 0..96 {
            let i = crate::filters::sample(row, x, image.header.depth);
            let orig = original[y * 96 + x];
            let a = alpha.get(i).copied().unwrap_or(255);
            assert_eq!(a, orig[3]);
            if a == 0 {
                assert_eq!(table[i], orig[..3]);
            }
            for (r, o) in table[i].iter().zip(orig) {
                sse += (i32::from(*r) - i32::from(o)).pow(2) as u64;
            }
            rebuilt.extend(table[i]);
            rebuilt.push(a);
        }
    }
    assert!(sse > 0 && sse <= 3 * 20 * original.len() as u64);
    assert!(optimize_lossy(&result, true).unwrap().is_none());
    let expanded = png(&header(96, 64, 8, 6, 0), &[], &rebuilt);
    assert!(optimize_lossy(&expanded, false).unwrap().is_none());
}

#[test]
fn optional_palette_declines_unsupported_layouts_and_metadata() {
    let indexed = png(
        &header(2, 1, 1, 3, 0),
        &[(*b"PLTE", vec![1, 2, 3, 4, 5, 6])],
        &[0, 64],
    );
    assert!(optimize_lossy(&indexed, true).unwrap().is_none());
    for (h, chunks, rows) in [
        (header(1, 1, 16, 2, 0), vec![], vec![0; 7]),
        (header(1, 1, 8, 2, 1), vec![], vec![0; 4]),
        (
            header(1, 1, 8, 2, 0),
            vec![(*b"bKGD", vec![0; 6])],
            vec![0; 4],
        ),
        (
            header(1, 1, 8, 2, 0),
            vec![(*b"sBIT", vec![7, 7, 7])],
            vec![0; 4],
        ),
        (
            header(1, 1, 8, 2, 0),
            vec![(*b"iCCP", b"Unknown\0\0invalid".to_vec())],
            vec![0; 4],
        ),
        (
            header(1, 1, 8, 2, 0),
            vec![(*b"caBX", vec![1, 2, 3])],
            vec![0; 4],
        ),
        (
            header(1, 1, 8, 2, 0),
            vec![
                (*b"acTL", vec![0, 0, 0, 1, 0, 0, 0, 0]),
                (*b"fcTL", framectl(0, 1, 1, 0, 0)),
            ],
            vec![0; 4],
        ),
        (header(4096, 3000, 8, 2, 0), vec![], vec![]),
    ] {
        assert!(
            optimize_lossy(&png(&h, &chunks, &rows), false)
                .unwrap()
                .is_none()
        );
    }
}
