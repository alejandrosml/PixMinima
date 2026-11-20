//! Search alternative lossless PNG representations when their ancillary chunks
//! can be preserved exactly. Unsupported metadata always keeps the source layout.
use std::collections::HashMap;

use super::{inflate, search};
use crate::filters::{Samples, Strategy, sample};
use crate::format::{Header, Image, MAGIC, write_chunk};
use miniz_oxide::deflate::compress_to_vec_zlib;

type Pixel = [u16; 4];
const FULL: u16 = u16::MAX;
const MAX_PIXELS: usize = 24_000_000;

struct Layout {
    h: Header,
    palette: Vec<Pixel>,
    key: Option<[u16; 3]>,
}

fn scale_to_16(value: usize, depth: u8) -> u16 {
    (value as u32 * 65535 / ((1u32 << depth) - 1)) as u16
}
fn can_represent(value: u16, depth: u8) -> bool {
    u32::from(value).is_multiple_of(65535 / ((1u32 << depth) - 1))
}

impl Layout {
    fn read(&self, row: &[u8], x: usize) -> Pixel {
        let h = self.h;
        if h.color == 3 {
            return self.palette[sample(row, x, h.depth)];
        }
        let channels = h.channels();
        let field = |n: usize| -> u16 {
            match h.depth {
                16 => {
                    let i = 2 * (x * channels + n);
                    u16::from_be_bytes([row[i], row[i + 1]])
                }
                8 => u16::from(row[x * channels + n]) * 257,
                _ => scale_to_16(sample(row, x, h.depth), h.depth),
            }
        };
        let v = field(0);
        let mut pixel = match h.color {
            0 => [v, v, v, FULL],
            4 => [v, v, v, field(1)],
            2 => [v, field(1), field(2), FULL],
            6 => [v, field(1), field(2), field(3)],
            _ => unreachable!(),
        };
        if self.key.as_ref().is_some_and(|key| pixel[..3] == *key) {
            pixel[3] = 0;
        }
        pixel
    }
    fn bytes_per_row(&self) -> usize {
        (self.h.width * self.h.channels() * usize::from(self.h.depth)).div_ceil(8)
    }
}

fn eligible(image: &Image<'_>) -> bool {
    if image.header.interlaced
        || image.streams.len() != 1
        || image.header.width * image.header.height > MAX_PIXELS
    {
        return false;
    }
    // bKGD, sBIT, hIST, ICC and private chunks may reference the exact source
    // channel layout, color space or palette. Never guess their intended meaning.
    image.chunks.iter().all(|c| {
        matches!(
            &c.kind,
            b"IHDR"
                | b"IDAT"
                | b"IEND"
                | b"gAMA"
                | b"sRGB"
                | b"cHRM"
                | b"pHYs"
                | b"tIME"
                | b"iTXt"
                | b"tEXt"
                | b"zTXt"
                | b"eXIf"
        ) || (image.header.color == 3 && matches!(&c.kind, b"PLTE" | b"tRNS"))
    })
}

fn source_layout(image: &Image<'_>) -> Result<Layout, String> {
    let mut palette = Vec::new();
    if image.header.color == 3 {
        let p = image
            .chunks
            .iter()
            .find(|c| &c.kind == b"PLTE")
            .ok_or("Missing palette")?;
        let alphas = image
            .chunks
            .iter()
            .find(|c| &c.kind == b"tRNS")
            .map_or(&[][..], |c| c.data);
        if alphas.len() > p.data.len() / 3 {
            return Err("Invalid palette alpha length".into());
        }
        for (i, rgb) in p.data.as_chunks::<3>().0.iter().enumerate() {
            palette.push(
                [
                    rgb[0],
                    rgb[1],
                    rgb[2],
                    alphas.get(i).copied().unwrap_or(255),
                ]
                .map(|v| u16::from(v) * 257),
            );
        }
    }
    let key = if matches!(image.header.color, 0 | 2) {
        image.chunks.iter().find(|c| &c.kind == b"tRNS").map(|c| {
            let value = |off: usize| {
                scale_to_16(
                    usize::from(u16::from_be_bytes([c.data[off], c.data[off + 1]])),
                    image.header.depth,
                )
            };
            if image.header.color == 0 {
                [value(0); 3]
            } else {
                [value(0), value(2), value(4)]
            }
        })
    } else {
        None
    };
    Ok(Layout {
        h: image.header,
        palette,
        key,
    })
}

