use super::{crc32, inflate_exact, smaller_deflate, u16le, u32le};
use roxmltree::Document;

fn xml_document(xml: &str) -> Result<Document<'_>, String> {
    let opts = roxmltree::ParsingOptions {
        nodes_limit: 1_000_000,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opts)
        .map_err(|e| format!("Unrecognized or invalid SVG: {e}"))?;
    let root = doc.root_element().tag_name();
    if root.name() != "svg"
        || root
            .namespace()
            .is_some_and(|s| s != "http://www.w3.org/2000/svg")
    {
        return Err("XML document has no SVG root".into());
    }
    Ok(doc)
}

/// Tighten only XML start/end tag punctuation. Text, spacing inside values,
/// style sheets, scripts, comments, CDATA, XML declarations and PIs stay intact.
pub(super) fn svg(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() > 32 * 1024 * 1024 {
        return Err("SVG exceeds the 32 MiB parsing limit".into());
    }
    let xml = std::str::from_utf8(input).map_err(|_| "Only UTF-8 SVG is supported")?;
    let parsed = xml_document(xml)?;
    if parsed
        .descendants()
        .any(|node| node.tag_name().namespace() == Some("http://www.w3.org/2000/09/xmldsig#"))
    {
        return Ok(input.to_vec());
    }
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != b'<' {
            out.push(input[i]);
            i += 1;
            continue;
        }
        let tail = &input[i..];
        let terminator: &[u8] = if tail.starts_with(b"<!--") {
            b"-->"
        } else if tail.starts_with(b"<![CDATA[") {
            b"]]>"
        } else if tail.starts_with(b"<?") {
            b"?>"
        } else {
            b""
        };
        if !terminator.is_empty() {
            let end = tail
                .windows(terminator.len())
                .position(|v| v == terminator)
                .ok_or("Unterminated XML section")?
                + terminator.len();
            out.extend_from_slice(&tail[..end]);
            i += end;
            continue;
        }
        if tail.starts_with(b"<!") {
            return Ok(input.to_vec());
        }
        let mut quote = 0;
        while i < input.len() {
            let b = input[i];
            i += 1;
            if quote != 0 {
                out.push(b);
                if b == quote {
                    quote = 0;
                }
                continue;
            }
            if matches!(b, b'\'' | b'"') {
                quote = b;
            }
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                while i < input.len() && matches!(input[i], b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                if !matches!(input.get(i), Some(b'=' | b'/' | b'>')) && out.last() != Some(&b'=') {
                    out.push(b' ');
                }
            } else {
                out.push(b);
            }
            if b == b'>' {
                break;
            }
        }
    }
    if out.len() >= input.len() {
        return Ok(input.to_vec());
    }
    let other = xml_document(std::str::from_utf8(&out).map_err(|e| e.to_string())?)?;
    let same = parsed.descendants().count() == other.descendants().count()
        && parsed.descendants().zip(other.descendants()).all(|(a, b)| {
            a.node_type() == b.node_type()
                && a.tag_name() == b.tag_name()
                && a.text() == b.text()
                && a.pi() == b.pi()
                && a.attributes()
                    .map(|x| (x.namespace(), x.name(), x.value()))
                    .eq(b.attributes().map(|x| (x.namespace(), x.name(), x.value())))
        });
    if !same {
        return Err("SVG structure changed during validation".into());
    }
    Ok(out)
}

pub(super) fn gzip_svg(input: &[u8], thorough: bool) -> Result<(&'static str, Vec<u8>), String> {
    if input.len() < 18 || input[3] & 0xe0 != 0 {
        return Err("Invalid gzip header".into());
    }
    let flags = input[3];
    let mut start = 10;
    if flags & 4 != 0 {
        start += 2 + u16le(input, start)?;
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            start += input
                .get(start..)
                .and_then(|s| s.iter().position(|b| *b == 0))
                .ok_or("Missing gzip field terminator")?
                + 1;
        }
    }
    if flags & 2 != 0 {
        if u16le(input, start)? != (crc32(&input[..start]) & 0xffff) as usize {
            return Err("Incorrect gzip header CRC".into());
        }
        start += 2;
    }
    let end = input.len() - 8;
    let compressed = input.get(start..end).ok_or("Invalid gzip offsets")?;
    let source = inflate_exact(compressed, u32le(input, end + 4)?)?;
    if crc32(&source) != u32le(input, end)? as u32 {
        return Err("Invalid gzip payload checksum".into());
    }
    xml_document(std::str::from_utf8(&source).map_err(|_| "Only UTF-8 SVGZ is supported")?)?;
    let best = smaller_deflate(&source, compressed, thorough)?;
    let mut out = input[..start].to_vec();
    out.extend_from_slice(&best);
    out.extend_from_slice(&input[end..]);
    Ok(("SVGZ", out))
}
