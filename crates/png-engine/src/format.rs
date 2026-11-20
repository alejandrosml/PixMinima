use std::ops::Range;

pub(crate) const MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_FILE: usize = 256 * 1024 * 1024;
const MAX_CHUNKS: usize = 100_000;
const MAX_STREAM: usize = 128 * 1024 * 1024;
const MAX_DECODED: usize = 256 * 1024 * 1024;

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < table.len() {
        let mut v = i as u32;
        let mut j = 0;
        while j < 8 {
            v = if v & 1 != 0 {
                0xedb88320 ^ (v >> 1)
            } else {
                v >> 1
            };
            j += 1;
        }
        table[i] = v;
        i += 1;
    }
    table
}
const CRC: [u32; 256] = crc_table();

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    !bytes
        .iter()
        .fold(u32::MAX, |v, &b| (v >> 8) ^ CRC[((v as u8) ^ b) as usize])
}
pub(crate) fn word(bytes: &[u8]) -> usize {
    u32::from_be_bytes(bytes[..4].try_into().expect("checked PNG field length")) as usize
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Header {
    pub width: usize,
    pub height: usize,
    pub depth: u8,
    pub color: u8,
    pub interlaced: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Pass {
    pub width: usize,
    pub rows: usize,
    pub bytes: usize,
}

impl Header {
    pub fn channels(self) -> usize {
        match self.color {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            6 => 4,
            _ => unreachable!(),
        }
    }
    pub fn bpp(self) -> usize {
        (self.channels() * usize::from(self.depth)).div_ceil(8)
    }
    pub fn passes(self) -> Result<(Vec<Pass>, usize), String> {
        const ADAM: [(usize, usize, usize, usize); 7] = [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ];
        let pattern: &[_] = if self.interlaced {
            &ADAM
        } else {
            &[(0, 0, 1, 1)]
        };
        let mut passes = Vec::new();
        let mut total = 0usize;
        for &(x, y, dx, dy) in pattern {
            let width = self.width.saturating_sub(x).div_ceil(dx);
            let rows = self.height.saturating_sub(y).div_ceil(dy);
            if width == 0 || rows == 0 {
                continue;
            }
            let bytes = width
                .checked_mul(self.channels() * usize::from(self.depth))
                .ok_or("PNG row length overflow")?
                .div_ceil(8);
            total = bytes
                .checked_add(1)
                .and_then(|s| s.checked_mul(rows))
                .and_then(|s| s.checked_add(total))
                .filter(|&n| n <= MAX_STREAM)
                .ok_or("PNG exceeds the 128 MiB decoded frame limit")?;
            passes.push(Pass { width, rows, bytes });
        }
        Ok((passes, total))
    }
}

pub(crate) struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
    full: &'a [u8],
}

pub(crate) struct Stream {
    pub header: Header,
    pub chunks: Vec<usize>,
}
pub(crate) struct Image<'a> {
    pub header: Header,
    pub chunks: Vec<Chunk<'a>>,
    pub streams: Vec<Stream>,
    pub palette: Option<usize>,
    pub preserve_entire_file: bool,
}

fn parse_header(data: &[u8]) -> Result<Header, String> {
    if data.len() != 13 {
        return Err("Invalid IHDR length".into());
    }
    let w = word(data);
    let h = word(&data[4..]);
    let d = data[8];
    let c = data[9];
    let valid = match c {
        0 => matches!(d, 1 | 2 | 4 | 8 | 16),
        3 => matches!(d, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(d, 8 | 16),
        _ => false,
    };
    if w == 0
        || h == 0
        || w > i32::MAX as usize
        || h > i32::MAX as usize
        || !valid
        || data[10..12] != [0, 0]
        || data[12] > 1
    {
        return Err("Invalid PNG dimensions, depth or coding method".into());
    }
    Ok(Header {
        width: w,
        height: h,
        depth: d,
        color: c,
        interlaced: data[12] == 1,
    })
}
fn scan(input: &[u8]) -> Result<Vec<Chunk<'_>>, String> {
    if input.len() > MAX_FILE || !input.starts_with(MAGIC) {
        return Err("Invalid PNG signature or file over 256 MiB".into());
    }
    let mut chunks = Vec::new();
    let mut tail = &input[MAGIC.len()..];
    while !tail.is_empty() {
        if tail.len() < 12 || chunks.len() >= MAX_CHUNKS {
            return Err("Incomplete PNG chunk or chunk limit exceeded".into());
        }
        let n = word(tail);
        if n > tail.len() - 12 {
            return Err("Truncated PNG chunk".into());
        }
        let kind: [u8; 4] = tail[4..8].try_into().unwrap();
        if !kind.iter().all(u8::is_ascii_alphabetic)
            || !kind[2].is_ascii_uppercase()
            || crc32(&tail[4..8 + n]) != word(&tail[8 + n..12 + n]) as u32
        {
            return Err("Invalid PNG chunk type or CRC".into());
        }
        let full = &tail[..12 + n];
        chunks.push(Chunk {
            kind,
            data: &tail[8..8 + n],
            full,
        });
        tail = &tail[12 + n..];
        if &kind == b"IEND" && !tail.is_empty() {
            return Err("PNG has data after IEND".into());
        }
    }
    Ok(chunks)
}
fn sequence(data: &[u8], next: &mut usize) -> Result<(), String> {
    if data.len() < 4 || word(data) != *next {
        return Err("Invalid APNG sequence number".into());
    }
    *next += 1;
    Ok(())
}

