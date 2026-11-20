//! Compact static HEIF/AVIF containers with fully resolved local item references.
//! Media and property bytes are copied verbatim. Movies, external references,
//! private properties and provenance-bearing documents keep their input layout.
use std::{collections::BTreeSet, ops::Range};

use super::bmff::{Atom, Kind, atom, boxes};
fn integer(data: &[u8], cursor: &mut usize, bytes: usize) -> Option<usize> {
    let end = (*cursor).checked_add(bytes)?;
    let n = data.get(*cursor..end)?.iter().try_fold(0usize, |s, b| {
        s.checked_mul(256)?.checked_add(usize::from(*b))
    })?;
    *cursor = end;
    Some(n)
}

pub(super) fn detect(data: &[u8]) -> Option<&'static str> {
    if data.get(4..8)? != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    let ftyp = data.get(8..size)?;
    if ftyp.len() < 8 || ftyp.len() % 4 != 0 {
        return None;
    }
    let has = |allowed: &[&Kind]| {
        std::iter::once(&ftyp[..4])
            .chain(ftyp[8..].as_chunks::<4>().0.iter().map(<[u8; 4]>::as_slice))
            .any(|brand| allowed.iter().any(|&a| a == brand))
    };
    if has(&[b"avif", b"avis"]) {
        Some("AVIF")
    } else if has(&[b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis"]) {
        Some("HEIC")
    } else if has(&[b"mif1", b"msf1"]) {
        Some("HEIF")
    } else {
        None
    }
}

fn unique<'a>(boxes: &'a [Atom], kind: &Kind) -> Option<&'a Atom> {
    let mut found = boxes.iter().filter(|b| &b.kind == kind);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn local_refs(data: &[u8], b: &Atom) -> Option<()> {
    let entries = boxes(data, b.payload.clone())?;
    let dref = unique(&entries, b"dref")?;
    if entries.len() != 1
        || data.get(dref.payload.start..dref.payload.start + 8)? != [0, 0, 0, 0, 0, 0, 0, 1]
    {
        return None;
    }
    let refs = boxes(data, dref.payload.start + 8..dref.payload.end)?;
    let url = unique(&refs, b"url ")?;
    (refs.len() == 1 && data[url.payload.clone()] == [0, 0, 0, 1]).then_some(())
}

fn properties(data: &[u8], b: &Atom) -> Option<()> {
    let fields = boxes(data, b.payload.clone())?;
    let ipco = unique(&fields, b"ipco")?;
    if !fields.iter().any(|p| p.kind == *b"ipma")
        || !fields.iter().all(|p| matches!(&p.kind, b"ipco" | b"ipma"))
    {
        return None;
    }
    boxes(data, ipco.payload.clone())?
        .iter()
        .all(|p| {
            matches!(
                &p.kind,
                b"ispe"
                    | b"pixi"
                    | b"av1C"
                    | b"hvcC"
                    | b"colr"
                    | b"clap"
                    | b"irot"
                    | b"imir"
                    | b"pasp"
                    | b"clli"
                    | b"mdcv"
                    | b"auxC"
                    | b"lsel"
                    | b"a1lx"
                    | b"a1op"
            )
        })
        .then_some(())
}

#[derive(Debug, PartialEq)]
struct Item {
    id: usize,
    extents: Vec<Range<usize>>,
}

fn locations(data: &[u8], media: &Range<usize>) -> Option<Vec<Item>> {
    let mut p = 0;
    let version = integer(data, &mut p, 1)?;
    if version > 2 || integer(data, &mut p, 3)? != 0 {
        return None;
    }
    let sizes = integer(data, &mut p, 1)?;
    let last = integer(data, &mut p, 1)?;
    let (offset, length, base, index) = (sizes >> 4, sizes & 15, last >> 4, last & 15);
    if ![offset, length, base]
        .iter()
        .all(|v| matches!(v, 0 | 4 | 8))
        || index != 0
    {
        return None;
    }
    let count = integer(data, &mut p, if version == 2 { 4 } else { 2 })?;
    if count == 0 || count > 16384 {
        return None;
    }
    let mut items = Vec::with_capacity(count);
    let mut ids = BTreeSet::new();
    let mut total_extents = 0usize;
    for _ in 0..count {
        let id = integer(data, &mut p, if version == 2 { 4 } else { 2 })?;
        if !ids.insert(id)
            || (version != 0 && integer(data, &mut p, 2)? != 0)
            || integer(data, &mut p, 2)? != 0
        {
            return None;
        }
        let origin = integer(data, &mut p, base)?;
        let n = integer(data, &mut p, 2)?;
        total_extents += n;
        if n == 0 || total_extents > 65535 {
            return None;
        }
        let mut extents = Vec::with_capacity(n);
        for _ in 0..n {
            let start = origin.checked_add(integer(data, &mut p, offset)?)?;
            let size = integer(data, &mut p, length)?;
            let end = start.checked_add(size)?;
            if size == 0 || start < media.start || end > media.end {
                return None;
            }
            extents.push(start - media.start..end - media.start);
        }
        items.push(Item { id, extents });
    }
    (p == data.len()).then_some(items)
}

