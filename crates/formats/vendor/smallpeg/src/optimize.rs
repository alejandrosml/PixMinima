//! Lossless re-optimization of existing JPEG files (clean-room from ITU
//! T.81): the entropy layer of a baseline or progressive JPEG is decoded
//! back to its quantized DCT
//! coefficients, and those exact coefficients are re-encoded through this
//! crate's optimized-Huffman baseline and scan-optimized progressive
//! emitters, keeping whichever is smaller. Decoded pixels are bit-identical
//! because everything the decoder derives pixels from is preserved:
//! coefficients, sampling factors and component ids exactly, and the
//! APPn/COM/DQT segments byte-for-byte. DQT and the APPn/COM segments
//! that came before the frame header are re-emitted after SOI; later
//! APPn/COM segments keep their side of the frame header and of the first
//! scan (see [`Placed`]), since readers treat those differently.
//!
//! This is an entropy decoder only (Huffman coding and the scan structure);
//! there is no IDCT, upsampling or color conversion here. The pixel
//! decoders (`decode_jpeg`, `decode_cmyk`, in `decode` and `cmyk`) build on
//! this parser. It parses hostile input, so every read is bounds-checked
//! and every failure returns `None`.

#[path = "requantize.rs"]
mod requantize;
pub use requantize::requantize_jpeg_with;

use crate::ceil_div;
use crate::component::{Component, interleave_order};
use crate::huffman::{BitWriter, HuffTable, build_huff_table, emit_dht};
use crate::progressive::encode_progressive_scans;
use crate::symbols::for_each_symbol;

/// Cap on total allocated coefficient blocks (~1 GiB of coefficients),
/// so a forged SOF header cannot demand an absurd allocation.
const MAX_TOTAL_BLOCKS: usize = 1 << 23;

/// Losslessly re-optimize a JPEG: same pixels, usually fewer bytes.
///
/// Returns the re-encoded file, or `None` for anything that cannot be
/// safely round-tripped: arithmetic-coded or hierarchical files, 12-bit
/// precision, component ids other than 1..=N / 0..N-1 (some encoders use
/// ids to signal the color space, so they must not be rewritten), files
/// whose restart cadence contradicts their DRI header (some encoders
/// emit these in progressive mode; standard decoders flag them as
/// corrupt data and resynchronize), or malformed input. The result is not
/// guaranteed to be smaller than the input — callers should keep
/// whichever is smaller.
pub fn optimize_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    optimize_jpeg_with(data, &Limits::default())
}

/// [`optimize_jpeg`] with caller-chosen [`Limits`]; `None` also when the
/// input exceeds them.
pub fn optimize_jpeg_with(data: &[u8], limits: &Limits) -> Option<Vec<u8>> {
    let parsed = parse_and_decode(data, false, limits)?;
    Some(reencode(data, parsed, ScanSearch::Thorough))
}

/// Try a denser progressive scan plan. Callers can retain the ordinary plan if
/// its emitted byte stream is smaller.
pub fn optimize_jpeg_maximum_with(data: &[u8], limits: &Limits) -> Option<Vec<u8>> {
    let parsed = parse_and_decode(data, false, limits)?;
    Some(reencode(data, parsed, ScanSearch::Maximum))
}

/// Like [`optimize_jpeg`], but the output is always a baseline sequential
/// JPEG (single interleaved scan, optimized Huffman tables) — for targets
/// that cannot take progressive files, such as PDF embedding. A
/// progressive input is converted to baseline; that too is lossless, since
/// the quantized coefficients are preserved exactly.
///
/// Stable API, like `Encoder::progressive`.
pub fn optimize_jpeg_baseline(data: &[u8]) -> Option<Vec<u8>> {
    optimize_jpeg_baseline_with(data, &Limits::default())
}

/// [`optimize_jpeg_baseline`] with caller-chosen [`Limits`]; `None` also
/// when the input exceeds them.
pub fn optimize_jpeg_baseline_with(data: &[u8], limits: &Limits) -> Option<Vec<u8>> {
    let parsed = parse_and_decode(data, false, limits)?;
    Some(reencode(data, parsed, ScanSearch::Baseline))
}

/// Compare all quantized coefficients representing image samples. Dummy MCU
/// edge blocks lie outside the component and are not exposed by JPEG decoders.
pub fn same_coefficients(a: &[u8], b: &[u8], limits: &Limits) -> bool {
    let (Some(left), Some(right)) = (
        parse_and_decode(a, false, limits),
        parse_and_decode(b, false, limits),
    ) else {
        return false;
    };
    if (
        left.width,
        left.height,
        left.adobe,
        left.jfif,
        &left.ids,
        &left.qts,
    ) != (
        right.width,
        right.height,
        right.adobe,
        right.jfif,
        &right.ids,
        &right.qts,
    ) || left.comps.len() != right.comps.len()
    {
        return false;
    }
    left.comps.iter().zip(&right.comps).all(|(l, r)| {
        (l.aw, l.ah, l.h, l.v, l.tq) == (r.aw, r.ah, r.h, r.v, r.tq)
            && (0..l.ah)
                .all(|y| l.blocks[y * l.bw..y * l.bw + l.aw] == r.blocks[y * r.bw..y * r.bw + r.aw])
    })
}