impl<'a> Image<'a> {
    pub fn parse(input: &'a [u8]) -> Result<Self, String> {
        let chunks = scan(input)?;
        if chunks.first().map(|c| c.kind) != Some(*b"IHDR")
            || !chunks
                .last()
                .is_some_and(|c| &c.kind == b"IEND" && c.data.is_empty())
        {
            return Err("Missing PNG header or end marker".into());
        }
        let header = parse_header(chunks[0].data)?;
        let mut streams = vec![Stream {
            header,
            chunks: Vec::new(),
        }];
        let mut palette = None;
        let mut control = None;
        let mut controls = 0;
        let mut next = 0;
        let mut ended_idat = false;
        let mut preserve_entire_file = false;
        let mut unique = std::collections::HashSet::new();
        for (i, c) in chunks.iter().enumerate().skip(1) {
            let data = c.data;
            let seen_idat = !streams[0].chunks.is_empty();
            if seen_idat && &c.kind != b"IDAT" {
                ended_idat = true;
            }
            if matches!(
                &c.kind,
                b"bKGD"
                    | b"cHRM"
                    | b"cICP"
                    | b"cLLI"
                    | b"eXIf"
                    | b"gAMA"
                    | b"hIST"
                    | b"iCCP"
                    | b"mDCV"
                    | b"pHYs"
                    | b"sBIT"
                    | b"sRGB"
                    | b"tIME"
                    | b"tRNS"
                    | b"sTER"
            ) && !unique.insert(c.kind)
            {
                return Err("Duplicate singleton PNG chunk".into());
            }
            match &c.kind {
                b"PLTE" => {
                    let n = data.len() / 3;
                    if seen_idat
                        || palette.is_some()
                        || n == 0
                        || n > 256
                        || data.len() % 3 != 0
                        || matches!(header.color, 0 | 4)
                        || (header.color == 3 && n > 1usize << header.depth)
                    {
                        return Err("Invalid PNG palette".into());
                    }
                    palette = Some(n);
                }
                b"IDAT" => {
                    if ended_idat || (header.color == 3 && palette.is_none()) {
                        return Err("Invalid PNG IDAT order".into());
                    }
                    streams[0].chunks.push(i);
                }
                b"acTL" => {
                    if control.is_some() || seen_idat || data.len() != 8 || word(data) == 0 {
                        return Err("Invalid APNG animation control".into());
                    }
                    control = Some(word(data));
                }
                b"fcTL" => {
                    if control.is_none()
                        || data.len() != 26
                        || (controls > 0 && streams.last().unwrap().chunks.is_empty())
                    {
                        return Err("Invalid APNG frame control".into());
                    }
                    sequence(data, &mut next)?;
                    let w = word(&data[4..]);
                    let h = word(&data[8..]);
                    let x = word(&data[12..]);
                    let y = word(&data[16..]);
                    if w == 0
                        || h == 0
                        || w > header.width
                        || h > header.height
                        || x > header.width - w
                        || y > header.height - h
                        || data[24] > 2
                        || data[25] > 1
                    {
                        return Err("Invalid APNG frame bounds or operations".into());
                    }
                    if seen_idat {
                        streams.push(Stream {
                            header: Header {
                                width: w,
                                height: h,
                                ..header
                            },
                            chunks: Vec::new(),
                        });
                    } else if controls > 0 || w != header.width || h != header.height {
                        return Err("Invalid APNG default frame".into());
                    }
                    controls += 1;
                }
                b"fdAT" => {
                    if streams.len() < 2 || data.len() < 4 {
                        return Err("APNG frame data without frame control".into());
                    }
                    sequence(data, &mut next)?;
                    streams.last_mut().unwrap().chunks.push(i);
                }
                b"tRNS" => {
                    let limit = (1u32 << header.depth) - 1;
                    let samples_fit = || {
                        data.as_chunks::<2>()
                            .0
                            .iter()
                            .all(|p| u32::from(u16::from_be_bytes([p[0], p[1]])) <= limit)
                    };
                    let valid = match header.color {
                        0 => data.len() == 2 && samples_fit(),
                        2 => data.len() == 6 && samples_fit(),
                        3 => palette.is_some_and(|n| !data.is_empty() && data.len() <= n),
                        _ => false,
                    };
                    if seen_idat || !valid {
                        return Err("Invalid PNG transparency table".into());
                    }
                }
                b"bKGD" => {
                    let limit = (1u32 << header.depth) - 1;
                    let samples_fit = || {
                        data.as_chunks::<2>()
                            .0
                            .iter()
                            .all(|p| u32::from(u16::from_be_bytes([p[0], p[1]])) <= limit)
                    };
                    let valid = match header.color {
                        0 | 4 => data.len() == 2 && samples_fit(),
                        2 | 6 => data.len() == 6 && samples_fit(),
                        3 => data.len() == 1 && palette.is_some_and(|n| usize::from(data[0]) < n),
                        _ => false,
                    };
                    if seen_idat || !valid {
                        return Err("Invalid PNG background-color reference".into());
                    }
                }
                b"hIST" => {
                    if seen_idat || !palette.is_some_and(|n| data.len() == 2 * n) {
                        return Err("Invalid PNG palette histogram".into());
                    }
                }
                b"sBIT" => {
                    let (expected, depth) = if header.color == 3 {
                        (3, 8)
                    } else {
                        (header.channels(), header.depth)
                    };
                    if seen_idat
                        || palette.is_some()
                        || data.len() != expected
                        || data.iter().any(|&n| n == 0 || n > depth)
                    {
                        return Err("Invalid PNG significant-bit information".into());
                    }
                }
                b"IEND" => {}
                // These standardized ancillary chunks are preserved verbatim while the pixel layout stays the same.
                b"cHRM" | b"cICP" | b"cLLI" | b"eXIf" | b"gAMA" | b"iCCP" | b"iTXt" | b"mDCV"
                | b"pHYs" | b"sPLT" | b"sRGB" | b"tEXt" | b"tIME" | b"zTXt" | b"sTER" => {}
                // A lowercase last letter marks an unknown ancillary chunk safe to copy after critical data changes.
                k if k[0].is_ascii_lowercase() => {
                    // Unknown unsafe-to-copy records, including signature manifests,
                    // can refer to the precise encoded image. Preserve the original file.
                    preserve_entire_file |= k[3].is_ascii_uppercase();
                }
                _ => {
                    return Err(format!(
                        "Unsupported critical chunk {}",
                        String::from_utf8_lossy(&c.kind)
                    ));
                }
            }
        }
        if streams.iter().any(|s| s.chunks.is_empty()) || control.is_some_and(|n| n != controls) {
            return Err("Incomplete PNG or APNG stream".into());
        }
        let mut total = 0;
        for s in &streams {
            total += s.header.passes()?.1;
            if total > MAX_DECODED {
                return Err("PNG exceeds the 256 MiB total decoded limit".into());
            }
        }
        Ok(Self {
            header,
            chunks,
            streams,
            palette,
            preserve_entire_file,
        })
    }

