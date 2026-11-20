//! Opt-in indexed PNG quantization with exact opacity and measured RGB bounds.
use super::{
    filters::{Samples, sample},
    format::Image,
    inflate,
    reduce::indexed_candidate,
};
use exoquant::{
    ColorCount, ColorMap, Colorf,
    optimizer::{KMeans, Optimizer},
};
use std::collections::{HashMap, HashSet};

type Pixel = [u8; 4];
type IndexTable = [Vec<u8>; 256];
const RGB_BINS: usize = 1 << 15;
const MAX_PIXELS: usize = 12_000_000;
const MAX_BINS: usize = 131_072;

fn compatible_icc(data: &[u8]) -> bool {
    let Some(n) = data.iter().position(|&b| b == 0) else {
        return false;
    };
    if !(1..=79).contains(&n) || data.get(n + 1) != Some(&0) {
        return false;
    }
    let Ok(profile) =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data[n + 2..], 4 << 20)
    else {
        return false;
    };
    if profile.len() < 132
        || u32::from_be_bytes(profile[..4].try_into().unwrap()) as usize != profile.len()
        || &profile[16..20] != b"RGB "
        || &profile[36..40] != b"acsp"
    {
        return false;
    }
    let word = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap()) as usize;
    let count = word(&profile[128..132]);
    if count == 0 || count > (profile.len() - 132) / 12 {
        return false;
    }
    let end = 132 + 12 * count;
    profile[132..end].as_chunks::<12>().0.iter().all(|tag| {
        let offset = word(&tag[4..8]);
        let len = word(&tag[8..12]);
        offset >= end && len >= 8 && offset.checked_add(len).is_some_and(|n| n <= profile.len())
    })
}

fn eligible(image: &Image<'_>) -> bool {
    let h = image.header;
    if h.interlaced
        || h.depth != 8
        || !matches!(h.color, 0 | 2 | 4 | 6)
        || h.width.checked_mul(h.height).is_none_or(|n| n > MAX_PIXELS)
        || image.preserve_entire_file
        || image.streams.len() != 1
    {
        return false;
    }
    let has_srgb = image.chunks.iter().any(|c| &c.kind == b"sRGB");
    let mut before_data = true;
    image.chunks.iter().all(|c| match &c.kind {
        b"IDAT" => {
            before_data = false;
            true
        }
        b"IHDR" | b"IEND" | b"tRNS" => true,
        b"tEXt" | b"iTXt" | b"zTXt" => c
            .data
            .iter()
            .position(|&b| b == 0)
            .is_some_and(|n| (1..=79).contains(&n)),
        b"pHYs" => before_data && c.data.len() == 9 && c.data[8] <= 1,
        b"tIME" => c.data.len() == 7,
        b"gAMA" => before_data && c.data.len() == 4 && c.data != [0, 0, 0, 0],
        b"cHRM" => before_data && c.data.len() == 32,
        b"sRGB" => before_data && matches!(c.data, [0..=3]),
        b"iCCP" => before_data && matches!(h.color, 2 | 6) && !has_srgb && compatible_icc(c.data),
        _ => false,
    })
}

fn samples(image: &Image<'_>) -> Result<Samples, String> {
    let raw = inflate(
        &image.compressed(&image.streams[0]),
        image.header.passes()?.1,
    )?;
    Samples::decode(image.header, &raw, image.palette)
}

fn rgba(image: &Image<'_>) -> Result<Vec<Pixel>, String> {
    let decoded = samples(image)?;
    let key = image
        .chunks
        .iter()
        .find(|c| &c.kind == b"tRNS")
        .map(|c| match c.data {
            [0, y] => [*y, *y, *y],
            [0, r, 0, g, 0, b] => [*r, *g, *b],
            _ => unreachable!("checked 8-bit transparency key"),
        });
    Ok(decoded
        .bytes
        .chunks_exact(image.header.channels())
        .map(|p| {
            let (rgb, a) = match p {
                [y] => ([*y; 3], 255),
                [y, a] => ([*y; 3], *a),
                [r, g, b] => ([*r, *g, *b], 255),
                [r, g, b, a] => ([*r, *g, *b], *a),
                _ => unreachable!(),
            };
            [rgb[0], rgb[1], rgb[2], if Some(rgb) == key { 0 } else { a }]
        })
        .collect())
}

