//! Exact shortest row representation for the two Windows BMP RLE grammars.
use super::{MAX_PIXELS, le16, le32, take};
use std::collections::VecDeque;

fn deque_push(q: &mut VecDeque<(usize, i64)>, pos: usize, score: i64) {
    while q.back().is_some_and(|&(_, cost)| cost >= score) {
        q.pop_back();
    }
    q.push_back((pos, score));
}

fn prune(q: &mut VecDeque<(usize, i64)>, min: usize) {
    while q.front().is_some_and(|&(i, _)| i < min) {
        q.pop_front();
    }
}

fn append_row(row: &[u8], bpp: usize, dst: &mut Vec<u8>) {
    // A run costs two bytes. A literal costs 2 + 2*ceil(length/group), allowing
    // a separate rolling minimum for each source residue. This computes the
    // true shortest row in O(width), without trying up to 255 lengths per pixel.
    let group = 16 / bpp;
    let mut cost = vec![0i64; row.len() + 1];
    let mut choice = vec![0u16; row.len() + 1];
    let mut runs = VecDeque::new();
    let mut literals: [VecDeque<(usize, i64)>; 4] = Default::default();
    let mut run_start = 0;
    let period = if bpp == 8 { 1 } else { 2 };
    for end in 1..=row.len() {
        if end > period && row[end - 1] != row[end - 1 - period] {
            run_start = end - period;
        }
        let min = end.saturating_sub(255);
        deque_push(&mut runs, end - 1, cost[end - 1]);
        prune(&mut runs, run_start.max(min));
        let (start, old) = runs[0];
        cost[end] = old + 2;
        choice[end] = (end - start) as u16;
        if let Some(i) = end.checked_sub(3) {
            deque_push(
                &mut literals[i % group],
                i,
                cost[i] - 2 * (i / group) as i64,
            );
        }
        for (residue, q) in literals[..group].iter_mut().enumerate() {
            prune(q, min);
            if let Some(&(i, score)) = q.front() {
                let candidate =
                    score + 2 + 2 * (end / group) as i64 + 2 * i64::from(end % group > residue);
                if candidate < cost[end] {
                    cost[end] = candidate;
                    choice[end] = 0x100 | (end - i) as u16;
                }
            }
        }
    }
    let mut steps = Vec::with_capacity(row.len() / 128 + 1);
    let mut end = row.len();
    while end > 0 {
        let op = choice[end];
        steps.push(op);
        end -= usize::from(op & 255);
    }
    let mut start = 0;
    for op in steps.into_iter().rev() {
        let n = usize::from(op & 255);
        let block = &row[start..start + n];
        if op & 0x100 == 0 {
            let code = if bpp == 8 {
                block[0]
            } else {
                block[0] << 4 | block.get(1).copied().unwrap_or(0)
            };
            dst.extend([n as u8, code]);
        } else {
            dst.extend([0, n as u8]);
            if bpp == 8 {
                dst.extend_from_slice(block);
            } else {
                dst.extend(
                    block
                        .chunks(2)
                        .map(|s| s[0] << 4 | s.get(1).copied().unwrap_or(0)),
                );
            }
            if (n * bpp).div_ceil(8) % 2 == 1 {
                dst.push(0);
            }
        }
        start += n;
    }
    dst.extend([0, 0]);
}

fn decode_rle(stream: &[u8], w: usize, h: usize, bpp: usize) -> Result<Option<Vec<u8>>, String> {
    let mut p = 0;
    let mut rows = Vec::with_capacity(w * h);
    let (mut x, mut y) = (0, 0);
    loop {
        let code = take(stream, &mut p, 2)?;
        let size = usize::from(code[0]);
        let n = if size == 0 {
            usize::from(code[1])
        } else {
            size
        };
        if size != 0 || n >= 3 {
            if y >= h || x + n > w {
                return Err("BMP RLE span outside row".into());
            }
            if size != 0 {
                for j in 0..n {
                    rows.push(if bpp == 8 {
                        code[1]
                    } else if j % 2 == 0 {
                        code[1] >> 4
                    } else {
                        code[1] & 15
                    });
                }
            } else {
                let count = (n * bpp).div_ceil(8);
                let block = take(stream, &mut p, count)?;
                if take(stream, &mut p, count % 2)?.iter().any(|&b| b != 0) {
                    return Ok(None);
                }
                if bpp == 8 {
                    rows.extend_from_slice(block);
                } else {
                    for j in 0..n {
                        rows.push(if j % 2 == 0 {
                            block[j / 2] >> 4
                        } else {
                            block[j / 2] & 15
                        });
                    }
                }
            }
            x += n;
        } else if n == 0 {
            if x != w || y >= h {
                return Ok(None);
            }
            x = 0;
            y += 1;
        } else if n == 1 {
            if p != stream.len() || rows.len() != w * h {
                return Ok(None);
            }
            return Ok(Some(rows));
        } else {
            // DELTA / implicit holes can depend on previously initialized display memory.
            return Ok(None);
        }
    }
}

