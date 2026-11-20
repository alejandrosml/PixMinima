//! Additional lossless searches for the opt-in maximum setting.
use super::{
    encode_zlib,
    filters::{Samples, Strategy},
    format::Image,
    inflate, keep_if_verified, regional,
};
use miniz_oxide::deflate::{
    compress_to_vec, compress_to_vec_zlib,
    core::{
        CompressionStrategy, CompressorOxide, TDEFLFlush, TDEFLStatus, compress_to_output,
        create_comp_flags_from_zip_params,
    },
};
use std::{collections::VecDeque, num::NonZeroU64};

struct Layout {
    size: usize,
    rows: Vec<u8>,
}

fn record(
    layouts: &mut Vec<Layout>,
    best: &mut Vec<u8>,
    data: &[u8],
    seed: &[u8],
    samples: &Samples,
) {
    let rows = samples.row_choices(data);
    if layouts.iter().any(|l| l.rows == rows) {
        return;
    }
    let compressed = compress_to_vec_zlib(data, 9);
    layouts.push(Layout {
        size: compressed.len(),
        rows,
    });
    // Reconstruct the raster, rather than trusting that a selector produced a
    // valid filter layout for these channels and the active Adam7 subdivision.
    keep_if_verified(best, compressed, seed, samples);
}

fn deflate_variant(data: &[u8], strategy: CompressionStrategy) -> Option<Vec<u8>> {
    let flags = create_comp_flags_from_zip_params(9, 1, strategy as i32);
    let mut encoder = CompressorOxide::new(flags);
    let mut output = Vec::new();
    let (status, n) = compress_to_output(&mut encoder, data, TDEFLFlush::Finish, |part| {
        output.extend_from_slice(part);
        true
    });
    (status == TDEFLStatus::Done && n == data.len()).then_some(output)
}

// Each branch starts from exactly the same LZ77 dictionary and pending
// Huffman block. Finishing the clone prices each row in the actual stream.
// Keep the winning row without flushing the encoder used for the next branch.
fn incremental(
    fixed: &[Vec<u8>],
    stride: usize,
    level: i32,
    strategy: CompressionStrategy,
) -> Option<Vec<u8>> {
    let size = fixed[0].len();
    let mut encoder =
        CompressorOxide::new(create_comp_flags_from_zip_params(level, 0, strategy as i32));
    let mut out = Vec::with_capacity(size);
    for start in (0..size).step_by(stride) {
        let mut best = usize::MAX;
        let mut chosen = 0;
        for (f, layout) in fixed.iter().enumerate() {
            let row = &layout[start..start + stride];
            let mut branch = encoder.clone();
            let mut bytes = 0;
            let (status, n) = compress_to_output(&mut branch, row, TDEFLFlush::Finish, |part| {
                bytes += part.len();
                true
            });
            if status != TDEFLStatus::Done || n != stride {
                return None;
            }
            if bytes < best {
                best = bytes;
                chosen = f;
            }
        }
        let row = &fixed[chosen][start..start + stride];
        let (status, n) = compress_to_output(&mut encoder, row, TDEFLFlush::None, |_| true);
        if status != TDEFLStatus::Okay || n != stride {
            return None;
        }
        out.extend_from_slice(row);
    }
    Some(out)
}

// Each trial changes one complete row. Its dictionary context and the following
// rows stay fixed so the score includes matches that *start on later rows*.
// Scanning in alternating directions exposes dependencies on both neighbors.
fn coordinate_sweep(seed: &[u8], fixed: &[Vec<u8>], stride: usize, reverse: bool) -> Vec<u8> {
    let mut out = seed.to_vec();
    let count = out.len() / stride;
    let neighbors = (8192 / stride).clamp(1, 4);
    let mut probe = Vec::new();
    for step in 0..count {
        let row = if reverse { count - 1 - step } else { step };
        let start = row.saturating_sub(neighbors) * stride;
        let end = ((row + neighbors + 1).min(count)) * stride;
        let index = row * stride;
        let local = index - start;
        probe.clear();
        probe.extend_from_slice(&out[start..end]);
        let initial = out[index] as usize;
        let mut id = initial;
        let mut size = compress_to_vec(&probe, 3).len();
        for (f, layout) in fixed.iter().enumerate() {
            if f == initial {
                continue;
            }
            probe[local..local + stride].copy_from_slice(&layout[index..index + stride]);
            let n = compress_to_vec(&probe, 3).len();
            if n < size {
                id = f;
                size = n;
            }
        }
        out[index..index + stride].copy_from_slice(&fixed[id][index..index + stride]);
    }
    out
}

