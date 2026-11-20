//! Classic TIFF, single or multiple IFDs, with uncompressed, LZW, PackBits or
//! DEFLATE strips. Decode samples without color conversion. Unknown offset
//! tags, private layouts, unreferenced nonzero bytes and shared ranges cause
//! a transparent pass-through instead of silently dropping data.
use super::MAX_PIXELS;
use miniz_oxide::deflate::compress_to_vec_zlib;
use std::{borrow::Cow, collections::HashSet, ops::Range};
use weezl::BitOrder;

#[derive(Clone, Copy)]
struct Order(bool);
impl Order {
    fn u16(self, b: &[u8]) -> u16 {
        let v = [b[0], b[1]];
        if self.0 {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        }
    }
    fn u32(self, b: &[u8]) -> u32 {
        let v = [b[0], b[1], b[2], b[3]];
        if self.0 {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        }
    }
    fn short(self, n: u16) -> [u8; 2] {
        if self.0 {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        }
    }
    fn long(self, n: u32) -> [u8; 4] {
        if self.0 {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        }
    }
}
#[derive(Clone)]
struct Field<'a> {
    tag: u16,
    ty: u16,
    count: u32,
    bytes: Cow<'a, [u8]>,
}
impl Field<'_> {
    fn numbers(&self, o: Order) -> Result<Vec<u32>, String> {
        match self.ty {
            3 => Ok(self
                .bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u32::from(o.u16(b)))
                .collect()),
            4 => Ok(self
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| o.u32(b))
                .collect()),
            _ => Err("Invalid TIFF integer type".into()),
        }
    }
}
struct Page<'a> {
    fields: Vec<Field<'a>>,
    strips: Vec<&'a [u8]>,
}
struct Product<'a> {
    fields: Vec<Field<'a>>,
    strips: Vec<Cow<'a, [u8]>>,
}

fn tagged<'a, 'b>(fields: &'b [Field<'a>], tag: u16) -> Option<&'b Field<'a>> {
    fields.iter().find(|f| f.tag == tag)
}
fn scalar(fields: &[Field<'_>], tag: u16, default: u32, o: Order) -> Result<u32, String> {
    match tagged(fields, tag) {
        None => Ok(default),
        Some(f) if f.count == 1 => Ok(f.numbers(o)?[0]),
        _ => Err("Invalid TIFF scalar count".into()),
    }
}

fn range(data: &[u8], start: usize, len: usize) -> Result<Range<usize>, String> {
    let end = start
        .checked_add(len)
        .filter(|&v| v <= data.len())
        .ok_or("TIFF range outside file")?;
    Ok(start..end)
}

fn read_pages(data: &[u8], o: Order) -> Result<Option<Vec<Page<'_>>>, String> {
    let header = data.get(..8).ok_or("Truncated TIFF header")?;
    let mut next = o.u32(&header[4..]) as usize;
    let mut regions = Vec::with_capacity(4);
    regions.push(0..8);
    let mut seen = HashSet::new();
    let mut pages = Vec::new();
    while next != 0 {
        if pages.len() >= 1024 || !seen.insert(next) {
            return Err("Repeated or excessive TIFF directories".into());
        }
        let n = o.u16(&data[range(data, next, 2)?]) as usize;
        if n > 4096 {
            return Err("Too many TIFF fields".into());
        }
        let table = range(data, next, 2 + 12 * n + 4)?;
        let entries = &data[table.clone()];
        regions.push(table);
        let mut fields = Vec::new();
        for entry in entries[2..2 + 12 * n].as_chunks::<12>().0 {
            let tag = o.u16(entry);
            let ty = o.u16(&entry[2..]);
            let count = o.u32(&entry[4..]);
            // Non-pointer baseline tags, ordinary EXIF-free description/ICC/XMP.
            if !matches!(tag,254..=259|262..=266|269..=274|277..=287|290..=291|296..=297|301|305..=306|315..=320|321|332..=334|336..=339|340..=342|700|33432|34675)
            {
                return Ok(None);
            }
            if fields.last().is_some_and(|f: &Field<'_>| f.tag >= tag) {
                return Ok(None);
            }
            let unit = match ty {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => return Ok(None),
            };
            let length = (count as usize)
                .checked_mul(unit)
                .ok_or("TIFF field size overflow")?;
            if length == 0 {
                return Ok(None);
            }
            let bytes = if length <= 4 {
                if entry[8 + length..].iter().any(|&b| b != 0) {
                    return Ok(None);
                }
                &entry[8..8 + length]
            } else {
                let r = range(data, o.u32(&entry[8..]) as usize, length)?;
                let bytes = &data[r.clone()];
                regions.push(r);
                bytes
            };
            fields.push(Field {
                tag,
                ty,
                count,
                bytes: Cow::Borrowed(bytes),
            });
        }
        let (Some(offsets), Some(sizes)) = (tagged(&fields, 273), tagged(&fields, 279)) else {
            return Ok(None);
        };
        if offsets.count != sizes.count || offsets.count == 0 || offsets.count > 65536 {
            return Err("Invalid TIFF strip table".into());
        }
        let mut strips = Vec::new();
        for (at, size) in offsets.numbers(o)?.into_iter().zip(sizes.numbers(o)?) {
            let r = range(data, at as usize, size as usize)?;
            strips.push(&data[r.clone()]);
            regions.push(r);
        }
        pages.push(Page { fields, strips });
        next = o.u32(&entries[2 + 12 * n..]) as usize;
    }
    if pages.is_empty() {
        return Err("TIFF has no pages".into());
    }
    regions.sort_by_key(|r| (r.start, r.end));
    let mut front = 0;
    for r in regions {
        if r.start < front || data[front..r.start].iter().any(|&b| b != 0) {
            return Ok(None);
        }
        front = r.end;
    }
    if data[front..].iter().any(|&b| b != 0) {
        return Ok(None);
    }
    Ok(Some(pages))
}