fn layouts(src: &Layout, samples: &Samples) -> Vec<Layout> {
    let mut counts = HashMap::<Pixel, usize>::new();
    let mut gray = true;
    let mut opaque = true;
    let mut keyed = matches!(src.h.color, 4 | 6 | 3);
    let mut key: Option<[u16; 3]> = None;
    let mut depths = [true; 4];
    let candidates = [1, 2, 4, 8];
    for row in samples.bytes.chunks_exact(src.bytes_per_row()) {
        for x in 0..src.h.width {
            let p = src.read(row, x);
            gray &= p[0] == p[1] && p[0] == p[2];
            opaque &= p[3] == FULL;
            if keyed && p[3] != FULL {
                let rgb = [p[0], p[1], p[2]];
                keyed = p[3] == 0 && key.is_none_or(|k| k == rgb);
                key = Some(rgb);
            }
            for (ok, bits) in depths.iter_mut().zip(candidates) {
                if *ok {
                    *ok = p.iter().all(|&c| can_represent(c, bits));
                }
            }
            if counts.len() <= 256 {
                *counts.entry(p).or_default() += 1;
            }
            if counts.len() > 256
                && !gray
                && (src.h.color == 2 || !opaque)
                && (src.h.depth == 8 || !depths[3])
                && !keyed
            {
                return Vec::new();
            }
        }
    }
    let mut out = Vec::new();
    {
        let color = match (gray, opaque) {
            (true, true) => 0,
            (true, false) => 4,
            (false, true) => 2,
            (false, false) => 6,
        };
        let depth = depths
            .iter()
            .zip(candidates)
            .find_map(|(&ok, d)| (ok && (color == 0 || d >= 8)).then_some(d))
            .unwrap_or(16);
        if (color, depth) != (src.h.color, src.h.depth) {
            out.push(Layout {
                h: Header {
                    color,
                    depth,
                    ..src.h
                },
                palette: Vec::new(),
                key: None,
            });
        }
    }
    // A color key can replace the entire alpha channel exactly when every
    // transparent pixel has the same RGB and that RGB is never opaque.
    if let Some(rgb) = key.filter(|_| keyed) {
        let opaque_collision = if counts.len() <= 256 {
            counts.contains_key(&[rgb[0], rgb[1], rgb[2], FULL])
        } else {
            samples.bytes.chunks_exact(src.bytes_per_row()).any(|row| {
                (0..src.h.width).any(|x| src.read(row, x) == [rgb[0], rgb[1], rgb[2], FULL])
            })
        };
        if !opaque_collision {
            let color = if gray { 0 } else { 2 };
            let depth = depths
                .iter()
                .zip(candidates)
                .find_map(|(&ok, d)| (ok && (gray || d >= 8)).then_some(d))
                .unwrap_or(16);
            out.push(Layout {
                h: Header {
                    color,
                    depth,
                    ..src.h
                },
                palette: Vec::new(),
                key: Some(rgb),
            });
        }
    }
    if depths[3] && counts.len() <= 256 {
        // This branch has no palette-referencing ancillary metadata. Each used
        // RGBA tuple, including the RGB of fully transparent pixels, survives.
        let mut palette: Vec<Pixel> = counts.keys().copied().collect();
        let len = palette.len();
        let depth = if len <= 2 {
            1
        } else if len <= 4 {
            2
        } else if len <= 16 {
            4
        } else {
            8
        };
        let h = Header {
            color: 3,
            depth,
            ..src.h
        };
        palette.sort_unstable_by_key(|p| {
            (
                p[3] == FULL,
                std::cmp::Reverse(counts.get(p).copied().unwrap_or(0)),
                *p,
            )
        });
        out.push(Layout {
            h,
            palette: palette.clone(),
            key: None,
        });
        palette.sort_unstable_by_key(|p| (p[3] == FULL, *p));
        if palette != out.last().unwrap().palette {
            out.push(Layout {
                h,
                palette,
                key: None,
            });
        }
    }
    out
}
// Put commonly adjacent samples near one another in the index space. This
// targets small Sub/Up residuals directly and changes no palette color.
fn spatial_palette(src: &Layout, samples: &Samples, seed: &Layout) -> Option<Layout> {
    let n = seed.palette.len();
    let area = src.h.width * src.h.height;
    if n < 4 || !(1024..=4 * 1024 * 1024).contains(&area) {
        return None;
    }
    let ids: HashMap<_, _> = seed.palette.iter().copied().zip(0..n).collect();
    let mut edges = vec![0u32; n * n];
    let mut previous = vec![0u8; src.h.width];
    let mut connect = |a: usize, b: usize| {
        if a != b {
            edges[a * n + b] += 1;
            edges[b * n + a] += 1;
        }
    };
    for (y, row) in samples.bytes.chunks_exact(src.bytes_per_row()).enumerate() {
        let mut left = None;
        for (x, up) in previous.iter_mut().enumerate() {
            let id = ids[&src.read(row, x)];
            if let Some(prior) = left {
                connect(prior, id);
            }
            if y > 0 {
                connect(usize::from(*up), id);
            }
            left = Some(id);
            *up = id as u8;
        }
    }
    let mut ordered = Vec::with_capacity(n);
    for opaque in [false, true] {
        let mut remaining: Vec<_> = (0..n)
            .filter(|&i| (seed.palette[i][3] == FULL) == opaque)
            .collect();
        let Some(start) = remaining.iter().copied().min_by_key(|&i| {
            (
                remaining.iter().filter(|&&j| edges[i * n + j] > 0).count(),
                seed.palette[i],
            )
        }) else {
            continue;
        };
        let mut current = start;
        loop {
            ordered.push(seed.palette[current]);
            remaining.retain(|&x| x != current);
            let Some(next) = remaining.iter().copied().min_by_key(|&j| {
                let distance: u32 = seed.palette[current]
                    .iter()
                    .zip(seed.palette[j])
                    .map(|(&a, b)| u32::from(a.abs_diff(b)))
                    .sum();
                (std::cmp::Reverse(edges[current * n + j]), distance, j)
            }) else {
                break;
            };
            current = next;
        }
    }
    Some(Layout {
        h: seed.h,
        palette: ordered,
        key: None,
    })
}
fn write_scalar(row: &mut [u8], index: usize, depth: u8, value: u16) {
    match depth {
        16 => row[index * 2..index * 2 + 2].copy_from_slice(&value.to_be_bytes()),
        8 => row[index] = (value / 257) as u8,
        _ => {
            let bit = index * usize::from(depth);
            let v = (u32::from(value) * ((1u32 << depth) - 1) / 65535) as u8;
            row[bit / 8] |= v << (8 - depth - (bit % 8) as u8);
        }
    }
}
fn convert(src: &Layout, dst: &Layout, original: &Samples) -> Samples {
    let mut lookup = HashMap::new();
    for (i, &p) in dst.palette.iter().enumerate() {
        lookup.entry(p).or_insert(i);
    }
    let stride = dst.bytes_per_row();
    let mut bytes = vec![0; dst.h.height * stride];
    for (row, dest) in original
        .bytes
        .chunks_exact(src.bytes_per_row())
        .zip(bytes.chunks_exact_mut(stride))
    {
        for x in 0..dst.h.width {
            let p = src.read(row, x);
            match dst.h.color {
                3 => {
                    let idx = lookup[&p] as u8;
                    let bit = x * usize::from(dst.h.depth);
                    dest[bit / 8] |= idx << (8 - dst.h.depth - (bit % 8) as u8);
                }
                0 => write_scalar(dest, x, dst.h.depth, p[0]),
                4 => {
                    write_scalar(dest, x * 2, dst.h.depth, p[0]);
                    write_scalar(dest, x * 2 + 1, dst.h.depth, p[3]);
                }
                _ => {
                    for (c, &value) in p.iter().take(dst.h.channels()).enumerate() {
                        write_scalar(dest, x * dst.h.channels() + c, dst.h.depth, value);
                    }
                }
            }
        }
    }
    Samples {
        bytes,
        passes: dst.h.passes().unwrap().0,
        header: dst.h,
    }
}
fn equal(src: &Layout, a: &Samples, dst: &Layout, b: &Samples) -> bool {
    a.bytes
        .chunks_exact(src.bytes_per_row())
        .zip(b.bytes.chunks_exact(dst.bytes_per_row()))
        .all(|(ra, rb)| (0..src.h.width).all(|x| src.read(ra, x) == dst.read(rb, x)))
}
fn rebuild(image: &Image<'_>, dst: &Layout, stream: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let mut data_written = false;
    for c in &image.chunks {
        match &c.kind {
            b"IHDR" => {
                let mut bytes = c.data.to_vec();
                bytes[8] = dst.h.depth;
                bytes[9] = dst.h.color;
                write_chunk(&mut out, &c.kind, &[], &bytes);
            }
            b"PLTE" | b"tRNS" => {}
            b"IDAT" => {
                if data_written {
                    continue;
                }
                if dst.h.color == 3 {
                    let mut rgb = Vec::new();
                    let mut alpha = Vec::new();
                    for p in &dst.palette {
                        rgb.extend(p[..3].iter().map(|v| (v / 257) as u8));
                        alpha.push((p[3] / 257) as u8);
                    }
                    write_chunk(&mut out, b"PLTE", &[], &rgb);
                    if let Some(i) = alpha.iter().rposition(|&a| a != 255) {
                        write_chunk(&mut out, b"tRNS", &[], &alpha[..=i]);
                    }
                }
                if let Some(rgb) = dst.key {
                    let channels = dst.h.channels();
                    let max = (1u32 << dst.h.depth) - 1;
                    let key: Vec<_> = rgb[..channels]
                        .iter()
                        .flat_map(|&v| ((u32::from(v) * max / 65535) as u16).to_be_bytes())
                        .collect();
                    write_chunk(&mut out, b"tRNS", &[], &key);
                }
                write_chunk(&mut out, b"IDAT", &[], stream);
                data_written = true;
            }
            _ => write_chunk(&mut out, &c.kind, &[], c.data),
        }
    }
    out
}

