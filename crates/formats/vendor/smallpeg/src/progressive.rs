//! Progressive scan script search and scan emission.

#[path = "scan_search.rs"]
mod scan_search;

use crate::component::Component;
use crate::huffman::{BitWriter, HuffTable, build_huff_table, emit_dht};
use crate::symbols::{
    SparseAc, ac_band_stats_multi, for_each_ac_band_symbol_sparse,
    for_each_ac_refine_symbol_sparse, for_each_dc_symbol, sparse_ac,
};

/// Each Huffman-coded scan carries the symbol statistics the scan search
/// already measured, so emission is a single pass.
enum ScanSpec {
    DcComponent {
        ci: usize,
        al: u8,
        freq: Option<[u32; 256]>,
    },
    /// Interleaved DC scan, Ah = 0, point transform `al`.
    DcFirst { al: u8, freqs: Vec<[u32; 256]> },
    /// Interleaved DC refinement: one raw bit per block, no Huffman data.
    DcRefine { al: u8 },
    /// Single-component AC band, Ah = 0, point transform `al`.
    AcFirst {
        ci: usize,
        ss: usize,
        se: usize,
        al: u8,
        freq: [u32; 256],
    },
    /// Single-component AC refinement, Ah = al + 1.
    AcRefine {
        ci: usize,
        ss: usize,
        se: usize,
        al: u8,
        freq: [u32; 256],
    },
}

