//! Optimal JPEG Huffman tables, DHT segments and the bit writer.

#[derive(Clone)]
pub(crate) struct HuffTable {
    /// bits[i] = number of codes of length i (1-indexed, up to 16).
    bits: [u8; 17],
    pub(crate) values: Vec<u8>,
    /// codes[symbol] = (code, length)
    pub(crate) codes: [(u16, u8); 256],
}

/// Build a JPEG-legal optimal Huffman table from symbol frequencies
/// (the T.81 Annex K.2 procedure: reserve one dummy symbol so no real
/// symbol is assigned the all-ones code, limit lengths to 16).
pub(crate) fn build_huff_table(freq_in: &[u32; 256]) -> HuffTable {
    let mut weights = *freq_in;
    // Lengths are counted up to 32 below. Skewed counts (Fibonacci-like,
    // millions of symbols) can make the optimal tree deeper; halving the
    // counts, keeping every used symbol, flattens it until it fits. The
    // limit to 16 below then costs a little more, as it always does.
    let codesize = loop {
        let codesize = code_sizes(&weights);
        if codesize.iter().all(|&c| c <= 32) {
            break codesize;
        }
        for w in weights.iter_mut() {
            *w = w.div_ceil(2);
        }
    };
    finish_table(codesize)
}

/// Optimal code length of each symbol (the T.81 Annex K.2 tree), the
/// reserved symbol 256 included. Unlimited: may exceed 16.
fn code_sizes(freq_in: &[u32; 256]) -> [usize; 257] {
    let mut freq = [0i64; 257];
    for (i, &f) in freq_in.iter().enumerate() {
        freq[i] = f as i64;
    }
    freq[256] = 1; // reserved: guarantees no all-ones code

    let mut codesize = [0usize; 257];
    let mut others = [-1i32; 257];

    loop {
        // Find the two lowest-frequency nonzero entries (ties -> larger index).
        let (mut v1, mut v2) = (-1i32, -1i32);
        let mut c1 = i64::MAX;
        for (i, &fr) in freq.iter().enumerate() {
            if fr > 0 && fr <= c1 {
                c1 = fr;
                v1 = i as i32;
            }
        }
        let mut c2 = i64::MAX;
        for (i, &fr) in freq.iter().enumerate() {
            if fr > 0 && fr <= c2 && i as i32 != v1 {
                c2 = fr;
                v2 = i as i32;
            }
        }
        if v2 < 0 {
            break; // one tree left — done
        }
        freq[v1 as usize] += freq[v2 as usize];
        freq[v2 as usize] = 0;

        codesize[v1 as usize] += 1;
        let mut i = v1 as usize;
        while others[i] >= 0 {
            i = others[i] as usize;
            codesize[i] += 1;
        }
        others[i] = v2;
        codesize[v2 as usize] += 1;
        let mut i = v2 as usize;
        while others[i] >= 0 {
            i = others[i] as usize;
            codesize[i] += 1;
        }
    }
    codesize
}

fn finish_table(codesize: [usize; 257]) -> HuffTable {
    let mut bits_long = [0u8; 33];
    for &cs in codesize.iter() {
        if cs > 0 {
            bits_long[cs.min(32)] += 1;
        }
    }
    // Limit code lengths to 16 (Annex K.2, Adjust_BITS).
    for i in (17..=32).rev() {
        while bits_long[i] > 0 {
            let mut j = i - 2;
            while bits_long[j] == 0 {
                j -= 1;
            }
            bits_long[i] -= 2;
            bits_long[i - 1] += 1;
            bits_long[j + 1] += 2;
            bits_long[j] -= 1;
        }
    }
    // Remove the reserved dummy symbol from the longest used length.
    let mut i = 16;
    while bits_long[i] == 0 {
        i -= 1;
    }
    bits_long[i] -= 1;

    let mut bits = [0u8; 17];
    bits[1..=16].copy_from_slice(&bits_long[1..=16]);

    // Symbol values sorted by code length, then by symbol value.
    let mut values = Vec::new();
    for len in 1..=32 {
        for (sym, &cs) in codesize.iter().enumerate().take(256) {
            if cs == len {
                values.push(sym as u8);
            }
        }
    }

    // Canonical code assignment.
    let mut codes = [(0u16, 0u8); 256];
    let mut code = 0u16;
    let mut vi = 0usize;
    for len in 1..=16u8 {
        for _ in 0..bits[len as usize] {
            codes[values[vi] as usize] = (code, len);
            code += 1;
            vi += 1;
        }
        code <<= 1;
    }

    HuffTable {
        bits,
        values,
        codes,
    }
}

// ---------------------------------------------------------------- bit writer

pub(crate) struct BitWriter {
    pub(crate) out: Vec<u8>,
    /// Pending bits, right-aligned; fewer than 32 between calls.
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub(crate) fn new(out: Vec<u8>) -> Self {
        Self {
            out,
            acc: 0,
            nbits: 0,
        }
    }

    /// Append the low `n` (<= 16) bits of `bits`. Whole 32-bit words are
    /// written at once; only a word that contains 0xFF takes the per-byte
    /// stuffing path.
    #[inline]
    pub(crate) fn put(&mut self, bits: u16, n: u8) {
        self.acc = (self.acc << n) | bits as u64;
        self.nbits += n as u32;
        if self.nbits >= 32 {
            self.nbits -= 32;
            let word = (self.acc >> self.nbits) as u32;
            self.acc &= (1u64 << self.nbits) - 1;
            let bytes = word.to_be_bytes();
            // A byte is 0xFF iff it has no zero bit: test all four at once.
            if (!word).wrapping_sub(0x0101_0101) & word & 0x8080_8080 == 0 {
                self.out.extend_from_slice(&bytes);
            } else {
                for b in bytes {
                    self.out.push(b);
                    if b == 0xFF {
                        self.out.push(0x00); // byte stuffing
                    }
                }
            }
        }
    }

    pub(crate) fn flush(&mut self) {
        // Drain whole bytes first, then pad the last partial byte with ones.
        while self.nbits >= 8 {
            self.nbits -= 8;
            let b = (self.acc >> self.nbits) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0x00);
            }
        }
        self.acc &= (1u64 << self.nbits) - 1;
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            let b = ((self.acc << pad) | ((1 << pad) - 1)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0x00);
            }
            self.acc = 0;
            self.nbits = 0;
        }
    }
}

pub(crate) fn emit_dht(out: &mut Vec<u8>, class: u8, dest: u8, t: &HuffTable) {
    let len = 2 + 1 + 16 + t.values.len() as u16;
    out.extend_from_slice(&[0xFF, 0xC4]);
    out.extend_from_slice(&len.to_be_bytes());
    out.push((class << 4) | dest);
    out.extend_from_slice(&t.bits[1..=16]);
    out.extend_from_slice(&t.values);
}