struct Candidate {
    cost: usize,
    layout: Layout,
    trial: Samples,
    raw: Vec<u8>,
    compressed: Vec<u8>,
}

fn estimate(image: &Image<'_>, src: &Layout, original: &Samples, layout: Layout) -> Candidate {
    let trial = convert(src, &layout, original);
    let (raw, compressed) = [Strategy::Fixed(0), Strategy::MinSum]
        .into_iter()
        .map(|s| {
            let raw = trial.filtered(s);
            let compressed = compress_to_vec_zlib(&raw, 6);
            (raw, compressed)
        })
        .min_by_key(|(_, c)| c.len())
        .unwrap();
    let cost = rebuild(image, &layout, &compressed).len();
    Candidate {
        cost,
        layout,
        trial,
        raw,
        compressed,
    }
}
pub(crate) fn candidate(
    image: &Image<'_>,
    original: &Samples,
    thorough: bool,
) -> Result<Option<Vec<u8>>, String> {
    if !eligible(image) {
        return Ok(None);
    }
    let src = source_layout(image)?;
    let layouts = layouts(&src, original);
    let spatial = if thorough {
        layouts
            .iter()
            .find(|layout| layout.h.color == 3)
            .and_then(|seed| spatial_palette(&src, original, seed))
            .filter(|l| {
                l.palette != src.palette
                    && !layouts.iter().any(|previous| l.palette == previous.palette)
            })
    } else {
        None
    };
    let standard = layouts
        .into_iter()
        .filter(|l| l.h != src.h || l.palette != src.palette || l.key != src.key)
        .map(|l| estimate(image, &src, original, l))
        .min_by_key(|c| c.cost);
    let extra = spatial
        .map(|l| estimate(image, &src, original, l))
        .filter(|v| {
            standard
                .as_ref()
                .is_none_or(|s| v.cost <= s.cost + s.cost / 20)
        });
    let mut result: Option<Vec<u8>> = None;
    for Candidate {
        layout,
        trial,
        raw,
        compressed,
        ..
    } in [standard, extra].into_iter().flatten()
    {
        let stream = search(&compressed, &raw, &trial, thorough, None, None)?;
        let file = rebuild(image, &layout, &stream);
        if result.as_ref().is_some_and(|b| b.len() <= file.len()) {
            continue;
        }
        let actual = Image::parse(&file)?;
        let decoded_layout = source_layout(&actual)?;
        let parsed = &actual.streams[0];
        let verified = Samples::decode(
            parsed.header,
            &inflate(&actual.compressed(parsed), parsed.header.passes()?.1)?,
            actual.palette,
        )?;
        if equal(&src, original, &decoded_layout, &verified) {
            result = Some(file);
        }
    }
    Ok(result)
}