fn unpack(input: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(n);
    let mut p = 0;
    while p < input.len() {
        let code = input[p] as i8;
        p += 1;
        match code {
            0..=127 => {
                let count = code as usize + 1;
                if out.len() + count > n {
                    return Err("TIFF PackBits overflow".into());
                }
                out.extend_from_slice(input.get(p..p + count).ok_or("Incomplete TIFF literal")?);
                p += count;
            }
            -127..=-1 => {
                let count = (1 - i16::from(code)) as usize;
                if out.len() + count > n {
                    return Err("TIFF PackBits overflow".into());
                }
                let value = *input.get(p).ok_or("Incomplete TIFF run")?;
                p += 1;
                out.resize(out.len() + count, value);
            }
            -128 => {}
        }
    }
    if out.len() != n {
        return Err("TIFF PackBits length mismatch".into());
    }
    Ok(out)
}

fn decode_strip(input: &[u8], codec: u32, n: usize) -> Result<Option<Vec<u8>>, String> {
    let result = match codec {
        1 => {
            if input.len() != n {
                return Err("TIFF raw strip length mismatch".into());
            }
            input.to_vec()
        }
        8 | 32946 => {
            use miniz_oxide::{
                DataFormat, MZFlush, MZStatus,
                inflate::stream::{InflateState, inflate},
            };
            let mut output = vec![0; n];
            let mut state = InflateState::new_boxed(DataFormat::Zlib);
            let result = inflate(&mut state, input, &mut output, MZFlush::Finish);
            if result.status != Ok(MZStatus::StreamEnd) || result.bytes_written != n {
                return Err("Invalid or oversized TIFF DEFLATE strip".into());
            }
            if result.bytes_consumed != input.len() {
                return Ok(None);
            }
            output
        }
        32773 => unpack(input, n)?,
        5 => {
            return super::decode_lzw(
                weezl::decode::Decoder::with_tiff_size_switch(BitOrder::Msb, 8),
                input,
                n,
            );
        }
        _ => return Err("Unsupported TIFF compression".into()),
    };
    Ok(Some(result))
}

