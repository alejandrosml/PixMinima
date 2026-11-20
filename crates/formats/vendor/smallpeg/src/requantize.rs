//! Requantization of an existing JPEG without a pixel round trip.
//! Each absolute floor is applied once. The DC coefficient and the first
//! five AC quantizers stay unchanged, along with sampling and color markers.

use super::{Limits, Placed, ScanSearch, encode_parsed, parse_and_decode};

// T.81 Annex K example tables in zigzag order, scaled to 16% for luminance
// and 12% for chrominance, with the six lowest frequencies protected.
const LUMA: [u8; 64] = [
    1, 1, 1, 1, 1, 1, 3, 2, 2, 2, 3, 3, 3, 3, 4, 6, 4, 4, 4, 4, 4, 8, 6, 6, 5, 6, 9, 8, 10, 10, 9,
    8, 9, 9, 10, 12, 15, 12, 10, 11, 14, 11, 9, 9, 13, 17, 13, 14, 15, 16, 16, 17, 16, 10, 12, 18,
    19, 18, 16, 19, 15, 16, 16, 16,
];
const CHROMA: [u8; 64] = [
    1, 1, 1, 1, 1, 1, 6, 3, 3, 6, 12, 8, 7, 8, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
];

/// Return a candidate only when one of the absolute target quantizers actually
/// changes. The caller must still check the resulting decoded pixel errors.
pub fn requantize_jpeg_with(data: &[u8], limits: &Limits, thorough: bool) -> Option<Vec<u8>> {
    let mut parsed = parse_and_decode(data, false, limits)?;
    if !matches!(parsed.comps.len(), 1 | 3) {
        return None;
    }
    let luma = parsed.comps[0].tq as usize;
    // The DC quantizer is immutable. It selects the same absolute AC profile
    // on every pass, including when the input already uses mixed/custom tables.
    let fine_source = parsed.qts[luma]?[0] <= 2;
    let mut changed = false;
    let mut seen = [false; 4];
    let mut proposed = parsed.qts;
    let mut headers = Vec::new();
    for range in &parsed.dqt {
        let segment = &data[range.clone()];
        if !segment.starts_with(&[0xff, 0xdb]) || segment.len() < 5 {
            return None;
        }
        let (tables, rest) = segment[4..].as_chunks::<65>();
        if !rest.is_empty() {
            return None;
        } // retain 16-bit and noncanonical DQTs
        headers.extend_from_slice(&segment[..4]);
        for table in tables {
            let id = usize::from(table[0]);
            if id >= seen.len() || seen[id] {
                return None;
            }
            seen[id] = true;
            let values = proposed[id].as_mut()?;
            let floor = if id == luma { &LUMA } else { &CHROMA };
            let used = parsed.comps.iter().any(|c| c.tq as usize == id);
            headers.push(table[0]);
            for (k, value) in values.iter_mut().enumerate() {
                let old = table[k + 1];
                if old == 0 {
                    return None;
                }
                let target = if fine_source { floor[k] } else { (3 * floor[k]).div_ceil(2) };
                let new = if used && k >= 6 { old.max(target) } else { old };
                changed |= old != new;
                *value = u16::from(new);
                headers.push(new);
            }
        }
    }
    if !changed {
        return None;
    }
    for component in &mut parsed.comps {
        let id = component.tq as usize;
        let before = parsed.qts[id]?;
        let after = proposed[id]?;
        for block in &mut component.blocks {
            for k in 6..64 {
                if before[k] == after[k] || block[k] == 0 {
                    continue;
                }
                let current = i32::from(block[k]) * i32::from(before[k]);
                let divisor = i32::from(after[k]);
                let nearest = (current.abs() + divisor / 2) / divisor;
                block[k] = (current.signum() * nearest) as i16;
            }
        }
    }
    let mut placed = Placed::new(data, &parsed, |_| {}, |_, _| {});
    placed.head.extend(headers);
    let search = if thorough {
        ScanSearch::Thorough
    } else {
        ScanSearch::Baseline
    };
    Some(encode_parsed(placed, parsed, search))
}