fn key([r, g, b, a]: Pixel) -> u32 {
    (u32::from(a) << 15) | (u32::from(r >> 3) << 10) | (u32::from(g >> 3) << 5) | u32::from(b >> 3)
}

#[derive(Default, Clone)]
struct Moments {
    count: usize,
    sums: [u64; 3],
}

impl Moments {
    fn rgb(&self) -> [u8; 3] {
        let n = self.count as u64;
        self.sums.map(|s| ((s + n / 2) / n) as u8)
    }
}

// Oklab coordinates of encoded RGB interpreted as sRGB for palette selection.
// The output retains the source profile. Acceptance is measured in the stored samples.
fn perceptual([r, g, b]: [u8; 3], linear: &[f64; 256]) -> Colorf {
    let (r, g, b) = (
        linear[usize::from(r)],
        linear[usize::from(g)],
        linear[usize::from(b)],
    );
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    Colorf {
        r: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        g: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
        a: 0.0,
    }
}

struct Bin {
    key: u32,
    stats: Moments,
    point: Colorf,
    weight: f64,
}
struct Group {
    alpha: u8,
    bins: Vec<Bin>,
    weight: f64,
}

fn histogram(pixels: &[Pixel], linear: &[f64; 256]) -> Option<Vec<Group>> {
    let mut hist = HashMap::<u32, Moments>::new();
    for &p in pixels {
        let entry = hist.entry(key(p)).or_default();
        entry.count += 1;
        for (sum, value) in entry.sums.iter_mut().zip(p) {
            *sum += u64::from(value);
        }
        if hist.len() > MAX_BINS {
            return None;
        }
    }
    let mut bins: Vec<_> = hist.into_iter().collect();
    bins.sort_unstable_by_key(|b| b.0);
    let mut groups: Vec<Group> = Vec::new();
    for (key, stats) in bins {
        let alpha = (key >> 15) as u8;
        if groups.last().is_none_or(|g| g.alpha != alpha) {
            groups.push(Group {
                alpha,
                bins: Vec::new(),
                weight: 0.0,
            });
        }
        let group = groups.last_mut().unwrap();
        // Sum of square-root frequencies rewards color diversity and area.
        let weight = (stats.count as f64).sqrt();
        group.weight += weight;
        group.bins.push(Bin {
            key,
            point: perceptual(stats.rgb(), linear),
            stats,
            weight,
        });
    }
    Some(groups)
}

fn allocate(groups: &[Group], budget: usize) -> Option<Vec<usize>> {
    if groups.len() > budget {
        return None;
    }
    let mut counts = vec![1; groups.len()];
    for _ in groups.len()..budget {
        let winner = groups
            .iter()
            .enumerate()
            .filter(|(i, g)| counts[*i] < g.bins.len())
            .max_by(|(i, a), (j, b)| {
                (a.weight / (counts[*i] + 1) as f64)
                    .total_cmp(&(b.weight / (counts[*j] + 1) as f64))
            });
        let Some((idx, _)) = winner else {
            break;
        };
        counts[idx] += 1;
    }
    Some(counts)
}

