use std::io::{self, Read};

pub(crate) const MAX_FILE: usize = 64 << 20;
pub(crate) const MAX_PIXELS: usize = 20_000_000;
pub(crate) const MAX_METADATA: usize = 8 << 20;
const MAX_SCANS: usize = 96;

pub(crate) fn check_jpeg(data: &[u8]) -> Result<(), String> {
    if data.len() > MAX_FILE || !data.starts_with(&[0xff, 0xd8]) {
        return Err("Expected a JPEG of at most 64 MiB".into());
    }
    let mut p = 2;
    let mut in_scan = false;
    let mut scans = 0;
    let mut frames = 0;
    let mut markers = 0;
    let mut meta = 0;
    loop {
        if in_scan {
            p += data[p..]
                .iter()
                .position(|&b| b == 0xff)
                .ok_or("JPEG scan has no terminator")?;
        }
        if data.get(p) != Some(&0xff) {
            return Err("Invalid JPEG marker".into());
        }
        while data.get(p) == Some(&0xff) {
            p += 1;
        }
        let marker = *data.get(p).ok_or("Truncated JPEG marker")?;
        p += 1;
        if in_scan && (marker == 0 || (0xd0..=0xd7).contains(&marker)) {
            continue;
        }
        in_scan = false;
        markers += 1;
        if markers > 8192 {
            return Err("Too many JPEG markers".into());
        }
        if marker == 0xd9 {
            if frames != 1 || scans == 0 || p != data.len() {
                return Err("Only complete single-image JPEGs can be archived".into());
            }
            return Ok(());
        }
        let size = data.get(p..p + 2).ok_or("Truncated JPEG length")?;
        let n = usize::from(u16::from_be_bytes([size[0], size[1]]));
        if n < 2 {
            return Err("Invalid JPEG segment length".into());
        }
        let payload = data.get(p + 2..p + n).ok_or("Incomplete JPEG segment")?;
        p += n;
        match marker {
            0xc0..=0xc2 => {
                if payload.len() < 6 {
                    return Err("Truncated JPEG frame".into());
                }
                let h = usize::from(u16::from_be_bytes([payload[1], payload[2]]));
                let w = usize::from(u16::from_be_bytes([payload[3], payload[4]]));
                let channels = usize::from(payload[5]);
                if frames != 0
                    || !matches!(channels, 1 | 3)
                    || payload[0] != 8
                    || payload.len() != 6 + 3 * channels
                    || w == 0
                    || h == 0
                    || w * h > MAX_PIXELS
                {
                    return Err("Archive mode supports single 8-bit grayscale and three-component JPEGs up to 20 megapixels".into());
                }
                let mut sampling = 0usize;
                for c in payload[6..].as_chunks::<3>().0 {
                    let (h, v) = (c[1] >> 4, c[1] & 15);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                        return Err("Invalid JPEG sampling".into());
                    }
                    sampling += usize::from(h * v);
                }
                if sampling > 10 {
                    return Err("Unsupported JPEG sampling layout".into());
                }
                frames += 1;
            }
            0xda => {
                if frames != 1 {
                    return Err("JPEG scan precedes frame".into());
                }
                scans += 1;
                if scans > MAX_SCANS {
                    return Err("Too many JPEG scans".into());
                }
                in_scan = true;
            }
            0xe0..=0xef | 0xfe => {
                if (marker == 0xe2 && payload.starts_with(b"MPF\0")) || marker == 0xeb {
                    return Err("Compound or signed JPEGs are left in their original format".into());
                }
                meta += payload.len();
                if meta > MAX_METADATA {
                    return Err("JPEG metadata exceeds 8 MiB".into());
                }
            }
            0xc4 | 0xdb | 0xdd => {}
            _ => return Err("Unsupported JPEG coding mode or layout".into()),
        }
    }
}

pub(super) fn brotli_size(data: &[u8], limit: usize) -> Result<usize, String> {
    if data.is_empty() || data[0] == 0x11 {
        return Err("Unsupported Brotli metadata window".into());
    }
    let decoder = brotli_decompressor::Decompressor::new(data, 4096);
    let n = io::copy(&mut decoder.take(limit as u64 + 1), &mut io::sink())
        .map_err(|e| format!("Invalid Brotli metadata: {e}"))?;
    if n > limit as u64 {
        return Err("Expanded JPEG XL metadata exceeds the archive limit".into());
    }
    Ok(n as usize)
}

