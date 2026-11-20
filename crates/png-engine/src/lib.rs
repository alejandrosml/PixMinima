#![forbid(unsafe_code)]
//! Lossless PNG/APNG optimization. Each accepted candidate must decode to every
//! original sample value, including colors beneath transparent pixels. Native-layout
//! recompression also preserves all packed scanline padding bits.

mod filters;
mod format;
mod lossy;
mod maximum;
pub use lossy::optimize_lossy;
mod reduce;

use filters::{Samples, Strategy};
use format::Image;
use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::{
    TINFLStatus,
    core::{DecompressorOxide, decompress, inflate_flags::*},
};
use std::borrow::Cow;
use std::num::NonZeroU64;
use zenzop::Write;

/// Optional accelerator for the PNG Sub encoding of packed, non-interlaced RGBA8.
/// Input and returned bytes are contiguous RGBA without filter-prefix bytes.
/// Coordinates are in pixels. Return None to let the CPU do the work.
pub type SubFilter<'a> = dyn Fn(&[u8], usize, usize) -> Option<Vec<u8>> + 'a;

/// Row selectors receive packed RGBA8 pixels and their width and height. Return one
/// filter index (0 through 4) per row, or None to run the exact CPU MinSum search.
pub type RowSelector<'a> = dyn Fn(&[u8], usize, usize) -> Option<Vec<u8>> + 'a;

fn inflate(data: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; size + 1];
    let (status, consumed, n) = decompress(
        &mut DecompressorOxide::new(),
        data,
        &mut out,
        0,
        TINFL_FLAG_PARSE_ZLIB_HEADER
            | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
            | TINFL_FLAG_COMPUTE_ADLER32,
    );
    if status != TINFLStatus::Done || consumed != data.len() || n != size {
        return Err("Invalid PNG zlib stream, checksum or scanline length".into());
    }
    out.truncate(n);
    Ok(out)
}

/// Compress arbitrary bytes into a zlib stream using the thorough Rust DEFLATE
/// search. Container writers verify their own reconstructed samples or payload.
pub fn compress_zlib_thorough(data: &[u8]) -> Result<Vec<u8>, String> {
    let iterations = if data.len() <= 256 * 1024 {
        15
    } else if data.len() <= 4 * 1024 * 1024 {
        8
    } else {
        4
    };
    let mut options = zenzop::Options::default();
    options.enhanced = true;
    options.iteration_count = NonZeroU64::new(iterations).unwrap();
    options.iterations_without_improvement = NonZeroU64::new(4).unwrap();
    encode_zlib(data, options, 1024 * 1024)
}

fn encode_zlib(
    data: &[u8],
    options: zenzop::Options,
    block_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut out = vec![0x78, 0xda];
    // Limit the size of a Zopfli search block; the encoder retains a sliding history.
    let mut encoder = zenzop::DeflateEncoder::new(options, &mut out);
    for block in data.chunks(block_bytes) {
        encoder.write_all(block).map_err(|e| e.to_string())?;
    }
    encoder.finish().map_err(|e| e.to_string())?;
    out.extend_from_slice(&adler2::adler32_slice(data).to_be_bytes());
    Ok(out)
}

fn external_sub(samples: &Samples, gpu: Option<&SubFilter<'_>>) -> Option<Vec<u8>> {
    let h = samples.header;
    if h.color != 6 || h.depth != 8 || h.interlaced {
        return None;
    }
    let accelerated = gpu?(&samples.bytes, h.width, h.height)?;
    if accelerated.len() != samples.bytes.len() {
        return None;
    }
    let mut prefixed = Vec::with_capacity(accelerated.len() + h.height);
    for row in accelerated.chunks_exact(h.width * 4) {
        prefixed.push(1);
        prefixed.extend_from_slice(row);
    }
    // A faulty accelerator can never affect the result; continue on CPU if it mismatches.
    let check = Samples::decode(h, &prefixed, None).ok()?;
    (check.bytes == samples.bytes).then_some(prefixed)
}

fn keep_if_verified(
    best: &mut Vec<u8>,
    proposed: Vec<u8>,
    expected_filtered: &[u8],
    samples: &Samples,
) {
    if proposed.len() >= best.len() {
        return;
    }
    let Ok(decoded) = inflate(&proposed, expected_filtered.len()) else {
        return;
    };
    if decoded == expected_filtered
        || Samples::decode(samples.header, &decoded, None).is_ok_and(|s| s.bytes == samples.bytes)
    {
        *best = proposed;
    }
}

