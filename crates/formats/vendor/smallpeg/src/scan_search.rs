//! Progressive scan search used by PixMinima's coefficient transcoder.

use super::ScanSpec;
use super::ScanSpec::{AcFirst, AcRefine, DcComponent, DcFirst, DcRefine};
use crate::component::Component;
use crate::huffman::build_huff_table;
use crate::symbols::{
    SparseAc, for_each_ac_band_symbol_sparse, for_each_ac_refine_symbol_sparse, for_each_dc_symbol,
};

// Each number is the beginning of a spectral band; 64 terminates the AC spectrum.
const CUTS: &[usize] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16, 18, 21, 25, 29, 34, 40, 47, 55, 64,
];
// Retain every fine low-frequency boundary and narrow the grid farther into the
// spectrum. This increases the number of candidate intervals quadratically.
const MAXIMUM_CUTS: &[usize] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27,
    29, 31, 34, 37, 40, 43, 47, 51, 55, 59, 64,
];
const LEVELS: usize = 6;

#[derive(Clone, Copy)]
struct Band {
    start: usize,
    end: usize,
    level: u8,
    refinement: bool,
}

fn statistics(ac: &SparseAc, band: Band) -> ([u32; 256], u64) {
    let mut freq = [0; 256];
    let mut payload = 0;
    let mut record = |symbol: Option<u8>, extra: u8, _: u16| {
        if let Some(symbol) = symbol {
            freq[usize::from(symbol)] += 1;
        }
        payload += u64::from(extra);
    };
    if band.refinement {
        for_each_ac_refine_symbol_sparse(ac, band.start, band.end, band.level, record);
    } else {
        for_each_ac_band_symbol_sparse(ac, band.start, band.end, band.level, |s, n, v| {
            record(Some(s), n, v)
        });
    }
    (freq, payload)
}

fn bits(freq: &[u32; 256], payload: u64, actual: u64, sampled: u64) -> u64 {
    let h = build_huff_table(freq);
    let coded: u64 = freq
        .iter()
        .zip(&h.codes)
        .map(|(&n, &(_, len))| u64::from(n) * u64::from(len))
        .sum();
    // DHT uses 4 framing bytes, selector, 16 counts and symbols. A
    // single-component SOS uses 10 bytes. Scale entropy, not those headers.
    (coded + payload) * actual / sampled + 8 * (31 + h.values.len() as u64)
}

struct Splits {
    cost: Vec<u64>,
    predecessor: Vec<usize>,
    n: usize,
}

impl Splits {
    fn new(n: usize, mut edge: impl FnMut(usize, usize) -> u64) -> Self {
        let mut cost = vec![0; n * n];
        let mut predecessor = vec![0; n * n];
        for end in 1..n {
            for start in (0..end).rev() {
                let at = start * n + end;
                cost[at] = edge(start, end);
                predecessor[at] = start;
                for middle in start + 1..end {
                    let score = cost[start * n + middle] + cost[middle * n + end];
                    if score < cost[at] {
                        cost[at] = score;
                        predecessor[at] = middle;
                    }
                }
            }
        }
        Self {
            cost,
            predecessor,
            n,
        }
    }
    fn ranges(&self, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
        let middle = self.predecessor[start * self.n + end];
        if middle == start {
            out.push((start, end));
        } else {
            self.ranges(start, middle, out);
            self.ranges(middle, end, out);
        }
    }
}

fn bands(ac: &SparseAc, maximum: bool) -> Vec<Band> {
    let (cuts, sample) = if maximum {
        (MAXIMUM_CUTS, ac.sample_runs(64, 64))
    } else {
        (CUTS, ac.sample())
    };
    let sample = sample.as_ref().unwrap_or(ac);
    let actual = ac.block_count() as u64;
    let sampled = sample.block_count() as u64;
    let n = cuts.len();
    let mut first = vec![[0; LEVELS]; n * n];
    let mut refine = vec![[0; LEVELS]; n * n];
    for (i, &start) in cuts.iter().enumerate().take(n - 1) {
        for (j, &stop) in cuts.iter().enumerate().skip(i + 1) {
            let idx = i * n + j;
            for level in 0..LEVELS {
                let band = Band {
                    start,
                    end: stop - 1,
                    level: level as u8,
                    refinement: false,
                };
                let (f, p) = statistics(sample, band);
                first[idx][level] = bits(&f, p, actual, sampled);
                if level + 1 < LEVELS {
                    let (f, p) = statistics(
                        sample,
                        Band {
                            refinement: true,
                            ..band
                        },
                    );
                    refine[idx][level] = bits(&f, p, actual, sampled);
                }
            }
        }
    }
    // A first scan and every successive refinement plane may each use their
    // own spectral subpartition. An outer shortest-path search can start a
    // different approximation chain in each adjacent range of coefficients.
    let first: Vec<_> = (0..LEVELS)
        .map(|l| Splits::new(n, |i, j| first[i * n + j][l]))
        .collect();
    let refine: Vec<_> = (0..LEVELS - 1)
        .map(|l| Splits::new(n, |i, j| refine[i * n + j][l]))
        .collect();
    let mut depths = vec![0; n * n];
    let chains = Splits::new(n, |i, j| {
        let at = i * n + j;
        let mut best = first[0].cost[at];
        let mut tail = 0;
        for l in 1..LEVELS {
            tail += refine[l - 1].cost[at];
            let price = first[l].cost[at] + tail;
            if price < best {
                best = price;
                depths[at] = l;
            }
        }
        best
    });
    let mut spectrum = Vec::new();
    chains.ranges(0, n - 1, &mut spectrum);
    let mut plan = Vec::new();
    for (start, end) in spectrum {
        let top = depths[start * n + end];
        let mut append = |paths: &Splits, level: usize, refinement: bool| {
            let mut ranges = Vec::new();
            paths.ranges(start, end, &mut ranges);
            plan.extend(ranges.into_iter().map(|(i, j)| Band {
                start: cuts[i],
                end: cuts[j] - 1,
                level: level as u8,
                refinement,
            }));
        };
        append(&first[top], top, false);
        for (level, paths) in refine.iter().enumerate().take(top) {
            append(paths, level, true);
        }
    }
    plan
}