/// Resource limits for the functions that take untrusted JPEG bytes
/// ([`decode_jpeg_with`](crate::decode_jpeg_with),
/// [`decode_cmyk_with`](crate::decode_cmyk_with), [`optimize_jpeg_with`],
/// [`optimize_jpeg_baseline_with`],
/// [`upright_jpeg_with`](crate::upright_jpeg_with)). Input beyond a limit is refused with
/// `None` before anything is allocated for it.
///
/// Memory is decided by the frame header alone: a file of a few hundred
/// bytes can declare a frame of hundreds of millions of pixels, and
/// decoding it costs about 3 bytes of coefficients and samples per pixel
/// and component plus the output, whatever the file size — up to about
/// 1.2 GB for a 100-megapixel color frame, 1.6 GB for CMYK. The defaults
/// bound that; callers that know their budget should set `max_pixels` to
/// it (and can check [`jpeg_info`] first).
///
/// ```
/// let limits = smallpeg::Limits::default();
/// assert_eq!((limits.max_pixels, limits.max_scans), (100_000_000, 128));
/// // Opt out of the pixel limit (the built-in block cap still applies):
/// let big = smallpeg::Limits::default().max_pixels(u64::MAX);
/// # assert_eq!(big.max_pixels, u64::MAX);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum `width * height` of the frame. Default 100,000,000. Above
    /// any setting, a built-in cap of 2^23 coefficient blocks (1 GiB of
    /// coefficients, about 537 million pixels of one component) applies.
    pub max_pixels: u64,
    /// Maximum number of scans. Each scan visits every block of its
    /// components however few bytes it has, so this bounds the work a
    /// small file can ask for. Progressive scans must also follow the
    /// successive-approximation order, which codes each coefficient in at
    /// most 14 scans, so decoding a frame costs at most a few times what
    /// its pixel count suggests (as memory does, time follows the frame
    /// size; `max_pixels` bounds both). Default 128; real files use about
    /// 10 at most.
    pub max_scans: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_pixels: 100_000_000,
            max_scans: 128,
        }
    }
}

impl Limits {
    /// Set [`max_pixels`](Limits::max_pixels).
    pub fn max_pixels(mut self, n: u64) -> Self {
        self.max_pixels = n;
        self
    }

    /// Set [`max_scans`](Limits::max_scans).
    pub fn max_scans(mut self, n: u32) -> Self {
        self.max_scans = n;
        self
    }
}

/// Frame-header facts of a JPEG, read without decoding; see [`jpeg_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct JpegInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Number of components: 1 (gray), 3 (YCbCr or RGB) or 4 (CMYK/YCCK).
    pub components: u8,
    /// Progressive (SOF2) rather than baseline/extended sequential.
    pub progressive: bool,
    /// EXIF orientation, 1..=8 (1 when there is no valid EXIF tag): how
    /// the stored pixels must be turned for display, e.g. 6 for a phone
    /// photo taken upright. The decoders return pixels as stored; see
    /// [`apply_orientation`](crate::apply_orientation).
    pub orientation: u8,
}

/// Read a JPEG's frame header without decoding anything: walks the
/// marker segments up to the SOF (picking up the EXIF orientation on the
/// way) and allocates nothing.
///
/// Meant as a cheap probe before [`decode_jpeg`](crate::decode_jpeg),
/// [`decode_cmyk`](crate::decode_cmyk) or [`optimize_jpeg`] on untrusted
/// input, e.g. to check the dimensions against a size budget. Returns
/// `None` when no frame header this crate can decode (8-bit baseline,
/// extended sequential or progressive Huffman) comes before the first
/// scan, or when that header is malformed. `Some` does not promise that
/// the rest of the file decodes.
pub fn jpeg_info(data: &[u8]) -> Option<JpegInfo> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    let mut orientation = None;
    loop {
        let (marker, after) = next_marker(data, pos)?;
        match marker {
            0x01 | 0xD0..=0xD7 => {
                pos = after;
                continue;
            }
            // EOI, SOI, a scan before any frame, or another frame type.
            0xD8 | 0xD9 | 0xDA | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return None;
            }
            _ => {}
        }
        if after + 2 > data.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([data[after], data[after + 1]]) as usize;
        if seg_len < 2 || after + seg_len > data.len() {
            return None;
        }
        if (0xC0..=0xC2).contains(&marker) {
            let f = parse_sof(&data[after + 2..after + seg_len])?;
            return Some(JpegInfo {
                width: f.width as u32,
                height: f.height as u32,
                components: f.comps.len() as u8,
                progressive: marker == 0xC2,
                orientation: orientation.unwrap_or(1),
            });
        }
        if marker == 0xE1 && orientation.is_none() {
            orientation = crate::meta::exif_orientation(&data[after + 2..after + seg_len]);
        }
        pos = after + seg_len;
    }
}

/// A validated SOF payload.
struct Sof {
    width: u16,
    height: u16,
    /// (id, h, v, tq) per component, in frame order.
    comps: Vec<(u8, usize, usize, u8)>,
}

fn parse_sof(payload: &[u8]) -> Option<Sof> {
    if payload.len() < 6 {
        return None;
    }
    let precision = payload[0];
    let height = u16::from_be_bytes([payload[1], payload[2]]);
    let width = u16::from_be_bytes([payload[3], payload[4]]);
    let n = payload[5] as usize;
    if precision != 8 || width == 0 || height == 0 || n == 0 || n > 4 {
        return None;
    }
    if payload.len() != 6 + 3 * n {
        return None;
    }
    let mut comps = Vec::with_capacity(n);
    for i in 0..n {
        let id = payload[6 + 3 * i];
        let hv = payload[7 + 3 * i];
        let (h, v) = ((hv >> 4) as usize, (hv & 0x0F) as usize);
        let tq = payload[8 + 3 * i];
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || tq > 3 {
            return None;
        }
        comps.push((id, h, v, tq));
    }
    Some(Sof {
        width,
        height,
        comps,
    })
}

