use crate::format::{Header, Pass};

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum Strategy {
    Original,
    Fixed(u8),
    MinSum,
    Runs,
    Entropy,
    PairEntropy,
    ContextEntropy,
    DeflateEstimate,
    Accelerated,
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i32::from(a), i32::from(b), i32::from(c));
    let pa = (b - c).abs();
    let pb = (a - c).abs();
    let pc = (a + b - 2 * c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}
fn predictor(filter: u8, a: u8, b: u8, c: u8) -> u8 {
    match filter {
        0 => 0,
        1 => a,
        2 => b,
        3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
        4 => paeth(a, b, c),
        _ => unreachable!(),
    }
}

pub(crate) struct Samples {
    pub bytes: Vec<u8>,
    pub passes: Vec<Pass>,
    pub header: Header,
}

impl Samples {
    pub fn decode(header: Header, filtered: &[u8], palette: Option<usize>) -> Result<Self, String> {
        let (passes, size) = header.passes()?;
        if size != filtered.len() {
            return Err("PNG pixel stream has the wrong size".into());
        }
        let mut bytes = vec![0; size - passes.iter().map(|p| p.rows).sum::<usize>()];
        let mut source = 0;
        let mut dest = 0;
        let bpp = header.bpp();
        for pass in &passes {
            for y in 0..pass.rows {
                let f = filtered[source];
                if f > 4 {
                    return Err("Invalid PNG row filter".into());
                }
                let src = &filtered[source + 1..source + 1 + pass.bytes];
                let (above, tail) = bytes.split_at_mut(dest);
                let row = &mut tail[..pass.bytes];
                if f == 0 {
                    row.copy_from_slice(src);
                } else {
                    let prev = if y == 0 {
                        &[][..]
                    } else {
                        &above[above.len() - pass.bytes..]
                    };
                    for x in 0..row.len() {
                        let left = x.checked_sub(bpp);
                        let a = left.map_or(0, |i| row[i]);
                        let b = prev.get(x).copied().unwrap_or(0);
                        let c = left.and_then(|i| prev.get(i).copied()).unwrap_or(0);
                        row[x] = src[x].wrapping_add(predictor(f, a, b, c));
                    }
                }
                if let Some(n) = palette.filter(|_| header.color == 3)
                    && n < (1usize << header.depth)
                    && (0..pass.width).any(|x| sample(row, x, header.depth) >= n)
                {
                    return Err("PNG pixel references an absent palette entry".into());
                }
                dest += pass.bytes;
                source += 1 + pass.bytes;
            }
        }
        Ok(Self {
            bytes,
            passes,
            header,
        })
    }

    pub fn row_choices(&self, filtered: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.passes.iter().map(|p| p.rows).sum());
        let mut data = filtered;
        for pass in &self.passes {
            let (pass_data, rest) = data.split_at((pass.bytes + 1) * pass.rows);
            result.extend(pass_data.iter().step_by(pass.bytes + 1));
            data = rest;
        }
        result
    }
    pub fn filtered_choices(&self, choices: &[u8]) -> Option<Vec<u8>> {
        if choices.len() != self.passes.iter().map(|p| p.rows).sum::<usize>()
            || choices.iter().any(|&f| f > 4)
        {
            return None;
        }
        let mut choices = choices.iter().copied();
        let mut pixels = self.bytes.as_slice();
        let mut output = Vec::with_capacity(self.bytes.len() + choices.len());
        for pass in &self.passes {
            let (data, rest) = pixels.split_at(pass.bytes * pass.rows);
            pixels = rest;
            let mut prev = &[][..];
            for (row, kind) in data.chunks_exact(pass.bytes).zip(choices.by_ref()) {
                output.push(kind);
                let start = output.len();
                output.resize(start + row.len(), 0);
                filter(kind, self.header.bpp(), row, prev, &mut output[start..]);
                prev = row;
            }
        }
        Some(output)
    }
    pub fn filtered(&self, strategy: Strategy) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            self.bytes.len() + self.passes.iter().map(|p| p.rows).sum::<usize>(),
        );
        let mut offset = 0;
        let mut pair_counts = if strategy == Strategy::PairEntropy {
            vec![0u32; 65536]
        } else {
            Vec::new()
        };
        let mut touched = Vec::<u16>::new();
        let mut probe = Vec::new();
        let mut history = [0u32; 256];
        let mut history_start = 0;
        for pass in &self.passes {
            let end = offset + pass.rows * pass.bytes;
            let mut prev = &[][..];
            let mut best = vec![0; pass.bytes];
            let mut trial = vec![0; pass.bytes];
            for row in self.bytes[offset..end].chunks_exact(pass.bytes) {
                let row_start = out.len();
                if let Strategy::Fixed(kind) = strategy {
                    out.push(kind);
                    let start = out.len();
                    out.resize(start + row.len(), 0);
                    filter(kind, self.header.bpp(), row, prev, &mut out[start..]);
                } else {
                    let mut best_score = u64::MAX;
                    let mut kind = 0;
                    for f in 0..=4 {
                        filter(f, self.header.bpp(), row, prev, &mut trial);
                        let score = match strategy {
                            Strategy::MinSum => trial
                                .iter()
                                .map(|&b| i8::unsigned_abs(b as i8) as u64)
                                .sum(),
                            Strategy::Runs => {
                                trial.windows(2).filter(|p| p[0] != p[1]).count() as u64
                            }
                            Strategy::Entropy => entropy(&trial),
                            Strategy::ContextEntropy => context_entropy(&history, f, &trial),
                            Strategy::PairEntropy => {
                                for pair in trial.windows(2) {
                                    let key = u16::from_be_bytes([pair[0], pair[1]]);
                                    let count = &mut pair_counts[usize::from(key)];
                                    if *count == 0 {
                                        touched.push(key);
                                    }
                                    *count += 1;
                                }
                                let mut gain = 0u64;
                                for key in touched.drain(..) {
                                    let n = std::mem::take(&mut pair_counts[usize::from(key)]);
                                    gain += info_gain(n);
                                }
                                u64::MAX - gain
                            }
                            Strategy::DeflateEstimate => {
                                // Probe a recent dictionary plus this row. The filter byte itself
                                // is part of the DEFLATE input and influences adjacent matches.
                                let context = (4 * (row.len() + 1)).min(16 * 1024);
                                probe.clear();
                                probe.extend_from_slice(&out[out.len().saturating_sub(context)..]);
                                probe.push(f);
                                probe.extend_from_slice(&trial);
                                miniz_oxide::deflate::compress_to_vec(&probe, 3).len() as u64
                            }
                            _ => unreachable!(),
                        };
                        if f == 0 || score < best_score {
                            best_score = score;
                            kind = f;
                            std::mem::swap(&mut best, &mut trial);
                        }
                    }
                    out.push(kind);
                    out.extend_from_slice(&best);
                }
                if strategy == Strategy::ContextEntropy {
                    // A DEFLATE block can reuse codes across rows. Model a recent
                    // window, allowing the model to adapt to regions in the image.
                    for &b in &out[row_start..] {
                        history[b as usize] += 1;
                    }
                    let keep_from = out.len().saturating_sub(16 * 1024);
                    for &b in &out[history_start..keep_from] {
                        history[b as usize] -= 1;
                    }
                    history_start = keep_from;
                }
                prev = row;
            }
            offset = end;
        }
        out
    }
}

