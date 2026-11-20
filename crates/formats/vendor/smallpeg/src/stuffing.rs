//! JPEG escapes every FF entropy byte with a zero byte. Swapping equal-length
//! Huffman symbols leaves all bit offsets and coded lengths unchanged, but can
//! remove escapes. Optimize the sampled bytes; the caller measures the complete
//! scan and accepts the alternate table only when its output is actually smaller.

use crate::huffman::HuffTable;

#[derive(Clone, Copy)]
struct Field {
    byte: u16,
    read_shift: u8,
    write_shift: u8,
    mask: u8,
}

impl Field {
    fn bits(self, code: u16) -> u8 {
        (((code >> self.read_shift) as u8) << self.write_shift) & self.mask
    }
}

pub(super) struct Sample {
    bytes: Vec<u8>,
    at: usize,
    fields: [Vec<Field>; 256],
}

impl Sample {
    pub(super) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            at: 0,
            fields: std::array::from_fn(|_| Vec::new()),
        }
    }

    pub(super) fn push(&mut self, symbol: Option<u8>, code: u16, mut length: u8) {
        const MAX: usize = 32_768;
        while length > 0 && self.at / 8 < MAX {
            let byte = self.at / 8;
            let available = 8 - (self.at % 8) as u8;
            let used = available.min(length);
            length -= used;
            let shift = available - used;
            let f = Field {
                byte: byte as u16,
                read_shift: length,
                write_shift: shift,
                mask: (((1_u16 << used) - 1) as u8) << shift,
            };
            if byte == self.bytes.len() {
                self.bytes.push(0);
            }
            self.bytes[byte] |= f.bits(code);
            if let Some(s) = symbol {
                self.fields[s as usize].push(f);
            }
            self.at += usize::from(used);
        }
    }

    // Only the bytes containing a field of this symbol can change. A byte
    // touched more than once contributes the net difference across its edits.
    fn set(&mut self, symbol: u8, code: u16) -> i32 {
        let mut difference = 0;
        for &f in &self.fields[symbol as usize] {
            let old = self.bytes[usize::from(f.byte)];
            let updated = (old & !f.mask) | f.bits(code);
            difference += i32::from(updated == 255) - i32::from(old == 255);
            self.bytes[usize::from(f.byte)] = updated;
        }
        difference
    }

    pub(super) fn improve(mut self, original: &HuffTable) -> Option<HuffTable> {
        if self.bytes.len() < 64 {
            return None;
        }
        let mut table = original.clone();
        let mut changed = false;
        // Successive strictly improving pair exchanges terminate; two sweeps
        // cap work even when many symbols have the same nominal length.
        for _ in 0..2 {
            let mut improved = false;
            for i in 0..table.values.len() {
                for j in i + 1..table.values.len() {
                    let (a, b) = (table.values[i], table.values[j]);
                    let (ca, la) = table.codes[a as usize];
                    let (cb, lb) = table.codes[b as usize];
                    if la != lb {
                        break;
                    }
                    if self.fields[a as usize].is_empty() && self.fields[b as usize].is_empty() {
                        continue;
                    }
                    let delta = self.set(a, cb) + self.set(b, ca);
                    if delta < 0 {
                        table.values.swap(i, j);
                        table.codes[a as usize] = (cb, la);
                        table.codes[b as usize] = (ca, lb);
                        improved = true;
                        changed = true;
                    } else {
                        self.set(a, ca);
                        self.set(b, cb);
                    }
                }
            }
            if !improved {
                break;
            }
        }
        changed.then_some(table)
    }
}