fn decode(input: &[u8], codec: u32, n: usize) -> Result<Vec<u8>, String> {
    decode_strip(input, codec, n)?.ok_or_else(|| "Unexpected trailing TIFF strip data".into())
}

struct Layout {
    stride: usize,
    channels: usize,
    bits: usize,
    order: Order,
}
impl Layout {
    fn difference(&self, bytes: &mut [u8], encode: bool) {
        let sample = self.bits / 8;
        let gap = self.channels * sample;
        for row in bytes.chunks_exact_mut(self.stride) {
            for step in 0..(row.len() - gap) / sample {
                let pos = if encode {
                    row.len() - (step + 1) * sample
                } else {
                    gap + step * sample
                };
                if sample == 1 {
                    row[pos] = if encode {
                        row[pos].wrapping_sub(row[pos - gap])
                    } else {
                        row[pos].wrapping_add(row[pos - gap])
                    };
                } else {
                    let a = self.order.u16(&row[pos..]);
                    let b = self.order.u16(&row[pos - gap..]);
                    let v = if encode {
                        a.wrapping_sub(b)
                    } else {
                        a.wrapping_add(b)
                    };
                    row[pos..pos + 2].copy_from_slice(&self.order.short(v));
                }
            }
        }
    }
}

fn replace_field(fields: &mut Vec<Field<'_>>, tag: u16, ty: u16, count: u32, bytes: Vec<u8>) {
    let field = Field {
        tag,
        ty,
        count,
        bytes: Cow::Owned(bytes),
    };
    match fields.binary_search_by_key(&tag, |f| f.tag) {
        Ok(i) => fields[i] = field,
        Err(i) => fields.insert(i, field),
    }
}

fn strip_data(blocks: &[Vec<u8>]) -> Cow<'_, [u8]> {
    if blocks.len() == 1 {
        Cow::Borrowed(&blocks[0])
    } else {
        Cow::Owned(blocks.concat())
    }
}

fn offset_table_cost(n: usize) -> usize {
    // A single LONG offset and byte count fit inline in their IFD entries.
    if n > 1 { 8 * n } else { 0 }
}