fn header_with_pixels(base: &[u8], stream: &[u8], bpp: usize, compression: u32) -> Vec<u8> {
    let mut out = base.to_vec();
    out[28..30].copy_from_slice(&(bpp as u16).to_le_bytes());
    out[30..34].copy_from_slice(&compression.to_le_bytes());
    out[34..38].copy_from_slice(&(stream.len() as u32).to_le_bytes());
    out[2..6].copy_from_slice(&((base.len() + stream.len()) as u32).to_le_bytes());
    out.extend(stream);
    out
}

fn row_bytes(w: usize, bpp: usize) -> usize {
    (w * bpp).div_ceil(32) * 4
}

fn choose(
    original: &[u8],
    base: &[u8],
    pixels: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
) -> Result<Vec<u8>, String> {
    let stride = row_bytes(w, bpp);
    let mut plain = Vec::with_capacity(stride * h);
    let mut rle = Vec::new();
    for row in pixels.chunks_exact(w) {
        let start = plain.len();
        match bpp {
            1 => {
                for part in row.chunks(8) {
                    plain.push(
                        part.iter()
                            .enumerate()
                            .fold(0, |b, (i, &x)| b | (x << (7 - i))),
                    );
                }
            }
            4 => plain.extend(
                row.chunks(2)
                    .map(|s| s[0] << 4 | s.get(1).copied().unwrap_or(0)),
            ),
            8 => plain.extend_from_slice(row),
            _ => unreachable!(),
        }
        plain.resize(start + stride, 0);
        if bpp > 1 && rle.len() + base.len() < original.len() {
            append_row(row, bpp, &mut rle);
        }
    }
    let mut best = original.to_vec();
    if base.len() + plain.len() < best.len() {
        best = header_with_pixels(base, &plain, bpp, 0);
    }
    if bpp > 1 && rle.len() + base.len() + 2 < best.len() {
        rle.extend([0, 1]);
        if decode_rle(&rle, w, h, bpp)?.as_deref() != Some(pixels) {
            return Err("BMP indices changed".into());
        }
        best = header_with_pixels(base, &rle, bpp, if bpp == 8 { 1 } else { 2 });
    }
    Ok(best)
}