// ---------------------------------------------------------------- parsing

pub(crate) struct FComp {
    pub(crate) h: usize,
    pub(crate) v: usize,
    pub(crate) tq: u8,
    /// Padded block grid (interleaved-MCU coverage), row stride `bw`.
    pub(crate) bw: usize,
    /// Actual data-unit grid (non-interleaved coverage).
    pub(crate) aw: usize,
    pub(crate) ah: usize,
    pub(crate) blocks: Vec<[i16; 64]>,
}

pub(crate) struct Parsed {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) comps: Vec<FComp>,
    /// Byte ranges of APPn/COM segments, in original order (marker included).
    pub(crate) meta: Vec<core::ops::Range<usize>>,
    /// Byte ranges of DQT segments (marker included).
    pub(crate) dqt: Vec<core::ops::Range<usize>>,
    /// Offset of the SOF marker: APPn segments before it are the ones
    /// `jpeg_info` and viewers read.
    pub(crate) frame_at: usize,
    /// Offset of the first SOS marker.
    pub(crate) scan_at: usize,
    /// MCU columns and rows.
    pub(crate) mcu_w: usize,
    pub(crate) mcu_h: usize,
    /// Quantization tables by destination, in zigzag order as stored
    /// (`None` where no DQT defined one). Only the pixel decoder reads
    /// these; re-encoding copies the DQT bytes verbatim.
    pub(crate) qts: [Option<[u16; 64]>; 4],
    /// Transform byte of an Adobe APP14 segment, if one is present.
    pub(crate) adobe: Option<u8>,
    /// A JFIF APP0 segment is present.
    pub(crate) jfif: bool,
    /// Frame component ids, in frame order.
    pub(crate) ids: Vec<u8>,
}

/// Bits resolved by a single table lookup in `BitReader::huff`; longer
/// codes fall back to the canonical-code walk.
const LOOKAHEAD: u32 = 9;

#[derive(Clone)]
struct DecHuff {
    values: Vec<u8>,
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [i32; 17],
    /// Indexed by the next LOOKAHEAD bits: `(length << 8) | value` for
    /// codes up to LOOKAHEAD bits long, 0 when the code is longer.
    lut: Box<[u16; 1 << LOOKAHEAD]>,
}

impl DecHuff {
    fn build(counts: &[u8], values: &[u8]) -> Option<DecHuff> {
        let mut mincode = [0i32; 17];
        let mut maxcode = [-1i32; 17];
        let mut valptr = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0i32;
        for l in 1..=16 {
            let n = counts[l - 1] as i32;
            if n > 0 {
                valptr[l] = k;
                mincode[l] = code;
                code += n;
                maxcode[l] = code - 1;
                k += n;
                // Canonical codes of length l must fit in l bits.
                if code > (1 << l) {
                    return None;
                }
            }
            code <<= 1;
        }
        if k as usize != values.len() {
            return None;
        }
        let mut lut = Box::new([0u16; 1 << LOOKAHEAD]);
        for l in 1..=LOOKAHEAD as usize {
            for code in mincode[l]..=maxcode[l] {
                let v = values[(valptr[l] + code - mincode[l]) as usize];
                let shift = LOOKAHEAD as usize - l;
                let first = (code as usize) << shift;
                lut[first..first + (1 << shift)].fill(((l as u16) << 8) | v as u16);
            }
        }
        Some(DecHuff {
            values: values.to_vec(),
            mincode,
            maxcode,
            valptr,
            lut,
        })
    }
}

struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(d: &'a [u8], pos: usize) -> BitReader<'a> {
        BitReader {
            d,
            pos,
            buf: 0,
            cnt: 0,
        }
    }

    fn fill(&mut self) {
        while self.cnt <= 56 && self.pos < self.d.len() {
            let b = self.d[self.pos];
            if b == 0xFF {
                if self.pos + 1 < self.d.len() && self.d[self.pos + 1] == 0x00 {
                    self.pos += 2; // stuffed 0xFF data byte
                } else {
                    return; // a real marker: entropy data ends here
                }
            } else {
                self.pos += 1;
            }
            self.buf = (self.buf << 8) | b as u64;
            self.cnt += 8;
        }
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.cnt < n {
            self.fill();
            if self.cnt < n {
                return None;
            }
        }
        self.cnt -= n;
        Some(((self.buf >> self.cnt) & ((1u64 << n) - 1)) as u32)
    }

    fn bit(&mut self) -> Option<u32> {
        self.bits(1)
    }

    fn huff(&mut self, t: &DecHuff) -> Option<u8> {
        if self.cnt < 16 {
            self.fill();
        }
        if self.cnt >= LOOKAHEAD {
            let e = t.lut[((self.buf >> (self.cnt - LOOKAHEAD)) as usize) & ((1 << LOOKAHEAD) - 1)];
            if e != 0 {
                self.cnt -= (e >> 8) as u32;
                return Some(e as u8);
            }
        }
        let avail = self.cnt.min(16);
        if avail == 0 {
            return None;
        }
        // Left-justified peek of the next `avail` bits.
        let peek = (((self.buf >> (self.cnt - avail)) as u32) << (16 - avail)) & 0xFFFF;
        for l in 1..=avail {
            let code = (peek >> (16 - l)) as i32;
            if code <= t.maxcode[l as usize] {
                let idx = (t.valptr[l as usize] + code - t.mincode[l as usize]) as usize;
                let v = *t.values.get(idx)?;
                self.cnt -= l;
                return Some(v);
            }
        }
        None
    }

    /// Byte-align (discarding pad bits) and consume the expected RSTn.
    fn restart(&mut self, n: u8) -> Option<()> {
        self.buf = 0;
        self.cnt = 0;
        // Optional 0xFF fill bytes are legal before any marker.
        while self.pos + 1 < self.d.len()
            && self.d[self.pos] == 0xFF
            && self.d[self.pos + 1] == 0xFF
        {
            self.pos += 1;
        }
        if self.pos + 1 < self.d.len()
            && self.d[self.pos] == 0xFF
            && self.d[self.pos + 1] == 0xD0 + n
        {
            self.pos += 2;
            Some(())
        } else {
            None
        }
    }
}