/// Progressive scan sequence with spectral selection AND successive
/// approximation (the common simple progression): coefficients are
/// sent coarse-first, then refined one bit plane at a time. The wins over
/// baseline come from EOB runs pricing empty blocks across the image, the
/// coarse first scans concentrating symbol statistics, and every scan
/// getting its own freshly optimized Huffman table (DHT redefinition
/// between scans is legal and effectively free).
pub(crate) fn encode_progressive_scans(
    comps: &[Component],
    order: &[(usize, usize)],
    out: Vec<u8>,
    maximum: bool,
) -> Vec<u8> {
    use ScanSpec::*;

    // Sparse (CSR) views of the AC coefficients: the scan search and the
    // AC scan emission only ever touch nonzero coefficients, which is what
    // makes the progressive stage cheap (byte-identical to the dense path).
    let sparse: Vec<SparseAc> = comps.iter().map(sparse_ac).collect();

    // ---- Scan-script optimization: measure the exact coded size of every
    // candidate (statistics pass + optimal table), pick the cheapest. This
    // is what lets progressive win on smooth content too — no fixed script
    // fits every image.

    // Exact bit cost of one Huffman-coded scan given its symbol stats.
    let table_bits = |freq: &[u32; 256], payload: u64| -> u64 {
        let t = build_huff_table(freq);
        let sym_bits: u64 = freq
            .iter()
            .enumerate()
            .map(|(s, &f)| f as u64 * t.codes[s].1 as u64)
            .sum();
        // DHT (19 + values) and SOS (~12) bytes of header per scan.
        sym_bits + payload + (19 + t.values.len() as u64 + 12) * 8
    };

    // DC: choose the point transform by measured cost. A refinement level
    // costs exactly one raw bit per block plus a scan header.
    let n_dc = comps.iter().map(|c| c.table_class).max().unwrap_or(0) + 1;
    let dc_cost = |al: u8| -> (u64, Vec<[u32; 256]>) {
        let mut freqs = vec![[0u32; 256]; n_dc];
        let mut payload = 0u64;
        for_each_dc_symbol(comps, order, al, |tbl, sym, n, _| {
            freqs[tbl][sym as usize] += 1;
            payload += n as u64;
        });
        let mut bits: u64 = freqs.iter().map(|f| table_bits(f, 0)).sum();
        bits += payload;
        bits += al as u64 * (order.len() as u64 + 12 * 8); // refinement scans
        (bits, freqs)
    };
    let (dc0, dc1) = (dc_cost(0), dc_cost(1));
    let (dc_al, dc_freqs) = if dc1.0 < dc0.0 {
        (1u8, dc1.1)
    } else {
        (0, dc0.1)
    };

    // AC per component: band split {no, yes} x point transform {0, 1, 2}.
    // Refinement cost per level is independent of the chain's top, so it
    // is measured once per level.
    let ac_refine_cost = |ci: usize, al: u8| -> (u64, [u32; 256]) {
        let mut freq = [0u32; 256];
        let mut payload = 0u64;
        for_each_ac_refine_symbol_sparse(&sparse[ci], 1, 63, al, |sym, n, _| {
            if let Some(sym) = sym {
                freq[sym as usize] += 1;
            }
            payload += n as u64;
        });
        (table_bits(&freq, payload), freq)
    };

    let mut scans: Vec<ScanSpec> = vec![DcFirst {
        al: dc_al,
        freqs: dc_freqs,
    }];
    let mut refine_freqs: Vec<[[u32; 256]; 2]> = Vec::with_capacity(comps.len());
    let mut chain_al = vec![0u8; comps.len()];
    for ci in 0..comps.len() {
        let (r0, r1) = (ac_refine_cost(ci, 0), ac_refine_cost(ci, 1));
        let refine: [u64; 2] = [r0.0, r1.0];
        refine_freqs.push([r0.1, r1.1]);
        // Candidates: no split, or split the band at sp (low 1..=sp, high
        // sp+1..=63). Where the split lands matters a lot: smooth content
        // wants a tiny dense low band (1..=2) so the high band is almost
        // all EOB runs.
        let mut best = (u64::MAX, None::<usize>, 0u8);
        // One sparse pass per band covers all three point transforms.
        const SPLITS: [usize; 3] = [2, 5, 8];
        let mut bands = vec![(1usize, 63usize)];
        for sp in SPLITS {
            bands.extend([(1, sp), (sp + 1, 63)]);
        }
        let mut stats = ac_band_stats_multi(&sparse[ci], &bands).into_iter();
        let full_s = stats.next().unwrap();
        let split_s: Vec<_> = SPLITS
            .iter()
            .map(|&sp| (sp, stats.next().unwrap(), stats.next().unwrap()))
            .collect();
        for al in 0u8..=2 {
            let tail: u64 = (0..al).map(|l| refine[l as usize]).sum();
            let (f, pl) = &full_s[al as usize];
            let full = table_bits(f, *pl) + tail;
            if full < best.0 {
                best = (full, None, al);
            }
            for (sp, lo_s, hi_s) in &split_s {
                let (lf, lp) = &lo_s[al as usize];
                let (hf, hp) = &hi_s[al as usize];
                let split = table_bits(lf, *lp) + table_bits(hf, *hp) + tail;
                if split < best.0 {
                    best = (split, Some(*sp), al);
                }
            }
        }
        let (_, split, al) = best;
        chain_al[ci] = al;
        match split {
            Some(sp) => {
                let (_, lo_s, hi_s) = split_s.iter().find(|s| s.0 == sp).unwrap();
                scans.push(AcFirst {
                    ci,
                    ss: 1,
                    se: sp,
                    al,
                    freq: lo_s[al as usize].0,
                });
                scans.push(AcFirst {
                    ci,
                    ss: sp + 1,
                    se: 63,
                    al,
                    freq: hi_s[al as usize].0,
                });
            }
            None => scans.push(AcFirst {
                ci,
                ss: 1,
                se: 63,
                al,
                freq: full_s[al as usize].0,
            }),
        }
    }
    // Refinement rounds, coarse planes first; DC refinement rides along.
    let max_al = chain_al.iter().copied().max().unwrap_or(0).max(dc_al);
    for level in (0..max_al).rev() {
        if level < dc_al {
            scans.push(DcRefine { al: level });
        }
        for ci in 0..comps.len() {
            if level < chain_al[ci] {
                scans.push(AcRefine {
                    ci,
                    ss: 1,
                    se: 63,
                    al: level,
                    freq: refine_freqs[ci][level as usize],
                });
            }
        }
    }

    let Some(extended) = scan_search::plan(comps, order, &sparse, maximum) else {
        return emit_scans(comps, order, out, &sparse, scans, false, maximum);
    };
    let ordinary = emit_scans(comps, order, out.clone(), &sparse, scans, false, maximum);
    let extended = emit_scans(comps, order, out, &sparse, extended, true, maximum);
    if extended.len() < ordinary.len() {
        extended
    } else {
        ordinary
    }
}