fn optimize_page<'a>(
    page: Page<'a>,
    o: Order,
    thorough: bool,
    budget: &mut usize,
) -> Result<Option<Product<'a>>, String> {
    let fields = page.fields;
    let w = scalar(&fields, 256, 0, o)? as usize;
    let h = scalar(&fields, 257, 0, o)? as usize;
    let channels = scalar(&fields, 277, 1, o)? as usize;
    let planar = scalar(&fields, 284, 1, o)?;
    let codec = scalar(&fields, 259, 1, o)?;
    let predictor = scalar(&fields, 317, 1, o)?;
    let rows = scalar(&fields, 278, u32::MAX, o)? as usize;
    if w == 0 || h == 0 || rows == 0 || w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS) {
        return Err("TIFF page outside pixel limits".into());
    }
    if !matches!(codec, 1 | 5 | 8 | 32946 | 32773)
        || !matches!(predictor, 1 | 2)
        || !matches!(planar, 1 | 2)
        || !matches!(scalar(&fields, 262, 1, o)?, 0..=5 | 8)
        || channels == 0
        || channels > 16
        || scalar(&fields, 266, 1, o)? != 1
    {
        return Ok(None);
    }
    let bits = match tagged(&fields, 258) {
        Some(f) => f.numbers(o)?,
        None => vec![1],
    };
    if bits.len() != channels || bits[0] == 0 || bits[0] > 64 || !bits.iter().all(|&b| b == bits[0])
    {
        return Ok(None);
    }
    let b = bits[0] as usize;
    if predictor == 2 && !matches!(b, 8 | 16) {
        return Ok(None);
    }
    let planes = if planar == 2 { channels } else { 1 };
    let bands = channels / planes;
    let stride = w
        .checked_mul(b * bands)
        .ok_or("TIFF row overflow")?
        .div_ceil(8);
    let count = h.div_ceil(rows);
    if count * planes != page.strips.len() {
        return Err("TIFF strip count mismatch".into());
    }
    let layout = Layout {
        stride,
        channels: bands,
        bits: b,
        order: o,
    };
    let mut raw = Vec::with_capacity(page.strips.len());
    for (i, strip) in page.strips.iter().enumerate() {
        let lines = rows.min(h - (i % count) * rows);
        let n = stride.checked_mul(lines).ok_or("TIFF strip overflow")?;
        *budget = budget
            .checked_sub(n)
            .ok_or("TIFF raster exceeds memory budget")?;
        let Some(mut bytes) = decode_strip(strip, codec, n)? else {
            return Ok(None);
        };
        if predictor == 2 {
            layout.difference(&mut bytes, false);
        }
        raw.push(bytes);
    }
    let mut best: Vec<Cow<'a, [u8]>> = page.strips.into_iter().map(Cow::Borrowed).collect();
    let mut best_len: usize = best
        .iter()
        .map(|s| s.len().next_multiple_of(2))
        .sum::<usize>()
        + offset_table_cost(best.len());
    let mut best_group = 1;
    let (mut best_codec, mut best_pred) = (codec, predictor);
    // A new Predictor field costs one 12-byte IFD entry.
    if tagged(&fields, 317).is_some() {
        best_len += 12;
    }
    let integer_samples = match tagged(&fields, 339) {
        Some(f) => f.numbers(o)?.iter().all(|&v| matches!(v, 1 | 2)),
        None => true,
    };
    let use_prediction = matches!(b, 8 | 16) && integer_samples;
    // Adjacent strips of the same plane can share an LZ77 dictionary and a
    // zlib header. Keep original grouping in competition and never join planes.
    let max_group = if thorough {
        (1_048_576 / stride.max(1) / rows.max(1)).clamp(1, count)
    } else {
        1
    };
    let groups = if max_group > 1 {
        vec![1, max_group]
    } else {
        vec![1]
    };
    for group in groups {
        let n = count.div_ceil(group) * planes;
        for pred in [1u32, 2] {
            if pred == 2 && !use_prediction {
                continue;
            }
            let mut total = offset_table_cost(n);
            if pred == 2 || tagged(&fields, 317).is_some() {
                total += 12;
            }
            let mut encoded = Vec::with_capacity(n);
            for blocks in raw.chunks(count).flat_map(|plane| plane.chunks(group)) {
                let mut scan = strip_data(blocks);
                if pred == 2 {
                    layout.difference(scan.to_mut(), true);
                }
                let mut candidate = compress_to_vec_zlib(&scan, 6);
                if thorough {
                    let alternate = compress_to_vec_zlib(&scan, 10);
                    if alternate.len() < candidate.len() {
                        candidate = alternate;
                    }
                }
                total += candidate.len().next_multiple_of(2);
                if total >= best_len {
                    break;
                }
                encoded.push(Cow::Owned(candidate));
            }
            if encoded.len() == n && total < best_len {
                best = encoded;
                best_len = total;
                best_codec = 8;
                best_pred = pred;
                best_group = group;
            }
        }
    }
    // Reserve the more expensive optimal-parse Zopfli search for the winning
    // predictor and layout on bounded, moderately sized pages.
    let effort_ok = thorough
        && matches!(best_codec, 8 | 32946)
        && raw.iter().map(Vec::len).sum::<usize>() <= 16 << 20;
    let blocks = raw.chunks(count).flat_map(|plane| plane.chunks(best_group));
    for (strip, blocks) in best.iter_mut().zip(blocks) {
        let original = strip_data(blocks);
        if effort_ok && original.len() <= 1_048_576 {
            let mut scan = original.clone();
            if best_pred == 2 {
                layout.difference(scan.to_mut(), true);
            }
            let candidate = pixminima_png::compress_zlib_thorough(&scan)?;
            if candidate.len() < strip.len() {
                *strip = Cow::Owned(candidate);
            }
        }
        let mut check = decode(strip, best_codec, original.len())?;
        if best_pred == 2 {
            layout.difference(&mut check, false);
        }
        if check.as_slice() != original.as_ref() {
            return Err("TIFF sample verification failed".into());
        }
    }
    let mut changed = fields;
    replace_field(&mut changed, 259, 3, 1, o.short(best_codec as u16).to_vec());
    if best_pred == 2 || tagged(&changed, 317).is_some() {
        replace_field(&mut changed, 317, 3, 1, o.short(best_pred as u16).to_vec());
    }
    if best_group > 1 {
        let rows = rows.saturating_mul(best_group).min(h);
        replace_field(&mut changed, 278, 4, 1, o.long(rows as u32).to_vec());
    }
    Ok(Some(Product {
        fields: changed,
        strips: best,
    }))
}