/// `extend` from T.81 F.2.2.1: map `t` received magnitude bits to a value.
fn extend(v: u32, t: u32) -> i32 {
    if (v as i32) < (1 << (t - 1)) {
        v as i32 - (1 << t) + 1
    } else {
        v as i32
    }
}

/// Find the next marker at or after `pos`, skipping fill bytes and any
/// stray non-marker bytes (T.81 allows fill; decoders tolerate junk).
pub(crate) fn next_marker(d: &[u8], mut pos: usize) -> Option<(u8, usize)> {
    while pos + 1 < d.len() {
        if d[pos] == 0xFF {
            let b = d[pos + 1];
            if b == 0x00 || b == 0xFF {
                pos += if b == 0x00 { 2 } else { 1 };
                continue;
            }
            return Some((b, pos + 2));
        }
        pos += 1;
    }
    None
}

struct ScanComp {
    ci: usize,
    dc: usize, // DC table dest
    ac: usize, // AC table dest
}

/// Parse a JPEG and entropy-decode every scan into quantized coefficients.
///
/// `any_ids`: accept any set of distinct component ids. Re-encoding writes
/// ids 1..=N, so the lossless path only takes id sets that are safe to
/// renumber; the pixel decoder never re-emits them and can take any.
pub(crate) fn parse_and_decode(data: &[u8], any_ids: bool, limits: &Limits) -> Option<Parsed> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    let mut meta: Vec<core::ops::Range<usize>> = Vec::new();
    let mut dqt: Vec<core::ops::Range<usize>> = Vec::new();
    let mut dc_tables: [Option<DecHuff>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<DecHuff>; 4] = [None, None, None, None];
    let mut frame: Option<(bool, u16, u16)> = None; // (progressive, w, h)
    let mut frame_at = 0usize;
    let mut scan_at = 0usize;
    let mut comps: Vec<FComp> = Vec::new();
    let mut comp_ids: Vec<u8> = Vec::new();
    let (mut mcu_w, mut mcu_h) = (0usize, 0usize);
    let mut restart_interval = 0usize;
    let mut seen_scan = false;
    let mut scans = 0u32;
    // Progressive state per component and coefficient: the Al of the last
    // scan that coded it, or -1 before its first scan.
    let mut approx: Vec<[i8; 64]> = Vec::new();
    let mut qts: [Option<[u16; 64]>; 4] = [None; 4];
    let mut adobe: Option<u8> = None;
    let mut jfif = false;

    loop {
        let (marker, after) = next_marker(data, pos)?;
        // Markers without a length field.
        match marker {
            0xD9 => break, // EOI
            0xD8 => return None,
            0x01 | 0xD0..=0xD7 => {
                pos = after;
                continue;
            }
            _ => {}
        }
        // Everything else carries a big-endian length that includes itself.
        if after + 2 > data.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([data[after], data[after + 1]]) as usize;
        if seg_len < 2 || after + seg_len > data.len() {
            return None;
        }
        let payload = &data[after + 2..after + seg_len];
        let seg_range = after - 2..after + seg_len;
        pos = after + seg_len;

        match marker {
            0xE0..=0xEF | 0xFE => {
                // APPn / COM. Adobe APP14: "Adobe", version (2), flags0
                // (2), flags1 (2), transform (1).
                if marker == 0xEE && payload.len() >= 12 && payload.starts_with(b"Adobe") {
                    adobe = Some(payload[11]);
                }
                if marker == 0xE0 && payload.starts_with(b"JFIF\0") {
                    jfif = true;
                }
                meta.push(seg_range);
            }
            0xDB => {
                // DQT: validate structure, then pass the bytes through.
                if seen_scan {
                    return None; // table change after a scan: not supported
                }
                let mut p = 0usize;
                while p < payload.len() {
                    let pq = payload[p] >> 4;
                    let n = match pq {
                        0 => 65,
                        1 => 129,
                        _ => return None,
                    };
                    if p + n > payload.len() {
                        return None;
                    }
                    let tq = (payload[p] & 0x0F) as usize;
                    if tq < 4 {
                        let mut t = [0u16; 64];
                        for (k, v) in t.iter_mut().enumerate() {
                            *v = if pq == 0 {
                                payload[p + 1 + k] as u16
                            } else {
                                u16::from_be_bytes([payload[p + 1 + 2 * k], payload[p + 2 + 2 * k]])
                            };
                        }
                        qts[tq] = Some(t);
                    }
                    p += n;
                }
                dqt.push(seg_range);
            }
            0xC4 => {
                // DHT: one or more tables.
                let mut p = 0usize;
                while p < payload.len() {
                    if p + 17 > payload.len() {
                        return None;
                    }
                    let tc = payload[p] >> 4;
                    let th = (payload[p] & 0x0F) as usize;
                    if tc > 1 || th > 3 {
                        return None;
                    }
                    let counts = &payload[p + 1..p + 17];
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    if total > 256 || p + 17 + total > payload.len() {
                        return None;
                    }
                    let t = DecHuff::build(counts, &payload[p + 17..p + 17 + total])?;
                    if tc == 0 {
                        dc_tables[th] = Some(t);
                    } else {
                        ac_tables[th] = Some(t);
                    }
                    p += 17 + total;
                }
            }
            0xDD => {
                if payload.len() != 2 {
                    return None;
                }
                restart_interval = u16::from_be_bytes([payload[0], payload[1]]) as usize;
            }
            0xC0..=0xC2 => {
                if frame.is_some() {
                    return None;
                }
                let sof = parse_sof(payload)?;
                let (width, height, n) = (sof.width, sof.height, sof.comps.len());
                if width as u64 * height as u64 > limits.max_pixels {
                    return None;
                }
                let mut hmax = 1usize;
                let mut vmax = 1usize;
                let mut raw: Vec<(usize, usize, u8)> = Vec::new();
                let mut ids: Vec<u8> = Vec::new();
                for &(id, mut h, mut v, tq) in &sof.comps {
                    // Component ids sometimes encode the color space (e.g.
                    // 'R','G','B' signals RGB to mainstream decoders),
                    // and our emitters write ids 1..=N. Only accept id sets
                    // that are safe to normalize: exactly 1..=N, the 0-based
                    // 0..N-1 convention (some encoders write this; decoders
                    // interpret it identically to 1..=N), or any single id
                    // for one-component frames (always grayscale).
                    ids.push(id);
                    // A single-component frame is always coded
                    // non-interleaved, where sampling factors carry no
                    // information — normalize them so the grid is unpadded.
                    if n == 1 {
                        h = 1;
                        v = 1;
                    }
                    hmax = hmax.max(h);
                    vmax = vmax.max(v);
                    raw.push((h, v, tq));
                }
                let one_based = ids.iter().enumerate().all(|(i, &id)| id == i as u8 + 1);
                let zero_based = n == 3 && ids.iter().enumerate().all(|(i, &id)| id == i as u8);
                let distinct = (1..n).all(|i| !ids[..i].contains(&ids[i]));
                if !(one_based || zero_based || n == 1 || (any_ids && distinct)) {
                    return None;
                }
                comp_ids = ids;
                approx = vec![[-1; 64]; n];
                mcu_w = ceil_div(width as usize, 8 * hmax);
                mcu_h = ceil_div(height as usize, 8 * vmax);
                let mut total = 0usize;
                for &(h, v, tq) in &raw {
                    let cw = ceil_div(width as usize * h, hmax);
                    let ch = ceil_div(height as usize * v, vmax);
                    let (bw, bh) = (mcu_w * h, mcu_h * v);
                    total += bw * bh;
                    if total > MAX_TOTAL_BLOCKS {
                        return None;
                    }
                    comps.push(FComp {
                        h,
                        v,
                        tq,
                        bw,
                        aw: ceil_div(cw, 8),
                        ah: ceil_div(ch, 8),
                        blocks: vec![[0i16; 64]; bw * bh],
                    });
                }
                frame = Some((marker == 0xC2, width, height));
                frame_at = seg_range.start;
            }
            0xDA => {
                let (progressive, _, _) = *frame.as_ref()?;
                if !seen_scan {
                    scan_at = seg_range.start;
                }
                scans += 1;
                if scans > limits.max_scans {
                    return None;
                }
                // SOS header.
                if payload.is_empty() {
                    return None;
                }
                let ns = payload[0] as usize;
                if ns == 0 || ns > 4 || payload.len() != 4 + 2 * ns {
                    return None;
                }
                let mut sc: Vec<ScanComp> = Vec::new();
                for i in 0..ns {
                    let cs = payload[1 + 2 * i];
                    let ci = comp_ids.iter().position(|&id| id == cs)?;
                    if sc.iter().any(|s| s.ci == ci) {
                        return None;
                    }
                    let tdta = payload[2 + 2 * i];
                    sc.push(ScanComp {
                        ci,
                        dc: (tdta >> 4) as usize,
                        ac: (tdta & 0x0F) as usize,
                    });
                    if sc[i].dc > 3 || sc[i].ac > 3 {
                        return None;
                    }
                }
                let ss = payload[1 + 2 * ns] as usize;
                let se = payload[2 + 2 * ns] as usize;
                let ahal = payload[3 + 2 * ns];
                let (ah, al) = ((ahal >> 4) as u32, (ahal & 0x0F) as u32);
                // T.81 G.1.1.1.2: a coefficient's first scan comes once, and
                // each refinement lowers Al by one from where the last scan
                // left it. Enforcing it keeps every scan to new bits: a
                // repeated or unanchored refinement would visit every block
                // again while reading next to nothing, so a small file could
                // ask for minutes of work.
                if progressive && ss <= se && se < 64 {
                    for s in &sc {
                        let state = &mut approx[s.ci][ss..=se];
                        let want = if ah == 0 { -1 } else { ah as i8 };
                        if state.iter().any(|&a| a != want) {
                            return None;
                        }
                        state.fill(al as i8);
                    }
                }
                let end = decode_scan(
                    data,
                    pos,
                    &mut comps,
                    &sc,
                    (ss, se, ah, al),
                    progressive,
                    ns,
                    mcu_w,
                    mcu_h,
                    restart_interval,
                    &dc_tables,
                    &ac_tables,
                )?;
                seen_scan = true;
                pos = end;
            }
            // Arithmetic coding, hierarchical mode, other SOFs, DNL: out of
            // scope — refuse rather than risk a lossy round trip.
            _ => return None,
        }
    }

    let (_, width, height) = frame?;
    if !seen_scan {
        return None;
    }
    // Re-encoding as baseline requires every coefficient in the T.81
    // 8-bit-precision range (DC in [-1024, 1023] keeps every DPCM diff in
    // category <= 11). Valid files always satisfy this.
    for c in &comps {
        for blk in &c.blocks {
            // Branch-free min/max over the AC part (vectorizes), DC apart.
            let (lo, hi) = blk[1..]
                .iter()
                .fold((0i16, 0i16), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            if blk[0] < -1024 || blk[0] > 1023 || lo < -1023 || hi > 1023 {
                return None;
            }
        }
    }
    Some(Parsed {
        width,
        height,
        comps,
        meta,
        dqt,
        frame_at,
        scan_at,
        mcu_w,
        mcu_h,
        qts,
        adobe,
        jfif,
        ids: comp_ids,
    })
}

