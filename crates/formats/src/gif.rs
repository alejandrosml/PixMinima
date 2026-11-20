use super::{MAX_ANIMATION_PIXELS, MAX_PIXELS, le16, take};
use weezl::{BitOrder, decode::Decoder, encode::Encoder};

fn subblocks(
    data: &[u8],
    p: &mut usize,
    mut destination: Option<&mut Vec<u8>>,
) -> Result<(), String> {
    loop {
        let count = usize::from(take(data, p, 1)?[0]);
        if count == 0 {
            return Ok(());
        }
        let block = take(data, p, count)?;
        if let Some(dst) = &mut destination {
            dst.extend_from_slice(block);
        }
    }
}

fn pack(size: u8, codes: &[u8]) -> Vec<u8> {
    let mut bytes = vec![size];
    for block in codes.chunks(255) {
        bytes.push(block.len() as u8);
        bytes.extend_from_slice(block);
    }
    bytes.push(0);
    bytes
}

fn decode(size: u8, codes: &[u8], count: usize) -> Result<Vec<u8>, String> {
    super::decode_lzw(Decoder::new(BitOrder::Lsb, size), codes, count)?
        .ok_or_else(|| "Unexpected trailing GIF LZW data".into())
}

struct Bits {
    output: Vec<u8>,
    accumulator: u32,
    bits: u8,
}
impl Bits {
    fn put(&mut self, code: u16, width: u8) {
        self.accumulator |= u32::from(code) << self.bits;
        self.bits += width;
        while self.bits >= 8 {
            self.output.push(self.accumulator as u8);
            self.accumulator >>= 8;
            self.bits -= 8;
        }
    }
}

// GIF permits holding a full dictionary without clearing it. This can help
// steady textures, while an early clear can help frames with changing statistics.
#[derive(Clone, Copy)]
enum Clear {
    At(u16),
    Freeze,
    Adaptive,
}

const CLEAR_POLICIES: [Clear; 4] = [
    Clear::At(1024),
    Clear::At(2048),
    Clear::Freeze,
    Clear::Adaptive,
];

fn alternative_lzw(indices: &[u8], size: u8, policy: Clear) -> Vec<u8> {
    let clear = 1u16 << size;
    let end = clear + 1;
    let mut width = size + 1;
    let mut next = end + 1;
    let mut stream = Bits {
        output: Vec::new(),
        accumulator: 0,
        bits: 0,
    };
    stream.put(clear, width);
    let mut prefix = u16::from(indices[0]);
    // A 2-way prefix/suffix dictionary in an open-address table. At most
    // 4090 live entries in 8192 bins keeps each lookup short.
    let mut dictionary = [u32::MAX; 8192];
    let mut entries = [0u16; 8192];
    let mut sample_start = 0;
    let mut sample_bits = 0;
    let mut best_rate = usize::MAX;
    for (pos, &byte) in indices[1..].iter().enumerate() {
        let key = (u32::from(prefix) << 8) | u32::from(byte);
        let mut slot = (key.wrapping_mul(2_654_435_761) >> 19) as usize;
        while dictionary[slot] != u32::MAX && dictionary[slot] != key {
            slot = (slot + 1) & 8191;
        }
        if dictionary[slot] == key {
            prefix = entries[slot];
            continue;
        }
        stream.put(prefix, width);
        // The decoder allocates its new entry one emitted code after us.
        if next == (1 << width) && width < 12 {
            width += 1;
        }
        let reset = match policy {
            Clear::At(limit) => next >= limit,
            Clear::Adaptive if next >= 512 && pos - sample_start >= 2048 => {
                let total = stream.output.len() * 8 + usize::from(stream.bits);
                let rate = (total - sample_bits) * 1024 / (pos - sample_start);
                let stale = rate > best_rate.saturating_add(best_rate / 4);
                best_rate = if stale {
                    usize::MAX
                } else {
                    best_rate.min(rate)
                };
                sample_start = pos;
                sample_bits = total;
                stale
            }
            _ => false,
        };
        if reset {
            stream.put(clear, width);
            dictionary.fill(u32::MAX);
            width = size + 1;
            next = end + 1;
        } else if next < 4096 {
            dictionary[slot] = key;
            entries[slot] = next;
            next += 1;
        }
        prefix = u16::from(byte);
    }
    stream.put(prefix, width);
    if next == (1 << width) && width < 12 {
        width += 1;
    }
    stream.put(end, width);
    if stream.bits > 0 {
        stream.output.push(stream.accumulator as u8);
    }
    stream.output
}