fn align(dst: &mut Vec<u8>) {
    if dst.len() % 2 == 1 {
        dst.push(0);
    }
}
fn serialize(pages: Vec<Product<'_>>, o: Order) -> Vec<u8> {
    let mut dst = if o.0 {
        b"II\x2a\0\0\0\0\0".to_vec()
    } else {
        b"MM\0\x2a\0\0\0\0".to_vec()
    };
    let mut next_offset = 4;
    for mut page in pages {
        let mut offsets = Vec::new();
        let mut lengths = Vec::new();
        for strip in &page.strips {
            align(&mut dst);
            offsets.extend(o.long(dst.len() as u32));
            lengths.extend(o.long(strip.len() as u32));
            dst.extend_from_slice(strip);
        }
        let n = page.strips.len() as u32;
        replace_field(&mut page.fields, 273, 4, n, offsets);
        replace_field(&mut page.fields, 279, 4, n, lengths);
        align(&mut dst);
        let addr = dst.len() as u32;
        dst[next_offset..next_offset + 4].copy_from_slice(&o.long(addr));
        let start = dst.len();
        let count = page.fields.len();
        dst.extend(o.short(count as u16));
        dst.resize(start + 2 + 12 * count + 4, 0);
        next_offset = start + 2 + 12 * count;
        for (i, f) in page.fields.into_iter().enumerate() {
            let at = start + 2 + 12 * i;
            dst[at..at + 2].copy_from_slice(&o.short(f.tag));
            dst[at + 2..at + 4].copy_from_slice(&o.short(f.ty));
            dst[at + 4..at + 8].copy_from_slice(&o.long(f.count));
            if f.bytes.len() <= 4 {
                dst[at + 8..at + 8 + f.bytes.len()].copy_from_slice(&f.bytes);
            } else {
                align(&mut dst);
                let off = dst.len() as u32;
                dst[at + 8..at + 12].copy_from_slice(&o.long(off));
                dst.extend_from_slice(&f.bytes);
            }
        }
    }
    dst
}

