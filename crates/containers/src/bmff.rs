use std::ops::Range;
pub(super) type Kind = [u8; 4];
#[derive(Clone)]
pub(super) struct Atom {
    pub(super) kind: Kind,
    pub(super) whole: Range<usize>,
    pub(super) payload: Range<usize>,
}

pub(super) fn boxes(data: &[u8], range: Range<usize>) -> Option<Vec<Atom>> {
    let mut result = Vec::new();
    let mut p = range.start;
    while p != range.end {
        let header = data.get(p..range.end)?.get(..8)?;
        let n = u32::from_be_bytes(header[..4].try_into().ok()?);
        let kind = header[4..8].try_into().ok()?;
        let (size, h) = match n {
            0 => (range.end - p, 8),
            1 => (
                usize::try_from(u64::from_be_bytes(
                    data.get(p + 8..p + 16)?.try_into().ok()?,
                ))
                .ok()?,
                16,
            ),
            _ => (n as usize, 8),
        };
        let end = p.checked_add(size)?;
        if size < h || end > range.end || result.len() >= 32768 {
            return None;
        }
        result.push(Atom {
            kind,
            whole: p..end,
            payload: p + h..end,
        });
        p = end;
    }
    Some(result)
}

pub(super) fn atom(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(8 + payload.len());
    result.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    result.extend_from_slice(&kind);
    result.extend_from_slice(payload);
    result
}