fn try_candidate(size: u8, codes: &[u8], pixels: &[u8], best: &mut Vec<u8>) -> Result<(), String> {
    if 2 + codes.len() + codes.len().div_ceil(255) < best.len() {
        if decode(size, codes, pixels.len())? != pixels {
            return Err("GIF index verification failed".into());
        }
        *best = pack(size, codes);
    }
    Ok(())
}

// Keep palette bytes intact. Every mapped index lands on a fixed representative,
// so applying this operation again to the same palette cannot shift a color.
fn merge_nearby(indices: &mut [u8], palette: &[u8], transparent: Option<u8>) -> bool {
    let colors = palette.as_chunks::<3>().0;
    let mut map = [0u8; 256];
    for (i, rgb) in colors.iter().enumerate() {
        map[i] = i as u8;
        if transparent == Some(i as u8) {
            continue;
        }
        let mut best = u16::MAX;
        for (j, other) in colors[..i].iter().enumerate() {
            if map[j] != j as u8 || transparent == Some(j as u8) {
                continue;
            }
            let d = [
                rgb[0].abs_diff(other[0]),
                rgb[1].abs_diff(other[1]),
                rgb[2].abs_diff(other[2]),
            ];
            if d.iter().any(|&x| x > 4) {
                continue;
            }
            let cost = d.iter().map(|&x| u16::from(x).pow(2)).sum();
            if cost < best {
                best = cost;
                map[i] = j as u8;
            }
        }
    }
    let mut total = [0u64; 3];
    let mut changed = false;
    for value in &mut *indices {
        let old = usize::from(*value);
        let next = map[old];
        for (c, sum) in total.iter_mut().enumerate() {
            *sum += u64::from(colors[old][c].abs_diff(colors[usize::from(next)][c])).pow(2);
        }
        changed |= next != *value;
        *value = next;
    }
    changed
        && total
            .iter()
            .all(|&sum| sum * 4 <= 25 * indices.len() as u64)
}

pub(crate) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    recompress(data, thorough, false)
}

pub(crate) fn optimize_lossy(data: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let candidate = recompress(data, thorough, true)?;
    Ok((candidate.len() < data.len()).then_some(candidate))
}