fn info(data: &[u8], b: &Atom, media: &Range<usize>, items: &[Item]) -> Option<()> {
    let bytes = &data[b.payload.clone()];
    let mut p = 0;
    let v = integer(bytes, &mut p, 1)?;
    if v > 1 || integer(bytes, &mut p, 3)? != 0 {
        return None;
    }
    let count = integer(bytes, &mut p, if v == 0 { 2 } else { 4 })?;
    let entries = boxes(data, b.payload.start + p..b.payload.end)?;
    if entries.len() != count || count != items.len() {
        return None;
    }
    let by_id: std::collections::BTreeMap<_, _> = items.iter().map(|i| (i.id, i)).collect();
    let mut seen = BTreeSet::new();
    for entry in entries {
        if entry.kind != *b"infe" {
            return None;
        }
        let d = &data[entry.payload];
        let mut p = 0;
        let version = integer(d, &mut p, 1)?;
        if !matches!(version, 2 | 3) || integer(d, &mut p, 3)? > 1 {
            return None;
        }
        let id = integer(d, &mut p, if version == 2 { 2 } else { 4 })?;
        if !seen.insert(id) || integer(d, &mut p, 2)? != 0 {
            return None;
        }
        let item = by_id.get(&id)?;
        match d.get(p..p + 4)? {
            b"av01" | b"hvc1" | b"grid" | b"iden" | b"iovl" => {}
            b"Exif" => {
                // Opaque maker notes can contain proprietary absolute file offsets.
                if item.extents.len() != 1 {
                    return None;
                }
                let extent = &item.extents[0];
                let exif = &data[media.start + extent.start..media.start + extent.end];
                if exif
                    .windows(2)
                    .any(|p| p == [0x92, 0x7c] || p == [0x7c, 0x92])
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(())
}

fn encode_locations(items: &[Item], start: usize) -> Option<Vec<u8>> {
    let wide_ids = items.iter().any(|i| i.id > usize::from(u16::MAX));
    let mut out = vec![if wide_ids { 2 } else { 0 }, 0, 0, 0, 0x44, 0];
    if wide_ids {
        out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    } else {
        out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    }
    for i in items {
        if wide_ids {
            out.extend_from_slice(&(i.id as u32).to_be_bytes());
            out.extend_from_slice(&[0, 0]);
        } else {
            out.extend_from_slice(&(i.id as u16).to_be_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(i.extents.len() as u16).to_be_bytes());
        for e in &i.extents {
            out.extend_from_slice(
                &(u32::try_from(start.checked_add(e.start)?).ok()?).to_be_bytes(),
            );
            out.extend_from_slice(&(u32::try_from(e.len()).ok()?).to_be_bytes());
        }
    }
    Some(atom(*b"iloc", &out))
}

fn compact(data: &[u8]) -> Option<Vec<u8>> {
    let top = boxes(data, 0..data.len())?;
    if top.first()?.kind != *b"ftyp"
        || !top.iter().all(|b| match &b.kind {
            b"ftyp" | b"meta" | b"mdat" => true,
            b"free" | b"skip" => data[b.payload.clone()].iter().all(|b| *b == 0),
            _ => false,
        })
    {
        return None;
    }
    let ftyp = unique(&top, b"ftyp")?;
    let meta = unique(&top, b"meta")?;
    let mdat = unique(&top, b"mdat")?;
    if meta.whole.start >= mdat.whole.start
        || data.get(meta.payload.start..meta.payload.start + 4)? != [0; 4]
    {
        return None;
    }
    let fields = boxes(data, meta.payload.start + 4..meta.payload.end)?;
    let iloc = unique(&fields, b"iloc")?;
    let items = locations(&data[iloc.payload.clone()], &mdat.payload)?;
    let pict = unique(&fields, b"hdlr")?;
    if data.get(pict.payload.start + 8..pict.payload.start + 12)? != b"pict" {
        return None;
    }
    unique(&fields, b"pitm")?;
    info(data, unique(&fields, b"iinf")?, &mdat.payload, &items)?;
    properties(data, unique(&fields, b"iprp")?)?;
    for b in &fields {
        match &b.kind {
            b"hdlr" | b"pitm" | b"iloc" | b"iinf" | b"iprp" => {}
            b"dinf" => local_refs(data, b)?,
            b"iref" => {
                let version = data.get(b.payload.start..b.payload.start + 4)?;
                if version != [0; 4] && version != [1, 0, 0, 0] {
                    return None;
                }
                if !boxes(data, b.payload.start + 4..b.payload.end)?
                    .iter()
                    .all(|r| matches!(&r.kind, b"dimg" | b"thmb" | b"auxl" | b"cdsc" | b"prem"))
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let template = encode_locations(&items, 0)?;
    let body_len = (meta.payload.len() - iloc.whole.len()).checked_add(template.len())?;
    let start = ftyp.whole.len().checked_add(8 + body_len + 8)?;
    if start.checked_add(mdat.payload.len())? >= data.len() {
        return None;
    }
    let packed = encode_locations(&items, start)?;
    let mut body = vec![0; 4];
    for field in fields {
        if field.kind == *b"iloc" {
            body.extend_from_slice(&packed);
        } else {
            body.extend_from_slice(&data[field.whole]);
        }
    }
    let mut out = Vec::with_capacity(start + mdat.payload.len());
    out.extend_from_slice(&data[ftyp.whole.clone()]);
    out.extend_from_slice(&atom(*b"meta", &body));
    out.extend_from_slice(&((8 + mdat.payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    if out.len() != start {
        return None;
    }
    out.extend_from_slice(&data[mdat.payload.clone()]);
    // Independently parse the emitted item locations and verify every media byte.
    let new_top = boxes(&out, 0..out.len())?;
    let new_meta = unique(&new_top, b"meta")?;
    let new_media = &unique(&new_top, b"mdat")?.payload;
    let new_fields = boxes(&out, new_meta.payload.start + 4..new_meta.payload.end)?;
    let new_locations = &unique(&new_fields, b"iloc")?.payload;
    if locations(&out[new_locations.clone()], new_media)? != items
        || out[new_media.clone()] != data[mdat.payload.clone()]
    {
        return None;
    }
    Some(out)
}

pub(super) fn optimize(data: &[u8]) -> Vec<u8> {
    compact(data).unwrap_or_else(|| data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &[u8] = include_bytes!("../tests/fixtures/flat.avif");

    #[test]
    fn zero_padding_is_removed_only_from_known_self_contained_still_images() {
        assert_eq!(detect(SOURCE), Some("AVIF"));
        let expected = optimize(SOURCE);
        let mut padded = SOURCE.to_vec();
        padded.extend_from_slice(&atom(*b"free", &[0; 1024]));
        assert_eq!(optimize(&padded), expected);
        for (kind, bytes) in [(b"uuid", [0; 16]), (b"free", [1; 16]), (b"moov", [0; 16])] {
            let mut opaque = padded.clone();
            opaque.extend_from_slice(&atom(*kind, &bytes));
            assert_eq!(optimize(&opaque), opaque);
        }
        let mut external = padded.clone();
        let loc = external.windows(4).position(|w| w == b"iloc").unwrap() + 4;
        external[loc + 11] = 1; // data_reference_index on the first version-0 item
        assert_eq!(optimize(&external), external);
    }

    #[test]
    fn animated_or_unresolved_obscure_metadata_keeps_the_layout() {
        let mut doc = SOURCE.to_vec();
        doc.extend_from_slice(&atom(*b"free", &[0; 32]));
        let prop = doc.windows(4).position(|w| w == b"colr").unwrap();
        doc[prop..prop + 4].copy_from_slice(b"uuid");
        assert_eq!(optimize(&doc), doc);
        let mut unknown = SOURCE.to_vec();
        let fmt = unknown.windows(4).position(|w| w == b"av01").unwrap();
        unknown[fmt..fmt + 4].copy_from_slice(b"jumb");
        unknown.extend_from_slice(&atom(*b"free", &[0; 32]));
        assert_eq!(optimize(&unknown), unknown);
    }
}