pub(crate) fn sample(row: &[u8], x: usize, depth: u8) -> usize {
    let bit = x * depth as usize;
    usize::from((row[bit / 8] >> (8 - depth - (bit % 8) as u8)) & (u8::MAX >> (8 - depth)))
}

// Row entropy comparison. The common n*log(n) term cancels; a large sum of n_i*log(n_i) is better.
fn entropy(data: &[u8]) -> u64 {
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let gain: f64 = freq
        .into_iter()
        .filter(|&n| n > 1)
        .map(|n| f64::from(n) * f64::from(n).log2())
        .sum();
    u64::MAX - (gain * 256.) as u64
}

fn context_entropy(history: &[u32; 256], filter: u8, row: &[u8]) -> u64 {
    // Candidate lengths are equal. Minimizing the joint Shannon entropy of
    // the dictionary and current row amounts to maximizing sum(n * log2(n)).
    static TERMS: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    fn term(n: u32) -> u64 {
        if n < 2 {
            0
        } else {
            (f64::from(n) * f64::from(n).log2() * 256.0) as u64
        }
    }
    let table = TERMS.get_or_init(|| (0..=32768).map(|n| term(n) as u32).collect());
    let mut freq = *history;
    freq[filter as usize] += 1;
    for &b in row {
        freq[b as usize] += 1;
    }
    let sum: u64 = freq
        .into_iter()
        .map(|n| {
            table
                .get(n as usize)
                .map_or_else(|| term(n), |&v| u64::from(v))
        })
        .sum();
    u64::MAX - sum
}

fn info_gain(n: u32) -> u64 {
    if n <= 1 {
        return 0;
    }
    let exponent = n.ilog2();
    // Piecewise linear approximation of n*log2(n) on power-of-two intervals.
    u64::from(n) * u64::from(exponent) + 2 * u64::from(n - (1 << exponent))
}

fn filter(kind: u8, bpp: usize, row: &[u8], prev: &[u8], out: &mut [u8]) {
    if kind == 0 || (kind == 2 && prev.is_empty()) {
        out.copy_from_slice(row);
        return;
    }
    if kind == 1 || (kind == 4 && prev.is_empty()) {
        let n = bpp.min(row.len());
        out[..n].copy_from_slice(&row[..n]);
        for ((o, &v), &a) in out[n..].iter_mut().zip(&row[n..]).zip(row) {
            *o = v.wrapping_sub(a);
        }
        return;
    }
    if kind == 2 {
        for ((o, &v), &b) in out.iter_mut().zip(row).zip(prev) {
            *o = v.wrapping_sub(b);
        }
        return;
    }
    let start = bpp.min(row.len());
    if prev.is_empty() {
        // With zero left/above bytes Average predicts half of the left sample.
        out[..start].copy_from_slice(&row[..start]);
        for ((dst, &value), &left) in out[start..].iter_mut().zip(&row[start..]).zip(row) {
            *dst = value.wrapping_sub(left >> 1);
        }
        return;
    }
    if kind == 3 {
        for ((dst, &value), &up) in out[..start].iter_mut().zip(row).zip(prev) {
            *dst = value.wrapping_sub(up >> 1);
        }
        for (((dst, &value), &left), &up) in out[start..]
            .iter_mut()
            .zip(&row[start..])
            .zip(row)
            .zip(&prev[start..])
        {
            *dst = value.wrapping_sub(((u16::from(left) + u16::from(up)) >> 1) as u8);
        }
    } else {
        for ((dst, &value), &up) in out[..start].iter_mut().zip(row).zip(prev) {
            *dst = value.wrapping_sub(up);
        }
        for ((((dst, &value), &left), &up), &upper_left) in out[start..]
            .iter_mut()
            .zip(&row[start..])
            .zip(row)
            .zip(&prev[start..])
            .zip(prev)
        {
            *dst = value.wrapping_sub(paeth(left, up, upper_left));
        }
    }
}