// ---------------------------------------------------------- scan decoding

/// Decode one scan's entropy data starting at `pos`; returns the position
/// just past it. Coefficients accumulate into `comps[..].blocks` in zigzag
/// order, exactly as the crate's emitters expect them.
#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    pos: usize,
    comps: &mut [FComp],
    sc: &[ScanComp],
    (ss, se, ah, al): (usize, usize, u32, u32),
    progressive: bool,
    ns: usize,
    mcu_w: usize,
    mcu_h: usize,
    ri: usize,
    dc_tables: &[Option<DecHuff>; 4],
    ac_tables: &[Option<DecHuff>; 4],
) -> Option<usize> {
    // Interleaved scans must cover all frame components (subset-interleaved
    // scans are legal but vanishingly rare; refuse rather than mis-decode).
    if ns > 1 && ns != comps.len() {
        return None;
    }
    if al > 13 {
        return None;
    }
    let mut r = BitReader::new(data, pos);
    let mut preds = vec![0i32; comps.len()];
    let mut eobrun = 0u32;
    let mut units_done = 0usize;
    let mut rst = 0u8;

    // DC band (sequential scans are ss == 0 too, handled separately below).
    enum Kind {
        Seq,
        DcFirst,
        DcRefine,
        AcFirst,
        AcRefine,
    }
    let kind = if !progressive {
        if ss != 0 || se != 63 || ah != 0 || al != 0 {
            return None;
        }
        Kind::Seq
    } else if ss == 0 {
        if se != 0 {
            return None; // progressive DC scan carries DC only
        }
        if ah == 0 {
            Kind::DcFirst
        } else {
            if ah != al + 1 {
                return None;
            }
            Kind::DcRefine
        }
    } else {
        if ns != 1 || ss > se || se > 63 {
            return None;
        }
        if ah == 0 {
            Kind::AcFirst
        } else {
            if ah != al + 1 {
                return None;
            }
            Kind::AcRefine
        }
    };

    // One "unit" is an MCU (interleaved) or a single block (ns == 1).
    let units = if ns > 1 {
        mcu_w * mcu_h
    } else {
        let c = &comps[sc[0].ci];
        c.aw * c.ah
    };

    let mut unit_blocks: Vec<(usize, usize)> = Vec::new();
    for u in 0..units {
        if ri > 0 && units_done == ri {
            r.restart(rst)?;
            rst = (rst + 1) & 7;
            preds.iter_mut().for_each(|p| *p = 0);
            eobrun = 0;
            units_done = 0;
        }
        units_done += 1;
        unit_blocks.clear();
        if ns > 1 {
            let (my, mx) = (u / mcu_w, u % mcu_w);
            for (si, s) in sc.iter().enumerate() {
                let c = &comps[s.ci];
                for dy in 0..c.v {
                    for dx in 0..c.h {
                        unit_blocks.push((si, (my * c.v + dy) * c.bw + (mx * c.h + dx)));
                    }
                }
            }
        } else {
            let c = &comps[sc[0].ci];
            unit_blocks.push((0, (u / c.aw) * c.bw + (u % c.aw)));
        }

        for &(si, bi) in unit_blocks.iter() {
            let s = &sc[si];
            let ci = s.ci;
            let blk = &mut comps[ci].blocks[bi];
            match kind {
                Kind::Seq => {
                    let dc = dc_tables[s.dc].as_ref()?;
                    let ac = ac_tables[s.ac].as_ref()?;
                    decode_block_seq(&mut r, blk, dc, ac, &mut preds[ci])?;
                }
                Kind::DcFirst => {
                    let dc = dc_tables[s.dc].as_ref()?;
                    let t = r.huff(dc)? as u32;
                    if t > 11 {
                        return None;
                    }
                    let diff = if t > 0 { extend(r.bits(t)?, t) } else { 0 };
                    preds[ci] += diff;
                    let v = preds[ci] << al;
                    if v < i16::MIN as i32 || v > i16::MAX as i32 {
                        return None;
                    }
                    blk[0] = v as i16;
                }
                Kind::DcRefine => {
                    if r.bit()? == 1 {
                        blk[0] |= (1i32 << al) as i16;
                    }
                }
                Kind::AcFirst => {
                    let ac = ac_tables[s.ac].as_ref()?;
                    decode_block_ac_first(&mut r, blk, ac, ss, se, al, &mut eobrun)?;
                }
                Kind::AcRefine => {
                    let ac = ac_tables[s.ac].as_ref()?;
                    decode_block_ac_refine(&mut r, blk, ac, ss, se, al, &mut eobrun)?;
                }
            }
        }
    }
    if eobrun != 0 {
        return None; // EOB run promising more blocks than the scan has
    }

    // Entropy data ends at the next marker; the reader stopped in front of
    // it (any buffered leftover bits are padding).
    Some(r.pos)
}