// A cheap, allocation-free walk over the standardized jbrd prefix locates
// its embedded Brotli stream. The decoder's pixel budget does not by itself
// bound this metadata. Check both declared and actually expanded byte counts.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}
impl Bits<'_> {
    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.pos = self
            .pos
            .checked_add(n)
            .filter(|&p| p <= self.data.len() * 8)
            .ok_or("Truncated JPEG reconstruction header")?;
        Ok(())
    }
    fn get(&mut self, n: usize) -> Result<usize, String> {
        let start = self.pos;
        self.skip(n)?;
        let mut v = 0;
        for i in 0..n {
            let bit = start + i;
            v |= usize::from((self.data[bit / 8] >> (bit % 8)) & 1) << i;
        }
        Ok(v)
    }
    fn number(&mut self, forms: [(usize, usize); 4]) -> Result<usize, String> {
        let i = self.get(2)?;
        let (width, base) = forms[i];
        Ok(base + self.get(width)?)
    }
}

fn jbrd_size(data: &[u8]) -> Result<usize, String> {
    if data.len() > MAX_METADATA {
        return Err("Oversized JPEG reconstruction box".into());
    }
    let mut r = Bits { data, pos: 0 };
    let channels = if r.get(1)? == 1 { 1 } else { 3 };
    let (mut app, mut comments, mut gaps, mut scans, mut dri) = (0, 0, 0, 0, false);
    let mut markers = 0;
    loop {
        markers += 1;
        if markers > 8192 {
            return Err("Too many reconstruction markers".into());
        }
        match r.get(6)? + 0xc0 {
            0xd9 => break,
            0xda => scans += 1,
            0xe0..=0xef => app += 1,
            0xfe => comments += 1,
            0xff => gaps += 1,
            0xdd => dri = true,
            _ => {}
        }
    }
    if scans == 0 || scans > MAX_SCANS {
        return Err("Invalid reconstruction scan count".into());
    }
    let mut declared = 0;
    let mut embedded = 0;
    for _ in 0..app {
        let kind = r.number([(0, 0), (0, 1), (1, 2), (2, 4)])?;
        if kind > 3 {
            return Err("Unknown JPEG marker representation".into());
        }
        let n = r.get(16)? + 1;
        declared += n;
        if kind == 0 {
            embedded += n;
        }
    }
    for _ in 0..comments {
        let n = r.get(16)? + 1;
        declared += n;
        embedded += n;
    }
    if declared > MAX_METADATA {
        return Err("JPEG reconstruction metadata exceeds 8 MiB".into());
    }
    let quant = r.get(2)? + 1;
    r.skip(4 * quant)?;
    let ty = r.get(2)?;
    let components = match ty {
        0 => 1,
        1 | 2 => 3,
        _ => {
            let n = r.get(2)? + 1;
            r.skip(8 * n)?;
            n
        }
    };
    if components != channels {
        return Err("JPEG reconstruction component mismatch".into());
    }
    r.skip(channels * 2)?;
    let tables = r.number([(0, 4), (3, 2), (4, 10), (6, 26)])?;
    for _ in 0..tables {
        r.skip(4)?;
        let mut symbols = 0;
        for depth in 0..17 {
            let n = r.number([(0, 0), (0, 1), (3, 2), (8, 0)])?;
            if depth == 0 && n != 0 {
                return Err("Invalid zero-length Huffman code".into());
            }
            symbols += n;
        }
        if !(1..=257).contains(&symbols) {
            return Err("Invalid JPEG Huffman alphabet".into());
        }
        for _ in 0..symbols {
            r.number([(2, 0), (2, 4), (4, 8), (8, 1)])?;
        }
    }
    for _ in 0..scans {
        let nc = r.get(2)? + 1;
        r.skip(20 + 6 * nc)?;
        r.number([(0, 0), (0, 1), (0, 2), (3, 3)])?;
    }
    if dri {
        r.skip(16)?;
    }
    let mut tokens = 0;
    for _ in 0..scans {
        for extra in [false, true] {
            let count = r.number([(0, 0), (2, 1), (4, 4), (16, 20)])?;
            tokens += count;
            if tokens > 1_048_576 {
                return Err("Too many JPEG reconstruction exceptions".into());
            }
            for _ in 0..count {
                if extra {
                    r.number([(0, 1), (2, 2), (4, 5), (8, 20)])?;
                }
                r.number([(0, 0), (3, 1), (5, 9), (28, 41)])?;
            }
        }
    }
    for _ in 0..gaps {
        let n = r.get(16)?;
        embedded += n;
        declared += n;
    }
    let tail = r.number([(0, 0), (8, 1), (16, 257), (22, 65793)])?;
    if tail != 0 {
        return Err("JPEG attachments are unsupported in archive mode".into());
    }
    if declared > MAX_METADATA {
        return Err("JPEG reconstruction payload exceeds 8 MiB".into());
    }
    if r.get(1)? == 1 {
        let padding = r.get(24)?;
        if padding > 8 * 1024 * 1024 {
            return Err("Excessive JPEG reconstruction padding".into());
        }
        r.skip(padding)?;
    }
    let compressed = &data[r.pos.div_ceil(8)..];
    if brotli_size(compressed, embedded)? != embedded {
        return Err("Reconstruction metadata size mismatch".into());
    }
    Ok(declared)
}