// Rank regional layouts with a shared recent DEFLATE dictionary. Different
// regions can favor different filters even when no single strategy wins globally.
fn regional(layouts: &[Vec<u8>], line_bytes: usize) -> Option<Vec<u8>> {
    let size = layouts.first()?.len();
    let rows_per_block = (16 * 1024 / line_bytes).max(4);
    let block_size = line_bytes * rows_per_block;
    if size <= block_size {
        return None;
    }
    let mut out = Vec::with_capacity(size);
    let mut probe = Vec::new();
    for from in (0..size).step_by(block_size) {
        let to = (from + block_size).min(size);
        let history = out.len().saturating_sub(4096);
        let context = out.len() - history;
        probe.clear();
        probe.extend_from_slice(&out[history..]);
        let mut best_len = usize::MAX;
        let mut chosen = &layouts[0][from..to];
        for (i, raw) in layouts.iter().enumerate() {
            let trial = &raw[from..to];
            if layouts[..i].iter().any(|p| &p[from..to] == trial) {
                continue;
            }
            probe.truncate(context);
            probe.extend_from_slice(trial);
            let n = miniz_oxide::deflate::compress_to_vec(&probe, 3).len();
            if n < best_len {
                chosen = trial;
                best_len = n;
            }
        }
        out.extend_from_slice(chosen);
    }
    if layouts.contains(&out) {
        return None;
    }
    Some(out)
}

struct FilterTrial {
    size: usize,
    strategy: Strategy,
    signature: u32,
    rows: Option<Vec<u8>>,
}

fn search(
    original: &[u8],
    filtered: &[u8],
    samples: &Samples,
    thorough: bool,
    gpu: Option<&SubFilter<'_>>,
    selector: Option<&RowSelector<'_>>,
) -> Result<Vec<u8>, String> {
    let external = external_sub(samples, gpu);
    let h = samples.header;
    let chosen = selector
        .filter(|_| h.color == 6 && h.depth == 8 && !h.interlaced)
        .and_then(|choose| choose(&samples.bytes, h.width, h.height))
        .and_then(|choices| samples.filtered_choices(&choices));
    let min_sum = chosen.unwrap_or_else(|| samples.filtered(Strategy::MinSum));
    let mut strategies = vec![Strategy::Original, Strategy::MinSum, Strategy::Fixed(0)];
    if thorough {
        strategies.extend([
            Strategy::Fixed(1),
            Strategy::Fixed(2),
            Strategy::Fixed(3),
            Strategy::Fixed(4),
            Strategy::Runs,
            Strategy::Entropy,
            Strategy::PairEntropy,
            Strategy::ContextEntropy,
        ]);
        if filtered.len() <= 2 * 1024 * 1024
            && samples.passes.iter().map(|p| p.rows).sum::<usize>() <= 4096
        {
            strategies.push(Strategy::DeflateEstimate);
        }
    } else if external.is_some() {
        strategies.push(Strategy::Fixed(1));
    }
    if external.is_some() {
        for s in &mut strategies {
            if *s == Strategy::Fixed(1) {
                *s = Strategy::Accelerated;
            }
        }
    }
    let candidate = |s| -> Cow<'_, [u8]> {
        match s {
            Strategy::Original => Cow::Borrowed(filtered),
            Strategy::MinSum => Cow::Borrowed(&min_sum),
            Strategy::Accelerated => Cow::Borrowed(external.as_ref().unwrap()),
            _ => Cow::Owned(samples.filtered(s)),
        }
    };
    let mut best = original.to_vec();
    let mut ranked = Vec::new();
    // A given raw scanline and filter ID uniquely determine its bytes. Save
    // just the IDs, so a finalist never has to rerun a per-row DEFLATE trial.
    // Bound storage even for very tall, narrow input images.
    let save_rows = samples.passes.iter().map(|p| p.rows).sum::<usize>() <= 1024 * 1024;
    for s in strategies {
        let trial = candidate(s);
        let rows = save_rows.then(|| samples.row_choices(&trial));
        let signature = adler2::adler32_slice(rows.as_deref().unwrap_or(trial.as_ref()));
        if ranked.iter().any(|previous: &FilterTrial| {
            previous.signature == signature
                && if save_rows {
                    previous.rows == rows
                } else {
                    trial == candidate(previous.strategy)
                }
        }) {
            continue;
        }
        let compressed = compress_to_vec_zlib(&trial, if thorough { 9 } else { 6 });
        let size = compressed.len();
        keep_if_verified(&mut best, compressed, filtered, samples);
        ranked.push(FilterTrial {
            size,
            strategy: s,
            signature,
            rows,
        });
    }
    if thorough {
        ranked.sort_by_key(|entry| entry.size);
        let finalists = if filtered.len() <= 2 * 1024 * 1024 {
            3
        } else if filtered.len() <= 8 * 1024 * 1024 {
            2
        } else {
            1
        };
        if !h.interlaced && (16 * 1024..=2 * 1024 * 1024).contains(&filtered.len()) {
            let layouts: Vec<_> = ranked
                .iter()
                .take(5)
                .filter_map(|v| {
                    v.rows
                        .as_ref()
                        .and_then(|rows| samples.filtered_choices(rows))
                })
                .collect();
            if let Some(raw) = regional(&layouts, samples.passes[0].bytes + 1) {
                let compressed = compress_to_vec_zlib(&raw, 9);
                let promising = ranked
                    .get(finalists - 1)
                    .is_none_or(|v| compressed.len() <= v.size);
                keep_if_verified(&mut best, compressed, filtered, samples);
                if promising && let Ok(compressed) = compress_zlib_thorough(&raw) {
                    keep_if_verified(&mut best, compressed, filtered, samples);
                }
            }
        }
        let mut regular = 0;
        for (
            rank,
            FilterTrial {
                strategy: s, rows, ..
            },
        ) in ranked.iter().enumerate()
        {
            if *s == Strategy::ContextEntropy {
                if rank >= finalists {
                    continue;
                }
            } else {
                if regular == finalists {
                    continue;
                }
                regular += 1;
            }
            let trial = match s {
                Strategy::Original | Strategy::MinSum | Strategy::Accelerated => candidate(*s),
                _ => rows
                    .as_ref()
                    .and_then(|v| samples.filtered_choices(v))
                    .map(Cow::Owned)
                    .unwrap_or_else(|| candidate(*s)),
            };
            if let Ok(compressed) = compress_zlib_thorough(&trial) {
                keep_if_verified(&mut best, compressed, filtered, samples);
            }
        }
    }
    Ok(best)
}