    pub fn compressed(&self, s: &Stream) -> Vec<u8> {
        let mut compressed = Vec::new();
        for &i in &s.chunks {
            let chunk = &self.chunks[i];
            let skip = usize::from(&chunk.kind == b"fdAT") * 4;
            compressed.extend_from_slice(&chunk.data[skip..]);
        }
        compressed
    }

    // Blend/dispose/offset do not affect a frame's independent zlib stream.
    // Reuse only byte-identical streams with identical decoding geometry.
    pub fn repeat_frame(
        &self,
        index: usize,
        original: &[u8],
        recent: &mut std::collections::VecDeque<(u32, usize)>,
    ) -> Option<usize> {
        if self.streams.len() == 1 {
            return None;
        }
        let digest = adler2::adler32_slice(original);
        let header = self.streams[index].header;
        if let Some(prior) = recent.iter().find_map(|&(hash, previous)| {
            (hash == digest
                && self.streams[previous].header == header
                && self.compressed(&self.streams[previous]) == original)
                .then_some(previous)
        }) {
            return Some(prior);
        }
        if recent.len() == 16 {
            recent.pop_front();
        }
        recent.push_back((digest, index));
        None
    }
    pub fn rebuild(&self, replacements: &[Vec<u8>], capacity: usize) -> Vec<u8> {
        // Keep the original frame numbering and the placement of interspersed ancillary chunks.
        let mut replace: Vec<Option<(usize, Range<usize>)>> =
            (0..self.chunks.len()).map(|_| None).collect();
        for (sidx, (stream, bytes)) in self.streams.iter().zip(replacements).enumerate() {
            let count = stream.chunks.len();
            for (j, &i) in stream.chunks.iter().enumerate() {
                let range = if sidx == 0 {
                    if j == 0 { 0..bytes.len() } else { 0..0 }
                } else {
                    let base = bytes.len() / count;
                    let rem = bytes.len() % count;
                    (base * j + rem.min(j))..(base * (j + 1) + rem.min(j + 1))
                };
                replace[i] = Some((sidx, range));
            }
        }
        let mut output = Vec::with_capacity(capacity);
        output.extend_from_slice(MAGIC);
        for (i, c) in self.chunks.iter().enumerate() {
            if let Some((n, r)) = &replace[i] {
                if *n == 0 && r.is_empty() {
                    continue;
                }
                let prefix = if *n == 0 { &[][..] } else { &c.data[..4] };
                write_chunk(&mut output, &c.kind, prefix, &replacements[*n][r.clone()]);
            } else {
                output.extend_from_slice(c.full);
            }
        }
        output
    }
}

pub(crate) fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], prefix: &[u8], data: &[u8]) {
    out.extend_from_slice(&((prefix.len() + data.len()) as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(prefix);
    out.extend_from_slice(data);
    let checksum = crc32(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}