fn recompress(data: &[u8], thorough: bool, lossy: bool) -> Result<Vec<u8>, String> {
    let mut p = 6;
    let header = take(data, &mut p, 7)?;
    let (screen_w, screen_h) = (le16(header), le16(&header[2..]));
    if screen_w == 0 || screen_h == 0 {
        return Err("Empty GIF screen".into());
    }
    let palette_entries = |flags: u8| {
        if flags & 0x80 == 0 {
            0
        } else {
            2usize << (flags & 7)
        }
    };
    let global_entries = palette_entries(header[4]);
    let global_palette = take(data, &mut p, 3 * global_entries)?;
    let mut transparent = None;
    let mut out = data[..p].to_vec();
    let mut total_pixels = 0;
    let mut frame_count = 0;
    loop {
        let start = p;
        match take(data, &mut p, 1)?[0] {
            0x3b => {
                if frame_count == 0 {
                    return Err("No GIF frames".into());
                }
                if p != data.len() {
                    return Ok(data.to_vec());
                }
                out.push(0x3b);
                return Ok(out);
            }
            0x21 => {
                let label = take(data, &mut p, 1)?[0];
                if lossy && label == 0xf9 {
                    let Some(ctrl) = data.get(p..p + 6) else {
                        return Err("Incomplete GIF graphics control".into());
                    };
                    if ctrl[0] != 4 || ctrl[5] != 0 {
                        return Ok(data.to_vec());
                    }
                    transparent = (ctrl[1] & 1 != 0).then_some(ctrl[4]);
                } else if label == 0x01 {
                    // A plain-text rendering block consumes the pending control.
                    transparent = None;
                }
                if label == 0xff {
                    // Other private records may contain byte offsets into image data.
                    let known = data.get(p..p + 12).is_some_and(|b| {
                        b[0] == 11
                            && matches!(
                                &b[1..],
                                b"NETSCAPE2.0" | b"ANIMEXTS1.0" | b"ICCRGBG1012" | b"XMP DataXMP"
                            )
                    });
                    if !known {
                        return Ok(data.to_vec());
                    }
                } else if !matches!(label, 0x01 | 0xf9 | 0xfe) {
                    return Ok(data.to_vec());
                }
                subblocks(data, &mut p, None)?;
                out.extend_from_slice(&data[start..p]);
            }
            0x2c => {
                let frame = take(data, &mut p, 9)?;
                let (x, y, w, h) = (
                    le16(frame),
                    le16(&frame[2..]),
                    le16(&frame[4..]),
                    le16(&frame[6..]),
                );
                if x + w > screen_w || y + h > screen_h || w == 0 || h == 0 {
                    return Err("Invalid GIF frame rectangle".into());
                }
                let n = w.checked_mul(h).ok_or("GIF size overflow")?;
                total_pixels += n;
                frame_count += 1;
                if n > MAX_PIXELS || total_pixels > MAX_ANIMATION_PIXELS || frame_count > 16_384 {
                    return Err("GIF exceeds frame limits".into());
                }
                let local = palette_entries(frame[8]);
                let local_palette = take(data, &mut p, local * 3)?;
                out.extend_from_slice(&data[start..p]);
                let start_codes = p;
                let original_size = take(data, &mut p, 1)?[0];
                if !(2..=8).contains(&original_size) {
                    return Err("Invalid GIF LZW minimum code size".into());
                }
                let mut codes = Vec::new();
                subblocks(data, &mut p, Some(&mut codes))?;
                let Some(mut pixels) =
                    super::decode_lzw(Decoder::new(BitOrder::Lsb, original_size), &codes, n)?
                else {
                    return Ok(data.to_vec());
                };
                let max = *pixels.iter().max().ok_or("Empty frame")?;
                let entries = if local == 0 { global_entries } else { local };
                if entries == 0 {
                    return Ok(data.to_vec());
                } // may rely on a preceding data stream's global table
                if usize::from(max) >= entries {
                    return Err("GIF index outside palette".into());
                }
                let max = if lossy {
                    let palette = if local == 0 {
                        global_palette
                    } else {
                        local_palette
                    };
                    let changed = merge_nearby(&mut pixels, palette, transparent.take());
                    if !changed {
                        out.extend_from_slice(&data[start_codes..p]);
                        continue;
                    }
                    *pixels.iter().max().unwrap()
                } else {
                    max
                };
                let smallest = (8 - max.leading_zeros() as u8).max(2);
                let mut best = data[start_codes..p].to_vec();
                let compacted = pack(original_size, &codes);
                if !lossy && compacted.len() < best.len() {
                    best = compacted;
                }
                let sizes = if thorough {
                    smallest..=original_size
                } else {
                    smallest..=smallest
                };
                for size in sizes {
                    let encoded = Encoder::new(BitOrder::Lsb, size)
                        .encode(&pixels)
                        .map_err(|e| e.to_string())?;
                    try_candidate(size, &encoded, &pixels, &mut best)?;
                    if thorough && size == smallest && pixels.len() > 1024 {
                        for threshold in CLEAR_POLICIES {
                            let other = alternative_lzw(&pixels, size, threshold);
                            try_candidate(size, &other, &pixels, &mut best)?;
                        }
                    }
                }
                out.extend_from_slice(&best);
            }
            _ => return Err("Unknown GIF block".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_palette_index_mapping_keeps_transparency_and_timing() {
        let mut input = b"GIF89a\x80\0\x10\0\x81\0\0".to_vec();
        input.extend_from_slice(&[40, 40, 40, 41, 42, 40, 42, 40, 40, 210, 10, 10]);
        let header = b"\x21\xf9\x04\x05\x07\x00\x02\x00\x2c\x00\x00\x00\x00\x80\x00\x10\x00\x00";
        let indices: Vec<_> = (0..2048)
            .map(|i| match i % 17 {
                0 => 2,
                1 => 3,
                _ => (((i * 373) ^ (i / 3)) & 1) as u8,
            })
            .collect();
        for _ in 0..2 {
            input.extend_from_slice(header);
            input.extend(pack(
                2,
                &Encoder::new(BitOrder::Lsb, 2).encode(&indices).unwrap(),
            ));
        }
        input.push(0x3b);
        let candidate = optimize_lossy(&input, true).unwrap().unwrap();
        assert_eq!(&candidate[..25], &input[..25]);
        let mut p = 25;
        for _ in 0..2 {
            assert_eq!(&candidate[p..p + 18], header);
            p += 18;
            let code_size = candidate[p];
            p += 1;
            let mut stream = Vec::new();
            subblocks(&candidate, &mut p, Some(&mut stream)).unwrap();
            let actual = decode(code_size, &stream, indices.len()).unwrap();
            assert!(
                indices
                    .iter()
                    .zip(actual)
                    .all(|(&a, b)| b == if a == 1 { 0 } else { a })
            );
        }
        assert!(optimize_lossy(&candidate, true).unwrap().is_none());
    }
    #[test]
    fn lzw_width_changes_and_dictionary_resets() {
        let mut r = 23u32;
        for depth in 2..=8 {
            let indices: Vec<u8> = (0..95000)
                .map(|_| {
                    r = r.wrapping_mul(1664525).wrapping_add(1013904223);
                    ((r >> 24) as u8) & ((1u16 << depth) - 1) as u8
                })
                .collect();
            for limit in CLEAR_POLICIES {
                assert_eq!(
                    decode(
                        depth,
                        &alternative_lzw(&indices, depth, limit),
                        indices.len()
                    )
                    .unwrap(),
                    indices
                );
            }
        }
    }
    #[test]
    fn animated_gif_preserves_extensions_frame_rectangles_and_indices() {
        let mut input = b"GIF89a\x04\x00\x02\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff".to_vec();
        input.extend_from_slice(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00");
        let mut expected = Vec::new();
        for rows in [[0, 0, 1, 0, 1, 1, 0, 1], [1, 0, 0, 1, 0, 0, 1, 0]] {
            let header =
                b"\x21\xf9\x04\x09\x07\x00\x00\x00\x2c\x00\x00\x00\x00\x04\x00\x02\x00\x00";
            input.extend_from_slice(header);
            let codes = Encoder::new(BitOrder::Lsb, 8).encode(&rows).unwrap();
            input.push(8);
            for b in codes {
                input.extend([1, b]);
            }
            input.push(0);
            expected.push(rows);
        }
        input.push(0x3b);
        let result = optimize(&input, true).unwrap();
        assert!(result.len() < input.len());
        let mut p = 38;
        for pixels in expected {
            assert_eq!(
                &result[p..p + 18],
                b"\x21\xf9\x04\x09\x07\x00\x00\x00\x2c\x00\x00\x00\x00\x04\x00\x02\x00\x00"
            );
            p += 18;
            let size = result[p];
            p += 1;
            let mut codes = Vec::new();
            subblocks(&result, &mut p, Some(&mut codes)).unwrap();
            assert_eq!(decode(size, &codes, 8).unwrap(), pixels);
        }
        assert_eq!(&result[p..], &[0x3b]);
        let mut opaque = input.clone();
        opaque.splice(
            19..19,
            b"\x21\xff\x0bPRIVATE0100\x02\x01\x02\0".iter().copied(),
        );
        assert_eq!(optimize(&opaque, true).unwrap(), opaque);
        let mut codes = Encoder::new(BitOrder::Lsb, 2)
            .encode(&[1, 0, 1, 0])
            .unwrap();
        codes.push(77);
        assert!(
            super::super::decode_lzw(Decoder::new(BitOrder::Lsb, 2), &codes, 4)
                .unwrap()
                .is_none()
        );
    }
}