pub(crate) fn optimize(data: &[u8], _thorough: bool) -> Result<Vec<u8>, String> {
    let hdr = data.get(..18).ok_or("Truncated BMP header")?;
    if le32(&hdr[14..]) != 40 {
        return Ok(data.to_vec());
    }
    let hdr = data.get(..54).ok_or("Truncated DIB header")?;
    let offset = le32(&hdr[10..]) as usize;
    let (w, h) = (le32(&hdr[18..]) as i32, le32(&hdr[22..]) as i32);
    let bpp = le16(&hdr[28..]);
    let comp = le32(&hdr[30..]);
    if h <= 0
        || w <= 0
        || le16(&hdr[26..]) != 1
        || !matches!((bpp, comp), (4 | 8 | 24, 0) | (8, 1) | (4, 2))
    {
        return Ok(data.to_vec());
    }
    let (w, h) = (w as usize, h as usize);
    let area = w.checked_mul(h).ok_or("BMP size overflow")?;
    if area > MAX_PIXELS {
        return Err("BMP exceeds pixel limit".into());
    }
    if le32(&hdr[2..]) as usize != data.len() || hdr[6..10] != [0; 4] || offset < 54 {
        return Ok(data.to_vec());
    }
    let stream = data.get(offset..).ok_or("Invalid BMP data offset")?;
    let stated = le32(&hdr[34..]) as usize;
    if stated != 0 && stated != stream.len() {
        return Ok(data.to_vec());
    }
    let declared = le32(&hdr[46..]) as usize;
    let palette = if bpp == 24 {
        declared
    } else if declared == 0 {
        1 << bpp
    } else {
        declared
    };
    if palette > 256 || offset < 54 + 4 * palette {
        return Ok(data.to_vec());
    }
    let stride = row_bytes(w, bpp);
    if comp == 0 && stride * h != stream.len() {
        return Ok(data.to_vec());
    }
    if bpp == 24 {
        // A Windows v3 24-bit bitmap has BGR samples with no color profile.
        if offset != 54 || declared != 0 || le32(&hdr[50..]) != 0 {
            return Ok(data.to_vec());
        }
        let mut lookup = std::collections::HashMap::new();
        let mut colors = Vec::new();
        let mut pixels = Vec::with_capacity(area);
        for row in stream.chunks_exact(stride) {
            if row[w * 3..].iter().any(|&b| b != 0) {
                return Ok(data.to_vec());
            }
            for px in row[..w * 3].as_chunks::<3>().0 {
                let bgr = [px[0], px[1], px[2]];
                if !lookup.contains_key(&bgr) && lookup.len() == 256 {
                    return Ok(data.to_vec());
                }
                let ix = *lookup.entry(bgr).or_insert_with(|| {
                    let i = colors.len() as u8;
                    colors.push(bgr);
                    i
                });
                pixels.push(ix);
            }
        }
        let mut base = data[..54].to_vec();
        for rgb in &colors {
            base.extend(rgb);
            base.push(0);
        }
        let len = base.len() as u32;
        base[10..14].copy_from_slice(&len.to_le_bytes());
        base[46..50].copy_from_slice(&(colors.len() as u32).to_le_bytes());
        let depths: &[usize] = match colors.len() {
            0..=2 => &[1, 4, 8],
            3..=16 => &[4, 8],
            _ => &[8],
        };
        let mut best = data.to_vec();
        for &depth in depths {
            best = choose(&best, &base, &pixels, w, h, depth)?;
        }
        return Ok(best);
    }
    let pixels = if comp == 0 {
        let mut px = Vec::with_capacity(area);
        for row in stream.chunks_exact(stride) {
            let count = (w * bpp).div_ceil(8);
            if row[count..].iter().any(|&b| b != 0)
                || (bpp == 4 && w % 2 == 1 && row[count - 1] & 15 != 0)
            {
                return Ok(data.to_vec());
            }
            if bpp == 8 {
                px.extend_from_slice(&row[..w]);
            } else {
                for j in 0..w {
                    px.push(if j % 2 == 0 {
                        row[j / 2] >> 4
                    } else {
                        row[j / 2] & 15
                    });
                }
            }
        }
        px
    } else {
        let Some(px) = decode_rle(stream, w, h, bpp)? else {
            return Ok(data.to_vec());
        };
        px
    };
    if pixels.iter().any(|&v| usize::from(v) >= palette) {
        return Err("BMP index outside palette".into());
    }
    choose(data, &data[..offset], &pixels, w, h, bpp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optimal_row_matches_exhaustive_search() {
        for bpp in [4usize, 8] {
            for seed in 1u32..55 {
                let mut s = seed;
                let row: Vec<u8> = (0..128)
                    .map(|_| {
                        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                        (s >> 30) as u8
                    })
                    .collect();
                let mut costs = vec![usize::MAX; row.len() + 1];
                costs[0] = 0;
                for i in 0..row.len() {
                    for n in 1..=255.min(row.len() - i) {
                        let span = &row[i..i + n];
                        let period = if bpp == 4 { 2 } else { 1 };
                        if span
                            .iter()
                            .skip(period)
                            .zip(span.iter())
                            .all(|(a, b)| a == b)
                        {
                            costs[i + n] = costs[i + n].min(costs[i] + 2);
                        }
                        if n >= 3 {
                            costs[i + n] =
                                costs[i + n].min(costs[i] + 2 + 2 * (n * bpp).div_ceil(16));
                        }
                    }
                }
                let mut encoded = Vec::new();
                append_row(&row, bpp, &mut encoded);
                assert_eq!(encoded.len(), costs[row.len()] + 2);
                encoded.extend([0, 1]);
                assert_eq!(
                    decode_rle(&encoded, row.len(), 1, bpp).unwrap().unwrap(),
                    row
                );
            }
        }
    }
    #[test]
    fn true_color_bmp_keeps_colors_and_resolution() {
        let (w, h) = (183usize, 31usize);
        let stride = row_bytes(w, 24);
        let mut input = vec![0; 54];
        input[..2].copy_from_slice(b"BM");
        input[10..14].copy_from_slice(&54u32.to_le_bytes());
        input[14..18].copy_from_slice(&40u32.to_le_bytes());
        input[18..22].copy_from_slice(&(w as u32).to_le_bytes());
        input[22..26].copy_from_slice(&(h as u32).to_le_bytes());
        input[26..28].copy_from_slice(&1u16.to_le_bytes());
        input[28..30].copy_from_slice(&24u16.to_le_bytes());
        input[38..42].copy_from_slice(&2835i32.to_le_bytes());
        input[42..46].copy_from_slice(&1190i32.to_le_bytes());
        let mut reference = Vec::new();
        for y in 0..h {
            let start = input.len();
            for x in 0..w {
                let b = (x / 27 + y / 5) as u8 % 3;
                let pixel = [b, 100 + b, 240 - b];
                input.extend(pixel);
                reference.push(pixel);
            }
            input.resize(start + stride, 0);
        }
        let n = input.len() as u32;
        input[2..6].copy_from_slice(&n.to_le_bytes());
        let result = optimize(&input, true).unwrap();
        assert!(result.len() < input.len() / 2);
        assert_eq!(&result[38..46], &input[38..46]);
        let start = le32(&result[10..]) as usize;
        let bpp = le16(&result[28..]);
        let px = decode_rle(&result[start..], w, h, bpp).unwrap().unwrap();
        let expanded: Vec<[u8; 3]> = px
            .iter()
            .map(|&ix| {
                let i = 54 + 4 * usize::from(ix);
                [result[i], result[i + 1], result[i + 2]]
            })
            .collect();
        assert_eq!(expanded, reference);
    }
}