fn centers(group: &Group, n: usize, iterations: usize) -> Vec<[u8; 3]> {
    let bins = &group.bins;
    if n == bins.len() {
        return bins.iter().map(|b| b.stats.rgb()).collect();
    }
    let hist: Vec<_> = bins
        .iter()
        .map(|b| ColorCount {
            color: b.point,
            count: b.stats.count,
        })
        .collect();
    let dominant = bins.iter().max_by_key(|b| b.stats.count).unwrap();
    let mut palette = vec![dominant.point];
    let mut distances = vec![f64::MAX; bins.len()];
    while palette.len() < n {
        let last = *palette.last().unwrap();
        let mut winner = (0, 0.0);
        for (i, b) in bins.iter().enumerate() {
            let d = b.point - last;
            distances[i] = distances[i].min(d.dot(&d));
            let score = distances[i] * b.weight;
            if score > winner.1 {
                winner = (i, score);
            }
        }
        if winner.1 == 0.0 {
            break;
        }
        palette.push(bins[winner.0].point);
    }
    for _ in 0..iterations {
        palette = KMeans.step(palette, &hist);
    }
    let mut sums = vec![Moments::default(); palette.len()];
    let map = ColorMap::from_float_colors(palette);
    for b in bins {
        let bucket = &mut sums[map.find_nearest(b.point)];
        bucket.count += b.stats.count;
        for (dst, src) in bucket.sums.iter_mut().zip(b.stats.sums) {
            *dst += src;
        }
    }
    sums.iter()
        .filter(|b| b.count > 0)
        .map(Moments::rgb)
        .collect()
}

fn quantize(
    groups: &[Group],
    budget: usize,
    linear: &[f64; 256],
    iterations: usize,
) -> Option<(Vec<Pixel>, IndexTable)> {
    let counts = allocate(groups, budget)?;
    let mut palette = Vec::new();
    let mut lookup = std::array::from_fn(|_| Vec::new());
    for (group, n) in groups.iter().zip(counts) {
        let mut rgb = centers(group, n, iterations);
        rgb.sort_unstable_by_key(|&[r, g, b]| {
            (
                54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b),
                r,
                g,
                b,
            )
        });
        rgb.dedup();
        let map = ColorMap::from_float_colors(rgb.iter().map(|&c| perceptual(c, linear)).collect());
        let mut indices = vec![0; RGB_BINS];
        for b in &group.bins {
            indices[b.key as usize % RGB_BINS] = (palette.len() + map.find_nearest(b.point)) as u8;
        }
        lookup[usize::from(group.alpha)] = indices;
        palette.extend(rgb.into_iter().map(|[r, g, b]| [r, g, b, group.alpha]));
    }
    Some((palette, lookup))
}

/// Require at least one changed sample, exact alpha and exact hidden colors.
/// RGB RMSE² ≤20, luma RMSE² ≤12, at most 1% of pixels over 24 in any channel,
/// maximum channel error 64 and local 32×32 luma RMSE² ≤49.
fn quality(original: &[Pixel], mapped: impl IntoIterator<Item = Pixel>, width: usize) -> bool {
    let mut count = 0usize;
    let (mut rgb, mut luma, mut tail) = (0u64, 0u64, 0u64);
    let mut tiles = vec![(0u64, 0u64); width.div_ceil(32)];
    for p in mapped {
        let Some(src) = original.get(count) else {
            return false;
        };
        if src[3] != p[3] || (p[3] == 0 && *src != p) {
            return false;
        }
        let diff: [i32; 3] = std::array::from_fn(|i| i32::from(src[i]) - i32::from(p[i]));
        let max = diff.iter().map(|d| d.unsigned_abs()).max().unwrap();
        if max > 64 {
            return false;
        }
        tail += u64::from(max > 24);
        rgb += diff.iter().map(|v| (v * v) as u64).sum::<u64>();
        let y = 54 * diff[0] + 183 * diff[1] + 19 * diff[2];
        let square = i64::from(y).pow(2) as u64;
        luma += square;
        let x = count % width;
        let bucket = &mut tiles[x / 32];
        bucket.0 += square;
        bucket.1 += 1;
        count += 1;
        if x + 1 == width && ((count / width).is_multiple_of(32) || count == original.len()) {
            if tiles.iter().any(|(s, n)| *s > 49 * 65536 * n) {
                return false;
            }
            tiles.fill((0, 0));
        }
    }
    let n = count as u64;
    count == original.len()
        && rgb > 0
        && rgb <= 3 * 20 * n
        && luma <= 12 * 65536 * n
        && tail * 100 <= n
}