pub(super) fn component_dc(
    c: &Component,
    level: u8,
    first: bool,
    mut visit: impl FnMut(u8, u8, u16),
) {
    let mut previous = 0;
    for y in 0..c.ah {
        for block in &c.blocks[y * c.bw..y * c.bw + c.aw] {
            let dc = i32::from(block[0]) >> level;
            if first {
                let delta = dc - previous;
                previous = dc;
                let n = crate::symbols::category(delta);
                let extra =
                    (if delta < 0 { delta - 1 } else { delta }) as u16 & ((1_u32 << n) - 1) as u16;
                visit(n, n, extra);
            } else {
                visit(0, 1, (dc & 1) as u16);
            }
        }
    }
}

fn separate_dc(comps: &[Component]) -> (u64, Vec<ScanSpec>) {
    let mut total = 0;
    let mut plan = Vec::new();
    for (ci, c) in comps.iter().enumerate() {
        let mut cheapest = (u64::MAX, 0, [0; 256]);
        for level in 0..=4 {
            let mut freq = [0; 256];
            let mut payload = 0;
            component_dc(c, level, true, |s, n, _| {
                freq[s as usize] += 1;
                payload += u64::from(n);
            });
            let cost = bits(&freq, payload, 1, 1) + u64::from(level) * ((c.aw * c.ah) as u64 + 80);
            if cost < cheapest.0 {
                cheapest = (cost, level, freq);
            }
        }
        let (cost, level, freq) = cheapest;
        total += cost;
        plan.push(DcComponent {
            ci,
            al: level,
            freq: Some(freq),
        });
        for al in (0..level).rev() {
            plan.push(DcComponent { ci, al, freq: None });
        }
    }
    (total, plan)
}

pub(super) fn plan(
    comps: &[Component],
    order: &[(usize, usize)],
    sparse: &[SparseAc],
    maximum: bool,
) -> Option<Vec<ScanSpec>> {
    if order.len() < 32 {
        return None;
    }
    let tables = comps.iter().map(|c| c.table_class).max()? + 1;
    let mut dc_best = (u64::MAX, 0, Vec::new());
    for level in 0..=4 {
        let mut f = vec![[0; 256]; tables];
        let mut payload = 0;
        for_each_dc_symbol(comps, order, level, |cls, s, n, _| {
            f[cls][usize::from(s)] += 1;
            payload += u64::from(n);
        });
        let cost = f.iter().map(|freq| bits(freq, 0, 1, 1)).sum::<u64>()
            + payload
            + u64::from(level) * (order.len() as u64 + 96);
        if cost < dc_best.0 {
            dc_best = (cost, level, f);
        }
    }
    let (dc_cost, dc_level, dc_freq) = dc_best;
    let (separate_cost, separate) = separate_dc(comps);
    let independent_dc = separate_cost < dc_cost && comps.len() > 1;
    let mut selected: Vec<_> = sparse
        .iter()
        .enumerate()
        .flat_map(|(ci, ac)| bands(ac, maximum).into_iter().map(move |s| (ci, s)))
        .collect();
    let dc_count = if independent_dc {
        separate.len()
    } else {
        1 + usize::from(dc_level)
    };
    if selected.len() + dc_count > 96 {
        return None;
    }
    // The decoder must receive the first approximation before refinements,
    // and successive refinement planes in descending order.
    selected.sort_by_key(|&(ci, b)| {
        (
            b.refinement,
            if b.refinement { u8::MAX - b.level } else { 0 },
            ci,
            b.start,
        )
    });
    let mut scans = if independent_dc {
        separate
    } else {
        vec![DcFirst {
            al: dc_level,
            freqs: dc_freq,
        }]
    };
    for (ci, b) in selected {
        let (freq, _) = statistics(&sparse[ci], b);
        let Band {
            start: ss,
            end: se,
            level: al,
            refinement,
        } = b;
        scans.push(if refinement {
            AcRefine {
                ci,
                ss,
                se,
                al,
                freq,
            }
        } else {
            AcFirst {
                ci,
                ss,
                se,
                al,
                freq,
            }
        });
    }
    if !independent_dc {
        for al in (0..dc_level).rev() {
            scans.push(DcRefine { al });
        }
    }
    Some(scans)
}