// For one Laplace-smoothed global symbol distribution, choose the cheapest
// filter per row under that distribution. Re-estimating the distribution from
// the new layout adapts Huffman costs without fabricating any pixel values.
fn global_huffman(seed: &[u8], fixed: &[Vec<u8>], stride: usize) -> Vec<u8> {
    let mut frequencies = [1u32; 256];
    for &b in seed {
        frequencies[b as usize] += 1;
    }
    let total = (seed.len() + 256) as f64;
    let cost = frequencies.map(|v| ((total / f64::from(v)).log2() * 1024.0).round() as u32);
    let mut out = Vec::with_capacity(seed.len());
    for start in (0..seed.len()).step_by(stride) {
        let best = fixed
            .iter()
            .map(|data| &data[start..start + stride])
            .min_by_key(|row| {
                row.iter()
                    .map(|&v| u64::from(cost[v as usize]))
                    .sum::<u64>()
            })
            .unwrap();
        out.extend_from_slice(best);
    }
    out
}

// Coordinate descent over successively shorter bands. Score the entire stream
// to include cross-band LZ77 matches and shared Huffman blocks.
fn band_search(leaders: &[Vec<u8>], stride: usize) -> Option<Vec<u8>> {
    let mut best = leaders.first()?.clone();
    let rows = best.len() / stride;
    if rows < 16 || leaders.len() < 2 {
        return None;
    }
    let mut size = compress_to_vec(&best, 7).len();
    let mut saved = Vec::new();
    for division in [4, 8] {
        let band = rows.div_ceil(division) * stride;
        for start in (0..best.len()).step_by(band) {
            let end = (start + band).min(best.len());
            saved.clear();
            saved.extend_from_slice(&best[start..end]);
            let mut winner = None;
            for (idx, layout) in leaders.iter().enumerate() {
                let part = &layout[start..end];
                if part == saved || leaders[..idx].iter().any(|p| &p[start..end] == part) {
                    continue;
                }
                best[start..end].copy_from_slice(part);
                let trial = compress_to_vec(&best, 7).len();
                if trial < size {
                    size = trial;
                    winner = Some(idx);
                }
            }
            match winner {
                Some(idx) => best[start..end].copy_from_slice(&leaders[idx][start..end]),
                None => best[start..end].copy_from_slice(&saved),
            }
        }
    }
    (!leaders.contains(&best)).then_some(best)
}

fn options(iterations: u64, patience: u64, splits: u16, enhanced: bool) -> zenzop::Options {
    let mut options = zenzop::Options::default();
    options.iteration_count = NonZeroU64::new(iterations).unwrap();
    options.iterations_without_improvement = NonZeroU64::new(patience).unwrap();
    options.maximum_block_splits = splits;
    options.enhanced = enhanced;
    options
}