fn decode_block_seq(
    r: &mut BitReader,
    blk: &mut [i16; 64],
    dc: &DecHuff,
    ac: &DecHuff,
    pred: &mut i32,
) -> Option<()> {
    let t = r.huff(dc)? as u32;
    if t > 11 {
        return None;
    }
    let diff = if t > 0 { extend(r.bits(t)?, t) } else { 0 };
    *pred += diff;
    if *pred < i16::MIN as i32 || *pred > i16::MAX as i32 {
        return None;
    }
    blk[0] = *pred as i16;
    let mut k = 1usize;
    while k <= 63 {
        let rs = r.huff(ac)?;
        let (run, s) = ((rs >> 4) as usize, (rs & 0x0F) as u32);
        if s == 0 {
            if run == 15 {
                k += 16; // ZRL
                continue;
            }
            if run != 0 {
                return None; // only 0x00 (EOB) and 0xF0 (ZRL) have size 0
            }
            break; // EOB
        }
        k += run;
        if k > 63 || s > 10 {
            return None;
        }
        blk[k] = extend(r.bits(s)?, s) as i16;
        k += 1;
    }
    Some(())
}

fn decode_block_ac_first(
    r: &mut BitReader,
    blk: &mut [i16; 64],
    ac: &DecHuff,
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Option<()> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Some(());
    }
    let mut k = ss;
    while k <= se {
        let rs = r.huff(ac)?;
        let (run, s) = ((rs >> 4) as usize, (rs & 0x0F) as u32);
        if s == 0 {
            if run == 15 {
                k += 16; // ZRL
                continue;
            }
            *eobrun = (1u32 << run) - 1 + if run > 0 { r.bits(run as u32)? } else { 0 };
            break;
        }
        k += run;
        if k > se || s > 10 || s + al > 14 {
            return None;
        }
        blk[k] = (extend(r.bits(s)?, s) << al) as i16;
        k += 1;
    }
    Some(())
}