/// Optimize a PNG or APNG and retain the original when every valid candidate is larger.
pub fn optimize(input: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    optimize_with_sub(input, thorough, None)
}

/// As `optimize`, with an optional externally computed Sub-filter candidate.
pub fn optimize_with_sub(
    input: &[u8],
    thorough: bool,
    gpu: Option<&SubFilter<'_>>,
) -> Result<Vec<u8>, String> {
    optimize_impl(input, thorough, gpu, None, true)
}

/// As `optimize`, using an optional accelerator for per-row MinSum filter selection.
/// The CPU applies the reported filters and verifies every final compressed frame.
pub fn optimize_with_rows(
    input: &[u8],
    thorough: bool,
    selector: Option<&RowSelector<'_>>,
) -> Result<Vec<u8>, String> {
    optimize_impl(input, thorough, None, selector, true)
}

/// Recompress native frame samples without changing the layout or the 13 IHDR
/// bytes. This keeps a containing icon's pixel-format directory entry valid.
pub fn optimize_preserving_header(input: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    let output = optimize_impl(input, thorough, None, None, false)?;
    // IHDR is the validated first chunk and is always exactly 13 bytes long.
    if output.get(16..29) != input.get(16..29) {
        return Ok(input.to_vec());
    }
    Ok(output)
}

/// Run the thorough optimizer first, then search additional frame layouts and
/// DEFLATE encodings. The completed thorough output is always retained as a candidate.
pub fn optimize_maximum_with_rows(
    input: &[u8],
    selector: Option<&RowSelector<'_>>,
) -> Result<Vec<u8>, String> {
    let best = optimize_with_rows(input, true, selector)?;
    // The baseline has already been fully validated; an optional search that
    // cannot validate its result must never turn that success into an error.
    Ok(maximum::improve(&best)
        .ok()
        .flatten()
        .filter(|v| v.len() < best.len())
        .unwrap_or(best))
}

fn optimize_impl(
    input: &[u8],
    thorough: bool,
    gpu: Option<&SubFilter<'_>>,
    selector: Option<&RowSelector<'_>>,
    allow_layout_reduction: bool,
) -> Result<Vec<u8>, String> {
    let image = Image::parse(input)?;
    if image.preserve_entire_file {
        for stream in &image.streams {
            let filtered = inflate(&image.compressed(stream), stream.header.passes()?.1)?;
            Samples::decode(stream.header, &filtered, image.palette)?;
        }
        return Ok(input.to_vec());
    }
    let mut streams: Vec<Vec<u8>> = Vec::with_capacity(image.streams.len());
    let mut recent = std::collections::VecDeque::<(u32, usize)>::new();
    let mut reduced = None;
    for (index, stream) in image.streams.iter().enumerate() {
        let original = image.compressed(stream);
        if let Some(id) = image.repeat_frame(index, &original, &mut recent) {
            streams.push(streams[id].clone());
            continue;
        }
        let raw = inflate(&original, stream.header.passes()?.1)?;
        let samples = Samples::decode(stream.header, &raw, image.palette)?;
        let best = search(&original, &raw, &samples, thorough, gpu, selector)?;
        if allow_layout_reduction && image.streams.len() == 1 {
            // Failure to validate an optional layout only disqualifies that candidate.
            // Input validation happened above, and native candidates remain available.
            reduced = reduce::candidate(&image, &samples, thorough).ok().flatten();
        }
        streams.push(best);
    }
    let mut output = image.rebuild(&streams, input.len());
    if let Some(candidate) = reduced.filter(|c| c.len() < output.len()) {
        output = candidate;
    }
    Ok(if output.len() < input.len() {
        output
    } else {
        input.to_vec()
    })
}

#[cfg(test)]
mod tests;