fn emit_scans(
    comps: &[Component],
    order: &[(usize, usize)],
    mut out: Vec<u8>,
    sparse: &[SparseAc],
    scans: Vec<ScanSpec>,
    tune: bool,
    reuse: bool,
) -> Vec<u8> {
    use ScanSpec::*;
    let sos_interleaved = |out: &mut Vec<u8>, ah: u8, al: u8| {
        let sos_len = 6 + 2 * comps.len() as u16;
        out.extend_from_slice(&[0xFF, 0xDA]);
        out.extend_from_slice(&sos_len.to_be_bytes());
        out.push(comps.len() as u8);
        for (i, c) in comps.iter().enumerate() {
            out.push(i as u8 + 1);
            out.push((c.table_class as u8) << 4);
        }
        out.extend_from_slice(&[0x00, 0x00, (ah << 4) | al]);
    };

    // Progressive JPEG has four persistent AC Huffman slots. A later scan
    // may reuse any active table covering its symbols, skipping the DHT.
    let mut ac_slots: Vec<HuffTable> = Vec::new();
    let mut last_use = [0usize; 4];
    for (index, spec) in scans.into_iter().enumerate() {
        match spec {
            DcComponent { ci, al, freq } => {
                let first = freq.is_some();
                let h = freq.as_ref().map(build_huff_table);
                if let Some(h) = &h {
                    emit_dht(&mut out, 0, 0, h);
                }
                out.extend_from_slice(&[
                    0xff,
                    0xda,
                    0,
                    8,
                    1,
                    ci as u8 + 1,
                    0,
                    0,
                    0,
                    if first { al } else { ((al + 1) << 4) | al },
                ]);
                let mut writer = BitWriter::new(out);
                scan_search::component_dc(&comps[ci], al, first, |sym, n, v| {
                    if let Some(h) = &h {
                        let (c, l) = h.codes[sym as usize];
                        writer.put(c, l);
                    }
                    writer.put(v, n);
                });
                writer.flush();
                out = writer.out;
            }
            DcFirst { al, freqs } => {
                let tables: Vec<HuffTable> = freqs.iter().map(build_huff_table).collect();
                for (dest, t) in tables.iter().enumerate() {
                    emit_dht(&mut out, 0, dest as u8, t);
                }
                sos_interleaved(&mut out, 0, al);
                let mut writer = BitWriter::new(out);
                for_each_dc_symbol(comps, order, al, |tbl, sym, n, bits| {
                    let (code, len) = tables[tbl].codes[sym as usize];
                    writer.put(code, len);
                    writer.put(bits, n);
                });
                writer.flush();
                out = writer.out;
            }
            DcRefine { al } => {
                sos_interleaved(&mut out, al + 1, al);
                let mut writer = BitWriter::new(out);
                for &(ci, bi) in order {
                    let bit = ((comps[ci].blocks[bi][0] as i32) >> al) & 1;
                    writer.put(bit as u16, 1);
                }
                writer.flush();
                out = writer.out;
            }
            AcFirst {
                ci,
                ss,
                se,
                al,
                freq,
            }
            | AcRefine {
                ci,
                ss,
                se,
                al,
                freq,
            } => {
                let refinement = matches!(spec, AcRefine { .. });
                let walk = |ac: &SparseAc, cb: &mut dyn FnMut(Option<u8>, u8, u16)| {
                    if refinement {
                        for_each_ac_refine_symbol_sparse(ac, ss, se, al, cb);
                    } else {
                        for_each_ac_band_symbol_sparse(ac, ss, se, al, |s, n, b| cb(Some(s), n, b));
                    }
                };
                let table = build_huff_table(&freq);
                let mut slot = if !reuse {
                    0
                } else if ac_slots.len() < 4 {
                    ac_slots.len()
                } else {
                    (0..4).min_by_key(|&s| last_use[s]).unwrap()
                };
                let prefix = [
                    0xff,
                    0xda,
                    0,
                    8,
                    1,
                    ci as u8 + 1,
                    0,
                    ss as u8,
                    se as u8,
                    if refinement { ((al + 1) << 4) | al } else { al },
                ];
                let emit = |t: &HuffTable, slot: usize, define: bool| {
                    let mut segment = Vec::new();
                    if define {
                        emit_dht(&mut segment, 1, slot as u8, t);
                    }
                    let mut header = prefix;
                    header[6] = slot as u8;
                    segment.extend_from_slice(&header);
                    let mut writer = BitWriter::new(segment);
                    walk(&sparse[ci], &mut |sym, n, b| {
                        if let Some(s) = sym {
                            let (code, len) = t.codes[s as usize];
                            writer.put(code, len);
                        }
                        writer.put(b, n);
                    });
                    writer.flush();
                    writer.out
                };
                let mut best = emit(&table, slot, true);
                let mut new_table = table;
                if tune && best.len() > 96 {
                    let mut sample = crate::stuffing::Sample::new();
                    let selected = sparse[ci].sample();
                    walk(
                        selected.as_ref().unwrap_or(&sparse[ci]),
                        &mut |sym, n, b| {
                            if let Some(s) = sym {
                                let (code, len) = new_table.codes[s as usize];
                                sample.push(Some(s), code, len);
                            }
                            sample.push(None, b, n);
                        },
                    );
                    if let Some(t) = sample.improve(&new_table) {
                        let alternative = emit(&t, slot, true);
                        if alternative.len() < best.len() {
                            best = alternative;
                            new_table = t;
                        }
                    }
                }
                if reuse {
                    let mut define = true;
                    for (id, current) in ac_slots.iter().enumerate() {
                        let covers = freq
                            .iter()
                            .zip(&current.codes)
                            .all(|(&count, &(_, width))| count == 0 || width != 0);
                        if covers {
                            let alternative = emit(current, id, false);
                            if alternative.len() < best.len() {
                                slot = id;
                                best = alternative;
                                define = false;
                            }
                        }
                    }
                    if define {
                        if slot == ac_slots.len() {
                            ac_slots.push(new_table);
                        } else {
                            ac_slots[slot] = new_table;
                        }
                    }
                    last_use[slot] = index + 1;
                }
                out.extend_from_slice(&best);
            }
        }
    }

    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}