/// T.81 G.1.2.3 successive-approximation AC refinement: previously nonzero
/// coefficients get one correction bit, newly significant coefficients
/// arrive as (run, 1) symbols whose run counts zero-history positions only.
fn decode_block_ac_refine(
    r: &mut BitReader,
    blk: &mut [i16; 64],
    ac: &DecHuff,
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Option<()> {
    let p1 = 1i16 << al;
    let m1 = -p1;
    let mut k = ss;
    if *eobrun == 0 {
        while k <= se {
            let rs = r.huff(ac)?;
            let (mut run, s) = ((rs >> 4) as u32, (rs & 0x0F) as u32);
            let mut newval = 0i16;
            if s == 0 {
                if run < 15 {
                    // Unlike the first-scan convention, this count INCLUDES
                    // the current block: its remaining coefficients still
                    // receive correction bits below before the run starts.
                    *eobrun = (1u32 << run) + if run > 0 { r.bits(run)? } else { 0 };
                    break;
                }
                // run == 15: sixteen zero-history positions, no new value.
            } else {
                if s != 1 {
                    return None;
                }
                newval = if r.bit()? == 1 { p1 } else { m1 };
            }
            while k <= se {
                let c = blk[k];
                if c != 0 {
                    if r.bit()? == 1 && (c & p1) == 0 {
                        blk[k] = refine(c, p1)?;
                    }
                } else {
                    if run == 0 {
                        break;
                    }
                    run -= 1;
                }
                k += 1;
            }
            if newval != 0 {
                if k > se {
                    return None;
                }
                blk[k] = newval;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= se {
            let c = blk[k];
            if c != 0 && r.bit()? == 1 && (c & p1) == 0 {
                blk[k] = refine(c, p1)?;
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Some(())
}

/// Apply a correction bit: move `c` one `p1` step away from zero. A file
/// whose successive-approximation scans are inconsistent (Al not stepping
/// down) can push a coefficient past i16; that is refused, not wrapped.
fn refine(c: i16, p1: i16) -> Option<i16> {
    if c > 0 {
        c.checked_add(p1)
    } else {
        c.checked_sub(p1)
    }
}

// -------------------------------------------------------------- re-encode

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanSearch {
    Baseline,
    Thorough,
    Maximum,
}

fn reencode(data: &[u8], p: Parsed, search: ScanSearch) -> Vec<u8> {
    let placed = Placed::new(data, &p, |_| {}, |seg, out| out.extend_from_slice(seg));
    encode_parsed(placed, p, search)
}

/// The non-image segments of re-encoded output, by where they go.
///
/// Readers treat APPn/COM segments differently by position: header probes
/// (and this crate's `jpeg_info`) stop at the frame header, common decoders
/// collect markers up to the first scan, and markers between scans are
/// rarely read. So each segment stays on its side of those two points.
pub(crate) struct Placed {
    /// SOI, the APPn/COM segments before the frame header, and every DQT.
    pub(crate) head: Vec<u8>,
    /// APPn/COM segments between the frame header and the first scan.
    pub(crate) after_frame: Vec<u8>,
    /// APPn/COM segments after the first scan; written before EOI.
    pub(crate) before_end: Vec<u8>,
}

impl Placed {
    /// Copy `p`'s segments from `data`, letting `edit` change each APPn/COM
    /// segment (marker included) on the way; `dqt` writes each DQT segment.
    pub(crate) fn new(
        data: &[u8],
        p: &Parsed,
        mut edit: impl FnMut(&mut [u8]),
        mut dqt: impl FnMut(&[u8], &mut Vec<u8>),
    ) -> Placed {
        let mut placed = Placed {
            head: Vec::with_capacity(1024),
            after_frame: Vec::new(),
            before_end: Vec::new(),
        };
        placed.head.extend_from_slice(&[0xFF, 0xD8]);
        for r in &p.meta {
            let out = if r.start < p.frame_at {
                &mut placed.head
            } else if r.start < p.scan_at {
                &mut placed.after_frame
            } else {
                &mut placed.before_end
            };
            let at = out.len();
            out.extend_from_slice(&data[r.clone()]);
            edit(&mut out[at..]);
        }
        for r in &p.dqt {
            dqt(&data[r.clone()], &mut placed.head);
        }
        placed
    }
}

/// Write `p`'s coefficients around `placed`'s segments: baseline, and
/// progressive when allowed, keeping the smaller.
fn encode_parsed(placed: Placed, p: Parsed, search: ScanSearch) -> Vec<u8> {
    let Placed {
        head,
        after_frame,
        before_end,
    } = placed;
    // Wrap the decoded grids in the crate's Component so the existing
    // emitters (optimized-Huffman baseline, scan-optimized progressive)
    // work unchanged. Component 0 gets table class 0, the rest share
    // class 1 — the conventional assignment.
    let comps: Vec<Component> = p
        .comps
        .into_iter()
        .enumerate()
        .map(|(i, c)| Component {
            blocks: c.blocks,
            table_class: usize::from(i != 0),
            bw: c.bw,
            aw: c.aw,
            ah: c.ah,
            h: c.h as u8,
            v: c.v as u8,
            qt_id: c.tq,
        })
        .collect();
    let order = interleave_order(&comps, p.mcu_w, p.mcu_h);
    let emit_sof = |out: &mut Vec<u8>, marker: u8| {
        let sof_len = 8 + 3 * comps.len() as u16;
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&sof_len.to_be_bytes());
        out.push(8);
        out.extend_from_slice(&p.height.to_be_bytes());
        out.extend_from_slice(&p.width.to_be_bytes());
        out.push(comps.len() as u8);
        for (i, c) in comps.iter().enumerate() {
            out.push(i as u8 + 1);
            out.push((c.h << 4) | c.v);
            out.push(c.qt_id);
        }
    };

    // Baseline candidate: one interleaved scan, per-image Huffman tables.
    let n_tables = comps.iter().map(|c| c.table_class).max().unwrap_or(0) * 2 + 2;
    let baseline = {
        let mut out = head.clone();
        emit_sof(&mut out, 0xC0);
        out.extend_from_slice(&after_frame);
        let mut freqs = vec![[0u32; 256]; n_tables];
        for_each_symbol(&comps, &order, |tbl, sym, _n, _bits| {
            freqs[tbl][sym as usize] += 1;
        });
        let tables: Vec<HuffTable> = freqs.iter().map(build_huff_table).collect();
        for (id, t) in tables.iter().enumerate() {
            emit_dht(&mut out, (id & 1) as u8, (id / 2) as u8, t);
        }
        let sos_len = 6 + 2 * comps.len() as u16;
        out.extend_from_slice(&[0xFF, 0xDA]);
        out.extend_from_slice(&sos_len.to_be_bytes());
        out.push(comps.len() as u8);
        for (i, c) in comps.iter().enumerate() {
            out.push(i as u8 + 1);
            out.push(((c.table_class as u8) << 4) | c.table_class as u8);
        }
        out.extend_from_slice(&[0x00, 0x3F, 0x00]);
        let mut writer = BitWriter::new(out);
        for_each_symbol(&comps, &order, |tbl, sym, n, bits| {
            let (code, len) = tables[tbl].codes[sym as usize];
            writer.put(code, len);
            writer.put(bits, n);
        });
        writer.flush();
        let mut out = writer.out;
        out.extend_from_slice(&before_end);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    };

    if search == ScanSearch::Baseline {
        return baseline;
    }
    // Progressive candidate with the measured scan script; keep the smaller.
    let mut prog_head = head;
    emit_sof(&mut prog_head, 0xC2);
    prog_head.extend_from_slice(&after_frame);
    let mut prog =
        encode_progressive_scans(&comps, &order, prog_head, search == ScanSearch::Maximum);
    let eoi = prog.len() - 2;
    prog.splice(eoi..eoi, before_end);
    if prog.len() <= baseline.len() {
        prog
    } else {
        baseline
    }
}

// ------------------------------------------------------------------ tests
