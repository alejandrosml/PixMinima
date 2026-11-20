//! Minimum-size PackBits/Targa packetization of an individual scanline.
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Packetizer {
    choices: Vec<u8>,
    literals: VecDeque<(usize, isize)>,
}
#[derive(Clone, Copy)]
pub(super) enum Encoding {
    Photoshop,
    Targa,
}
impl Packetizer {
    pub fn append(&mut self, row: &[u8], unit: usize, kind: Encoding, out: &mut Vec<u8>) {
        let n = row.len() / unit;
        self.choices.clear();
        self.literals.clear();
        let mut costs = [0usize; 129];
        let mut identical_from = 0;
        // For a literal ending at i the cost is 1 + i*unit + min(cost[j]-j*unit).
        // A monotone queue finds that sliding minimum over the last 128 positions.
        for i in 1..=n {
            let j = i - 1;
            let delta = costs[j % 129] as isize - (j * unit) as isize;
            while self.literals.back().is_some_and(|&(_, v)| v >= delta) {
                self.literals.pop_back();
            }
            self.literals.push_back((j, delta));
            while self.literals[0].0 + 128 < i {
                self.literals.pop_front();
            }
            let (start, value) = self.literals[0];
            let mut cost = (1 + (i * unit) as isize + value) as usize;
            let mut choice = (i - start - 1) as u8;
            if j > 0 && row[j * unit..i * unit] != row[(j - 1) * unit..j * unit] {
                identical_from = j;
            }
            let run = identical_from.max(i.saturating_sub(128));
            // Truncating the last packet cannot increase cost, so prefix costs
            // never decrease. The earliest valid run start minimizes a repeat.
            if run + 1 < i {
                let repeated = costs[run % 129] + unit + 1;
                if repeated <= cost {
                    cost = repeated;
                    choice = 0x80 | ((i - run - 1) as u8);
                }
            }
            costs[i % 129] = cost;
            self.choices.push(choice);
        }
        let base = out.len();
        let mut end = base + costs[n % 129];
        out.resize(end, 0);
        let mut i = n;
        while i > 0 {
            let choice = self.choices[i - 1];
            let len = usize::from(choice & 127) + 1;
            let start = i - len;
            let repeat = choice & 0x80 != 0;
            let bytes = if repeat { unit } else { len * unit };
            end -= bytes + 1;
            out[end] = match (kind, repeat) {
                (Encoding::Photoshop, true) => (257 - len) as u8,
                _ => choice,
            };
            out[end + 1..end + 1 + bytes].copy_from_slice(&row[start * unit..][..bytes]);
            i = start;
        }
        debug_assert_eq!(end, base);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn brute(row: &[u8], unit: usize) -> usize {
        let n = row.len() / unit;
        let mut dp = vec![usize::MAX; n + 1];
        dp[0] = 0;
        for end in 1..=n {
            for from in end.saturating_sub(128)..end {
                let len = end - from;
                dp[end] = dp[end].min(dp[from] + 1 + unit * len);
                if len >= 2
                    && row[from * unit..end * unit]
                        .chunks_exact(unit)
                        .all(|p| p == &row[from * unit..(from + 1) * unit])
                {
                    dp[end] = dp[end].min(dp[from] + 1 + unit);
                }
            }
        }
        dp[n]
    }
    #[test]
    fn matches_exhaustive_shortest_path_for_all_short_binary_rows_and_packet_boundaries() {
        let mut packets = Packetizer::default();
        for unit in 1..=4 {
            for mask in 0u16..1024 {
                let row: Vec<_> = (0..10)
                    .flat_map(|i| vec![u8::from(mask & (1 << i) != 0); unit])
                    .collect();
                let mut buf = Vec::new();
                packets.append(&row, unit, Encoding::Targa, &mut buf);
                assert_eq!(buf.len(), brute(&row, unit));
            }
            for n in [127, 128, 129, 255, 256, 257] {
                let row: Vec<_> = (0..n)
                    .flat_map(|i| vec![((i / 137 + i % 7) % 3) as u8; unit])
                    .collect();
                let mut buf = Vec::new();
                packets.append(&row, unit, Encoding::Photoshop, &mut buf);
                assert_eq!(buf.len(), brute(&row, unit));
            }
        }
    }
}