pub(crate) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    let order = Order(data[0] == b'I');
    let Some(pages) = read_pages(data, order)? else {
        return Ok(data.to_vec());
    };
    let mut packed = Vec::new();
    let mut budget = 192 * 1024 * 1024;
    for p in pages {
        let Some(page) = optimize_page(p, order, thorough, &mut budget)? else {
            return Ok(data.to_vec());
        };
        packed.push(page);
    }
    let out = serialize(packed, order);
    if out.len() >= data.len() {
        return Ok(data.to_vec());
    }
    if read_pages(&out, order)?.is_none() {
        return Err("TIFF output structure verification failed".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(o: Order) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut pages = Vec::new();
        let mut rasters = Vec::new();
        for i in 0..2 {
            let mut fields = Vec::new();
            for (tag, value) in [
                (256, 64),
                (257, 47),
                (258, 16),
                (259, 1),
                (262, 1),
                (277, 1),
                (278, 47),
            ] {
                replace_field(&mut fields, tag, 3, 1, o.short(value).to_vec());
            }
            replace_field(&mut fields, 270, 2, 8, b"details\0".to_vec());
            replace_field(&mut fields, 34675, 7, 12, b"PROFILE-data".to_vec());
            let raw: Vec<u8> = (0..64 * 47).flat_map(|n| o.short(n + i * 150)).collect();
            rasters.push(raw.clone());
            pages.push(Product {
                fields,
                strips: vec![Cow::Owned(raw)],
            });
        }
        (serialize(pages, o), rasters)
    }
    #[test]
    fn both_endians_and_all_pages_profiles_and_samples() {
        for o in [Order(true), Order(false)] {
            let (input, original) = sample(o);
            let candidate = optimize(&input, true).unwrap();
            assert!(candidate.len() < input.len());
            let pages = read_pages(&candidate, o).unwrap().unwrap();
            assert_eq!(pages.len(), 2);
            for (page, expected) in pages.iter().zip(original) {
                assert_eq!(
                    tagged(&page.fields, 270).unwrap().bytes.as_ref(),
                    b"details\0"
                );
                assert_eq!(
                    tagged(&page.fields, 34675).unwrap().bytes.as_ref(),
                    b"PROFILE-data"
                );
                let codec = scalar(&page.fields, 259, 1, o).unwrap();
                let mut raw = decode(page.strips[0], codec, expected.len()).unwrap();
                if scalar(&page.fields, 317, 1, o).unwrap() == 2 {
                    for row in raw.as_chunks_mut::<128>().0 {
                        let mut previous = 0u16;
                        for pair in row.as_chunks_mut::<2>().0 {
                            previous = previous.wrapping_add(o.u16(pair));
                            pair.copy_from_slice(&o.short(previous));
                        }
                    }
                }
                assert_eq!(raw, expected);
            }
        }
    }
    #[test]
    fn coalesces_strips_within_each_plane() {
        let o = Order(true);
        let mut fields = Vec::new();
        for (tag, value) in [
            (256, 64),
            (257, 16),
            (258, 8),
            (259, 1),
            (262, 1),
            (277, 1),
            (278, 1),
        ] {
            replace_field(&mut fields, tag, 3, 1, o.short(value).to_vec());
        }
        let row: Vec<u8> = (0..64).map(|x| x * 3).collect();
        let strips = vec![Cow::Borrowed(row.as_slice()); 16];
        let candidate = optimize(&serialize(vec![Product { fields, strips }], o), true).unwrap();
        let pages = read_pages(&candidate, o).unwrap().unwrap();
        let page = &pages[0];
        assert_eq!(page.strips.len(), 1);
        assert_eq!(scalar(&page.fields, 278, 0, o).unwrap(), 16);
        let mut samples = decode(
            page.strips[0],
            scalar(&page.fields, 259, 0, o).unwrap(),
            1024,
        )
        .unwrap();
        if scalar(&page.fields, 317, 1, o).unwrap() == 2 {
            Layout {
                stride: 64,
                channels: 1,
                bits: 8,
                order: o,
            }
            .difference(&mut samples, false);
        }
        assert_eq!(samples, row.repeat(16));
    }
    #[test]
    fn retains_unreferenced_private_data() {
        let (mut input, _) = sample(Order(true));
        input.extend(b"private thumbnail");
        assert_eq!(optimize(&input, true).unwrap(), input);
        let raw = [5; 123];
        let mut compressed = compress_to_vec_zlib(&raw, 6);
        compressed.push(71);
        assert!(decode_strip(&compressed, 8, raw.len()).unwrap().is_none());
        let mut encoded = weezl::encode::Encoder::with_tiff_size_switch(BitOrder::Msb, 8)
            .encode(&raw)
            .unwrap();
        encoded.push(71);
        assert!(decode_strip(&encoded, 5, raw.len()).unwrap().is_none());
    }
}