fn frame(original: &[u8], seed: &[u8], samples: &Samples) -> Vec<u8> {
    let mut best = original.to_vec();
    let bytes = seed.len();
    let iterations = if bytes <= 256 * 1024 {
        48
    } else if bytes <= 4 * 1024 * 1024 {
        30
    } else {
        14
    };
    let enhanced = options(iterations, 10, 40, true);
    if let Ok(stream) = encode_zlib(seed, enhanced, 1024 * 1024) {
        keep_if_verified(&mut best, stream, seed, samples);
    }
    let row_count = samples.passes.iter().map(|p| p.rows).sum::<usize>();
    // Store rows, not full images, for each prospective layout. Tall narrow
    // images still receive the enhanced encoding of their prior winning layout.
    if row_count > 512 * 1024 {
        return best;
    }
    let mut layouts = Vec::new();
    record(&mut layouts, &mut best, seed, seed, samples);
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
        Strategy::ContextEntropy,
    ] {
        record(&mut layouts, &mut best, &samples.filtered(s), seed, samples);
    }
    if bytes <= 2 * 1024 * 1024 && row_count <= 4096 {
        record(
            &mut layouts,
            &mut best,
            &samples.filtered(Strategy::DeflateEstimate),
            seed,
            samples,
        );
    }
    let mut band_rows = None;
    if !samples.header.interlaced && bytes <= 4 * 1024 * 1024 && row_count <= 4096 {
        let stride = samples.passes[0].bytes + 1;
        let fixed: Vec<_> = (0..=4)
            .map(|f| samples.filtered(Strategy::Fixed(f)))
            .collect();
        for (level, mode) in [
            (3, CompressionStrategy::Filtered),
            (8, CompressionStrategy::Default),
        ] {
            if let Some(data) = incremental(&fixed, stride, level, mode) {
                record(&mut layouts, &mut best, &data, seed, samples);
            }
        }
        let entropy = global_huffman(seed, &fixed, stride);
        record(&mut layouts, &mut best, &entropy, seed, samples);
        let entropy2 = global_huffman(&entropy, &fixed, stride);
        record(&mut layouts, &mut best, &entropy2, seed, samples);
        let forward = coordinate_sweep(seed, &fixed, stride, false);
        record(&mut layouts, &mut best, &forward, seed, samples);
        let backward = coordinate_sweep(&forward, &fixed, stride, true);
        record(&mut layouts, &mut best, &backward, seed, samples);
        layouts.sort_by_key(|l| l.size);
        let leaders: Vec<_> = layouts
            .iter()
            .take(6)
            .filter_map(|l| samples.filtered_choices(&l.rows))
            .collect();
        if (32 * 1024..=2 * 1024 * 1024).contains(&bytes)
            && let Some(mixed) = band_search(&leaders, stride)
        {
            // Keep this independently generated candidate outside the ranking so
            // that it cannot displace a well-compressing individual filter layout.
            band_rows = Some(samples.row_choices(&mixed));
        }
        if let Some(mixed) = regional(&leaders, stride) {
            record(&mut layouts, &mut best, &mixed, seed, samples);
        }
    }
    layouts.sort_by_key(|l| l.size);
    let finalists = if bytes <= 256 * 1024 {
        8
    } else if bytes <= 4 * 1024 * 1024 {
        6
    } else {
        3
    };
    let baseline_rows = samples.row_choices(seed);
    let top: Vec<_> = layouts.iter().take(finalists).map(|l| &l.rows).collect();
    let band = band_rows.as_ref().filter(|rows| !top.contains(rows));
    for (rank, rows) in top.into_iter().chain(band).enumerate() {
        let Some(data) = samples.filtered_choices(rows) else {
            continue;
        };
        for mode in [
            CompressionStrategy::Filtered,
            CompressionStrategy::RLE,
            CompressionStrategy::HuffmanOnly,
        ] {
            if let Some(stream) = deflate_variant(&data, mode) {
                keep_if_verified(&mut best, stream, seed, samples);
            }
        }
        if *rows != baseline_rows
            && let Ok(stream) = encode_zlib(&data, enhanced, 1024 * 1024)
        {
            keep_if_verified(&mut best, stream, seed, samples);
        }
        if rank < 2 || Some(rows) == band {
            // The upstream and enhanced parsers take different LZ77/Huffman
            // paths. Smaller chunk boundaries also change the partition search.
            if let Ok(stream) = encode_zlib(&data, options(iterations, 10, 32, false), 256 * 1024) {
                keep_if_verified(&mut best, stream, seed, samples);
            }
        }
    }
    best
}

pub(super) fn improve(input: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let image = Image::parse(input)?;
    if image.preserve_entire_file {
        return Ok(None);
    }
    let mut streams: Vec<Vec<u8>> = Vec::with_capacity(image.streams.len());
    let mut recent = VecDeque::<(u32, usize)>::new();
    for (i, s) in image.streams.iter().enumerate() {
        let original = image.compressed(s);
        if let Some(id) = image.repeat_frame(i, &original, &mut recent) {
            streams.push(streams[id].clone());
            continue;
        }
        let raw = inflate(&original, s.header.passes()?.1)?;
        let samples = Samples::decode(s.header, &raw, image.palette)?;
        streams.push(frame(&original, &raw, &samples));
    }
    let candidate = image.rebuild(&streams, input.len());
    Ok((candidate.len() < input.len()).then_some(candidate))
}