fn verify(source: &Image<'_>, original: &[Pixel], output: &[u8]) -> Result<bool, String> {
    let out = Image::parse(output)?;
    let h = out.header;
    if h.width != source.header.width
        || h.height != source.header.height
        || h.color != 3
        || h.interlaced
    {
        return Ok(false);
    }
    let ancillary =
        |kind: &[u8; 4]| !matches!(kind, b"IHDR" | b"PLTE" | b"IDAT" | b"tRNS" | b"IEND");
    let a = source
        .chunks
        .iter()
        .filter(|c| ancillary(&c.kind))
        .map(|c| (c.kind, c.data));
    let b = out
        .chunks
        .iter()
        .filter(|c| ancillary(&c.kind))
        .map(|c| (c.kind, c.data));
    if !a.eq(b) {
        return Ok(false);
    }
    let colors = out
        .chunks
        .iter()
        .find(|c| &c.kind == b"PLTE")
        .ok_or("Missing candidate palette")?;
    let alpha = out
        .chunks
        .iter()
        .find(|c| &c.kind == b"tRNS")
        .map_or(&[][..], |c| c.data);
    let table: Vec<_> = colors
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .map(|(i, p)| [p[0], p[1], p[2], alpha.get(i).copied().unwrap_or(255)])
        .collect();
    let decoded = samples(&out)?;
    let actual = decoded
        .bytes
        .chunks_exact(decoded.passes[0].bytes)
        .flat_map(|row| {
            let table = &table;
            (0..h.width).map(move |x| table[sample(row, x, h.depth)])
        });
    Ok(quality(original, actual, h.width))
}

/// Try an indexed PNG that is smaller than its source and meets fixed color-error limits.
/// Unsupported inputs and candidates outside those limits return None.
pub fn optimize_lossy(input: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let image = Image::parse(input)?;
    if !eligible(&image) {
        return Ok(None);
    }
    let original = rgba(&image)?;
    let gray = original.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
    // A later lossless optimization may represent our palette as RGB or gray.
    // Recognize those rasters too, without writing a proprietary marker.
    let threshold = if gray { 64 } else { 256 };
    let mut distinct = HashSet::new();
    if !original.iter().any(|&p| {
        distinct.insert(p);
        distinct.len() > threshold
    }) {
        return Ok(None);
    }

    let linear = std::array::from_fn(|i| {
        let x = i as f64 / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    });
    let Some(groups) = histogram(&original, &linear) else {
        return Ok(None);
    };
    let mut best: Option<Vec<u8>> = None;
    let budgets: &[usize] = if thorough {
        &[32, 64, 128, 192, 256]
    } else {
        &[64, 128, 256]
    };
    for &n in budgets {
        if gray && n > 64 {
            break;
        }
        let Some((palette, lookup)) = quantize(&groups, n, &linear, if thorough { 8 } else { 4 })
        else {
            continue;
        };
        if palette.len() > 64 && palette.iter().all(|p| p[0] == p[1] && p[1] == p[2]) {
            continue;
        }
        let indices: Vec<_> = original
            .iter()
            .map(|&p| lookup[usize::from(p[3])][key(p) as usize % RGB_BINS])
            .collect();
        if !quality(
            &original,
            indices.iter().map(|&i| palette[usize::from(i)]),
            image.header.width,
        ) {
            continue;
        }
        let candidate = indexed_candidate(&image, &palette, &indices, thorough)?;
        if candidate.len() < best.as_ref().map_or(input.len(), Vec::len)
            && verify(&image, &original, &candidate)?
        {
            best = Some(candidate);
        }
    }
    Ok(best)
}
