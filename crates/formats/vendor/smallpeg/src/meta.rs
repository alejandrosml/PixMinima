/// The EXIF Orientation tag (0x0112) from an APP1 payload
/// (`Exif\0\0` + TIFF stream), if present and in 1..=8.
pub(crate) fn exif_orientation(app1: &[u8]) -> Option<u8> {
    exif_orientation_at(app1).map(|(o, _, _)| o)
}

/// [`exif_orientation`] with where its value sits: the offset of the
/// two-byte SHORT in `app1`, and whether the TIFF stream is little-endian.
pub(crate) fn exif_orientation_at(app1: &[u8]) -> Option<(u8, usize, bool)> {
    let t = app1.strip_prefix(b"Exif\0\0")?;
    let le = match t.get(..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let u16_at = |p: usize| -> Option<u16> {
        let b = t.get(p..p.checked_add(2)?)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |p: usize| -> Option<u32> {
        let b = t.get(p..p.checked_add(4)?)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    // IFD0: entry count, then 12-byte entries (tag, type, count, value).
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    for i in 0..n {
        let e = ifd.checked_add(2 + 12 * i)?;
        if u16_at(e)? == 0x0112 {
            // SHORT, count 1: the value sits in the first two value bytes.
            if u16_at(e + 2)? != 3 || u32_at(e + 4)? != 1 {
                return None;
            }
            let v = u16_at(e + 8)?;
            return (1..=8).contains(&v).then_some((v as u8, 6 + e + 8, le));
        }
    }
    None
}
