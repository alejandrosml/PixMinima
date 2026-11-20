use super::MAX_DECODED;
use std::borrow::Cow;

fn space_byte(c: u8) -> bool {
    c.is_ascii_whitespace() || c == 0x0b
}
pub(super) fn detect(b: &[u8]) -> Option<&'static str> {
    if b.len() < 3 || b[0] != b'P' || !space_byte(b[2]) {
        return None;
    }
    match b[1] {
        b'1' | b'4' => Some("PBM"),
        b'2' | b'5' => Some("PGM"),
        b'3' | b'6' => Some("PPM"),
        _ => None,
    }
}
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
    comments: usize,
}
impl Cursor<'_> {
    fn space(&mut self) {
        while let Some(&c) = self.data.get(self.at) {
            if space_byte(c) {
                self.at += 1;
                continue;
            }
            if c == b'#' {
                self.comments += 1;
                while self.at < self.data.len() && !matches!(self.data[self.at], b'\n' | b'\r') {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }
    fn number(&mut self) -> Result<usize, String> {
        self.space();
        let start = self.at;
        let mut n = 0usize;
        while let Some(&c) = self.data.get(self.at).filter(|b| b.is_ascii_digit()) {
            n = n
                .checked_mul(10)
                .and_then(|v| v.checked_add(usize::from(c - b'0')))
                .ok_or("Netpbm integer overflow")?;
            self.at += 1;
        }
        if self.at == start {
            return Err("Missing Netpbm numeric value".into());
        }
        Ok(n)
    }
}

struct Frame<'a> {
    plain: bool,
    kind: u8,
    width: usize,
    height: usize,
    max: usize,
    end_header: usize,
    end: usize,
    comments_in_raster: bool,
    raster: Cow<'a, [u8]>,
}
fn frame(b: &[u8]) -> Result<Frame<'_>, String> {
    detect(b).ok_or("Invalid Netpbm identifier")?;
    let plain = b[1] < b'4';
    let kind = (b[1] - b'1') % 3;
    let mut c = Cursor {
        data: b,
        at: 2,
        comments: 0,
    };
    let w = c.number()?;
    let h = c.number()?;
    if w == 0 || h == 0 {
        return Err("Zero Netpbm dimension".into());
    }
    let max = if kind == 0 { 1 } else { c.number()? };
    if max == 0 || max > 65535 {
        return Err("Invalid Netpbm maximum sample".into());
    }
    let samples = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(if kind == 2 { 3 } else { 1 }))
        .ok_or("Netpbm dimensions overflow")?;
    let len = if kind == 0 {
        w.div_ceil(8).checked_mul(h)
    } else {
        samples.checked_mul(if max >= 256 { 2 } else { 1 })
    }
    .filter(|&n| n <= MAX_DECODED)
    .ok_or("Netpbm exceeds the decoded size limit")?;
    let end_header = c.at;
    let comments_before = c.comments;
    let raster = if plain {
        let mut raw = Vec::with_capacity(len);
        if kind == 0 {
            let stride = w.div_ceil(8);
            raw.resize(len, 0);
            for y in 0..h {
                for x in 0..w {
                    c.space();
                    let bit = *b.get(c.at).ok_or("Truncated plain PBM raster")?;
                    if !matches!(bit, b'0' | b'1') {
                        return Err("Invalid plain PBM bit".into());
                    }
                    raw[y * stride + x / 8] |= (bit - b'0') << (7 - x % 8);
                    c.at += 1;
                }
            }
        } else {
            for _ in 0..samples {
                let value = c.number()?;
                if value > max {
                    return Err("Netpbm sample exceeds its declared maximum".into());
                }
                if max >= 256 {
                    raw.extend((value as u16).to_be_bytes());
                } else {
                    raw.push(value as u8);
                }
            }
        }
        c.space();
        Cow::Owned(raw)
    } else {
        if !b.get(c.at).copied().is_some_and(space_byte) {
            return Err("Missing binary Netpbm raster separator".into());
        }
        c.at += 1;
        let data = b
            .get(c.at..c.at.checked_add(len).ok_or("Netpbm raster overflow")?)
            .ok_or("Truncated binary Netpbm raster")?;
        if kind != 0 && max < 255 && data.iter().any(|&x| usize::from(x) > max) {
            return Err("Binary Netpbm sample out of range".into());
        }
        if kind != 0
            && max > 255
            && max < 65535
            && data
                .as_chunks::<2>()
                .0
                .iter()
                .any(|&p| usize::from(u16::from_be_bytes(p)) > max)
        {
            return Err("Binary Netpbm 16-bit sample out of range".into());
        }
        c.at += len;
        Cow::Borrowed(data)
    };
    Ok(Frame {
        plain,
        kind,
        width: w,
        height: h,
        max,
        end_header,
        end: c.at,
        comments_in_raster: comments_before != c.comments,
        raster,
    })
}

pub(super) fn optimize(input: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let first = frame(input)?;
    if first.end < input.len() {
        // Raw Netpbm permits multiple concatenated images. Preserve mixed image
        // sequences and validate each raster without rewriting their boundaries.
        let mut tail = &input[first.end..];
        while !tail.is_empty() {
            if tail.iter().copied().all(space_byte) {
                break;
            }
            if detect(tail).is_none() {
                return Err("Extra data after Netpbm raster".into());
            }
            let next = frame(tail)?;
            tail = &tail[next.end..];
        }
        return Ok(None);
    }
    if !first.plain || first.comments_in_raster {
        return Ok(None);
    }
    let mut result = input[..first.end_header].to_vec();
    result[1] += 3;
    result.push(b'\n');
    result.extend_from_slice(&first.raster);
    if result.len() >= input.len() {
        return Ok(None);
    }
    let reread = frame(&result)?;
    let same = reread.kind == first.kind
        && reread.width == first.width
        && reread.height == first.height
        && reread.max == first.max
        && reread.end == result.len()
        && reread.raster == first.raster;
    Ok(same.then_some(result))
}