pub(super) fn indexed_candidate(
    image: &Image<'_>,
    palette: &[[u8; 4]],
    indices: &[u8],
    thorough: bool,
) -> Result<Vec<u8>, String> {
    let depth = match palette.len() {
        1..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        17..=256 => 8,
        _ => return Err("Invalid candidate palette".into()),
    };
    let h = Header {
        depth,
        color: 3,
        ..image.header
    };
    let (passes, _) = h.passes()?;
    let stride = passes[0].bytes;
    let mut bytes = vec![0; stride * h.height];
    for (row, packed) in indices
        .chunks_exact(h.width)
        .zip(bytes.chunks_exact_mut(stride))
    {
        for (x, &index) in row.iter().enumerate() {
            let bit = x * usize::from(depth);
            packed[bit / 8] |= index << (8 - usize::from(depth) - bit % 8);
        }
    }
    let samples = Samples {
        bytes,
        passes,
        header: h,
    };
    let raw = samples.filtered(Strategy::Fixed(0));
    let compressed = compress_to_vec_zlib(&raw, 8);
    let selected = search(&compressed, &raw, &samples, thorough, None, None)?;
    let layout = Layout {
        h,
        key: None,
        palette: palette
            .iter()
            .map(|p| p.map(|c| u16::from(c) * 257))
            .collect(),
    };
    Ok(rebuild(image, &layout, &selected))
}