pub(crate) fn check_container(data: &[u8]) -> Result<(), String> {
    const MAGIC: &[u8] = b"\0\0\0\x0cJXL \r\n\x87\n";
    if data.len() > MAX_FILE || !data.starts_with(MAGIC) {
        return Err("Expected a JPEG XL container of at most 64 MiB".into());
    }
    let mut p = MAGIC.len();
    let mut boxes = 0;
    let mut jbrd = false;
    let mut stream_complete = false;
    let mut fragments = 0u32;
    let mut meta = 0;
    let mut ftyp = false;
    while p < data.len() {
        let head = data.get(p..p + 8).ok_or("Truncated JPEG XL box")?;
        let n = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
        let (length, header) = match n {
            0 => (data.len() - p, 8),
            1 => {
                let ext = data
                    .get(p + 8..p + 16)
                    .ok_or("Truncated extended JPEG XL box")?;
                (
                    usize::try_from(u64::from_be_bytes(ext.try_into().unwrap()))
                        .map_err(|_| "Invalid JPEG XL size")?,
                    16,
                )
            }
            _ => (n, 8),
        };
        if length < header || length > data.len() - p {
            return Err("Invalid JPEG XL box length".into());
        }
        let kind = &head[4..];
        let payload = &data[p + header..p + length];
        p += length;
        boxes += 1;
        if boxes > 1024 {
            return Err("Too many JPEG XL boxes".into());
        }
        match kind {
            b"ftyp"
                if boxes == 1
                    && payload.len() >= 12
                    && payload.len().is_multiple_of(4)
                    && payload.starts_with(b"jxl ")
                    && payload[8..].as_chunks::<4>().0.contains(b"jxl ") =>
            {
                ftyp = true;
            }
            b"jbrd" if !jbrd => {
                jbrd = true;
                meta += jbrd_size(payload)?;
            }
            b"jxlc" if !stream_complete && fragments == 0 && payload.starts_with(&[0xff, 0x0a]) => {
                stream_complete = true;
            }
            b"jxlp" if !stream_complete && payload.len() > 4 => {
                let id = u32::from_be_bytes(payload[..4].try_into().unwrap());
                if id & 0x7fff_ffff != fragments
                    || (fragments == 0 && !payload[4..].starts_with(&[0xff, 0x0a]))
                {
                    return Err("Invalid or out-of-order JPEG XL codestream fragments".into());
                }
                fragments += 1;
                stream_complete = id >> 31 != 0;
            }
            b"Exif" | b"xml " => meta += payload.len(),
            b"brob" if payload.starts_with(b"Exif") || payload.starts_with(b"xml ") => {
                meta += brotli_size(&payload[4..], MAX_METADATA)?;
            }
            b"jxll" if payload.len() == 1 => {}
            _ => return Err("Unsupported JPEG XL container box".into()),
        }
        // The jbrd declared APP counts include ICC/EXIF/XMP stored in separate boxes.
        if meta > 2 * MAX_METADATA {
            return Err("JPEG XL metadata exceeds the archive limit".into());
        }
    }
    if !ftyp || !jbrd || !stream_complete {
        return Err("No complete JPEG reconstruction data in this JXL".into());
    }
    Ok(())
}
