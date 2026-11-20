use super::{MAX_ANIMATION_PIXELS, MAX_PIXELS, le32, take};
use image_webp::{ColorType, EncoderParams, WebPDecoder, WebPEncoder};
use std::io::Cursor;

type Shape = (u32, u32);

fn write_chunk(dst: &mut Vec<u8>, kind: &[u8], payload: &[u8]) {
    dst.extend_from_slice(kind);
    dst.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    dst.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        dst.push(0);
    }
}

fn single_frame(payload: &[u8]) -> Vec<u8> {
    let mut riff = b"RIFF\0\0\0\0WEBP".to_vec();
    write_chunk(&mut riff, b"VP8L", payload);
    let size = (riff.len() - 8) as u32;
    riff[4..8].copy_from_slice(&size.to_le_bytes());
    riff
}

fn shape(payload: &[u8]) -> Result<Shape, String> {
    if payload.len() < 5 || payload[0] != 0x2f || payload[4] & 0xe0 != 0 {
        return Err("Invalid VP8L header".into());
    }
    let bits = le32(&payload[1..]);
    Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
}

fn decode(payload: &[u8], dims: Shape) -> Result<Vec<u8>, String> {
    let mut riff = single_frame(payload);
    // The VP8L alpha flag is only a hint. Request all four channels even if its
    // producer set it incorrectly, including RGB underneath fully transparent pixels.
    riff[24] |= 0x10;
    let mut d = WebPDecoder::new(Cursor::new(&riff)).map_err(|e| e.to_string())?;
    if d.dimensions() != dims || !d.has_alpha() {
        return Err("Inconsistent VP8L dimensions".into());
    }
    let mut rgba = vec![0; dims.0 as usize * dims.1 as usize * 4];
    d.read_image(&mut rgba).map_err(|e| e.to_string())?;
    Ok(rgba)
}

fn vp8l_payload(encoded: &[u8]) -> Result<&[u8], String> {
    if encoded.get(12..16) != Some(b"VP8L") {
        return Err("Unexpected encoded WebP chunk".into());
    }
    let size = encoded.get(16..20).ok_or("Incomplete VP8L chunk header")?;
    encoded
        .get(20..20 + le32(size) as usize)
        .ok_or_else(|| "Incomplete VP8L output".into())
}

// Quantize visible RGB on an absolute lattice. Both endpoints are fixed and
// alpha, including RGB underneath zero alpha, is preserved exactly.
fn quantize_visible(rgba: &mut [u8]) -> bool {
    let mut error = [0u64; 3];
    let mut changes = false;
    let count = rgba.len() / 4;
    for px in rgba.as_chunks_mut::<4>().0 {
        if px[3] == 0 {
            continue;
        }
        for c in 0..3 {
            let old = px[c];
            let mapped = ((u16::from(old) + 4) / 8 * 8).min(255) as u8;
            let delta = u64::from(old.abs_diff(mapped));
            error[c] += delta * delta;
            changes |= old != mapped;
            px[c] = mapped;
        }
    }
    changes && error.iter().all(|&m| m * 4 <= 25 * count as u64)
}

fn compress_frame(
    payload: &[u8],
    dims: Shape,
    thorough: bool,
    lossy: bool,
) -> Result<Option<Vec<u8>>, String> {
    let mut rgba = decode(payload, dims)?;
    // The VP8L decoder stops once it has reconstructed the rectangle. If even
    // a complete byte can be removed, its possible private use is unknown.
    if payload.len() > 5 && decode(&payload[..payload.len() - 1], dims).is_ok_and(|p| p == rgba) {
        return Ok(None);
    }
    if lossy && !quantize_visible(&mut rgba) {
        return Ok(Some(payload.to_vec()));
    }
    let mut best = payload.to_vec();
    let mut mono = true;
    let mut alpha = false;
    for p in rgba.as_chunks::<4>().0 {
        mono &= p[0] == p[1] && p[1] == p[2];
        alpha |= p[3] != 255;
    }
    if alpha && payload[4] & 0x10 == 0 {
        return Ok(Some(best));
    }
    let (color, channels) = match (mono, alpha) {
        (true, false) => (ColorType::L8, vec![0]),
        (true, true) => (ColorType::La8, vec![0, 3]),
        (false, false) => (ColorType::Rgb8, vec![0, 1, 2]),
        (false, true) => (ColorType::Rgba8, vec![0, 1, 2, 3]),
    };
    let narrow: Vec<u8>;
    let samples = if channels.len() == 4 {
        rgba.as_slice()
    } else {
        narrow = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| channels.iter().map(move |&c| px[c]))
            .collect();
        &narrow
    };
    let mut consider = |frame: &[u8]| -> Result<(), String> {
        if frame.len() >= best.len() {
            return Ok(());
        }
        let mut candidate = frame.to_vec();
        if shape(&candidate)? != dims {
            return Err("Encoded VP8L dimensions changed".into());
        }
        candidate[4] = (candidate[4] & !0x10) | (payload[4] & 0x10);
        if decode(&candidate, dims)? == rgba {
            best = candidate;
        }
        Ok(())
    };
    let predicted = if thorough {
        &[true, false][..]
    } else {
        &[true][..]
    };
    for &predict in predicted {
        let mut encoded = Vec::new();
        let mut e = WebPEncoder::new(&mut encoded);
        let mut params = EncoderParams::default();
        params.use_predictor_transform = predict;
        e.set_params(params);
        e.encode(samples, dims.0, dims.1, color)
            .map_err(|e| e.to_string())?;
        consider(vp8l_payload(&encoded)?)?;
    }
    // A complementary encoder searches LZ77 backward references and color-cache
    // hits. The runs-only encoder is still a candidate for high-entropy images.
    let mut encoded = Vec::new();
    let encoder = rusty_webp::WebPEncoder::new(&mut encoded);
    encoder
        .encode(&rgba, dims.0, dims.1, rusty_webp::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    consider(vp8l_payload(&encoded)?)?;
    if thorough && rgba.len() / 4 <= 1_048_576 {
        let mut colors = std::collections::HashSet::with_capacity(257);
        for p in rgba.as_chunks::<4>().0 {
            colors.insert(*p);
            if colors.len() > 256 {
                break;
            }
        }
        // Repetitive palette images trigger much longer match searches at the
        // highest effort. The palette transform already handles them at level 5.
        let level = if colors.len() <= 256 { 5 } else { 9 };
        let config = webp_rust::LosslessEncodingConfig { z_level: level };
        if let Ok(frame) = webp_rust::encoder::encode_lossless_rgba_to_vp8l_with_config(
            dims.0 as usize,
            dims.1 as usize,
            &rgba,
            &config,
        ) {
            consider(&frame)?;
        }
    }
    Ok(Some(best))
}

fn le24(s: &[u8]) -> u32 {
    u32::from_le_bytes([s[0], s[1], s[2], 0])
}

struct Limits {
    pixels: usize,
    frames: usize,
    chunks: usize,
}

// Rebuild only VP8L payloads and their mandatory surrounding lengths. All
// animation controls, offsets, blending, lossy frames and metadata stay intact.
fn chunks(
    src: &[u8],
    frame: Option<Shape>,
    limits: &mut Limits,
    thorough: bool,
    lossy: bool,
) -> Result<Option<Vec<u8>>, String> {
    let mut p = 0;
    let mut dst = Vec::with_capacity(src.len());
    let mut has_image = false;
    while p < src.len() {
        let start = p;
        let header = take(src, &mut p, 8)?;
        let kind = &header[..4];
        let len = le32(&header[4..]) as usize;
        let payload = take(src, &mut p, len)?;
        let pad = take(src, &mut p, len % 2)?;
        if pad.first().is_some_and(|&v| v != 0) {
            return Ok(None);
        }
        limits.chunks += 1;
        if limits.chunks > 65_536 {
            return Err("Too many WebP chunks".into());
        }
        match kind {
            b"VP8L" => {
                if has_image {
                    return Ok(None);
                }
                let dims = shape(payload)?;
                if frame.is_some_and(|f| f != dims) {
                    return Err("WebP animation rectangle mismatch".into());
                }
                let n = dims.0 as usize * dims.1 as usize;
                limits.pixels += n;
                limits.frames += 1;
                if n > MAX_PIXELS || limits.pixels > MAX_ANIMATION_PIXELS || limits.frames > 16384 {
                    return Err("WebP exceeds frame limits".into());
                }
                let Some(best) = compress_frame(payload, dims, thorough, lossy)? else {
                    return Ok(None);
                };
                write_chunk(&mut dst, kind, &best);
                has_image = true;
            }
            b"ANMF" if frame.is_none() => {
                if len < 16 {
                    return Err("Truncated WebP frame".into());
                }
                let dims = (le24(&payload[6..]) + 1, le24(&payload[9..]) + 1);
                let Some(optimized) = chunks(&payload[16..], Some(dims), limits, thorough, lossy)?
                else {
                    return Ok(None);
                };
                let mut data = payload[..16].to_vec();
                data.extend(optimized);
                write_chunk(&mut dst, kind, &data);
            }
            b"VP8 " => {
                if has_image {
                    return Ok(None);
                }
                dst.extend_from_slice(&src[start..p]);
                has_image = true;
            }
            b"ALPH" | b"VP8X" | b"ANIM" | b"ICCP" | b"EXIF" | b"XMP " => {
                dst.extend_from_slice(&src[start..p])
            }
            // Private chunks may contain signatures or absolute file offsets.
            _ => return Ok(None),
        }
    }
    if frame.is_some() && !has_image {
        return Err("Missing WebP frame bitstream".into());
    }
    Ok(Some(dst))
}

pub(crate) fn optimize(data: &[u8], thorough: bool) -> Result<Vec<u8>, String> {
    recompress(data, thorough, false)
}

pub(crate) fn optimize_lossy(data: &[u8], thorough: bool) -> Result<Option<Vec<u8>>, String> {
    let candidate = recompress(data, thorough, true)?;
    Ok((candidate.len() < data.len()).then_some(candidate))
}

fn recompress(data: &[u8], thorough: bool, lossy: bool) -> Result<Vec<u8>, String> {
    if le32(&data[4..]) as usize != data.len() - 8 {
        return Ok(data.to_vec());
    }
    let mut limits = Limits {
        pixels: 0,
        frames: 0,
        chunks: 0,
    };
    let Some(body) = chunks(&data[12..], None, &mut limits, thorough, lossy)? else {
        return Ok(data.to_vec());
    };
    if body.len() >= data.len() - 12 {
        return Ok(data.to_vec());
    }
    WebPDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend(body);
    WebPDecoder::new(Cursor::new(&out)).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_grid_preserves_hidden_samples_alpha_and_metadata() {
        let mut samples = Vec::new();
        for y in 0u8..40 {
            for x in 0u8..40 {
                samples.extend([
                    x * 4 + 1,
                    y * 4 + 1,
                    (x ^ y) * 4 + 1,
                    if (x + y) % 9 == 0 {
                        0
                    } else {
                        x.saturating_mul(8)
                    },
                ]);
            }
        }
        let mut data = Vec::new();
        let mut encoder = WebPEncoder::new(&mut data);
        encoder.set_xmp_metadata(b"image properties".to_vec());
        encoder.encode(&samples, 40, 40, ColorType::Rgba8).unwrap();
        let next = optimize_lossy(&data, true).unwrap().unwrap();
        let mut decoded = WebPDecoder::new(Cursor::new(&next)).unwrap();
        assert_eq!(
            decoded.xmp_metadata().unwrap().unwrap(),
            b"image properties"
        );
        let mut actual = vec![0; samples.len()];
        decoded.read_image(&mut actual).unwrap();
        assert_ne!(samples, actual);
        for (a, b) in samples
            .as_chunks::<4>()
            .0
            .iter()
            .zip(actual.as_chunks::<4>().0)
        {
            assert_eq!(a[3], b[3]);
            if a[3] == 0 {
                assert_eq!(a, b);
            } else {
                assert!(
                    a[..3]
                        .iter()
                        .zip(&b[..3])
                        .all(|(&x, &y)| x.abs_diff(y) <= 4)
                );
            }
        }
        assert!(optimize_lossy(&next, true).unwrap().is_none());
    }
    #[test]
    fn metadata_and_invisible_rgb_survive_reencoding() {
        let mut px = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                px.extend([x, y, x ^ y, if y % 3 == 0 { 0 } else { 255 }]);
            }
        }
        let mut data = Vec::new();
        let mut e = WebPEncoder::new(&mut data);
        let mut p = EncoderParams::default();
        p.use_predictor_transform = false;
        e.set_params(p);
        e.set_xmp_metadata(b"keep exact bytes".to_vec());
        e.encode(&px, 64, 64, ColorType::Rgba8).unwrap();
        let optimized = optimize(&data, true).unwrap();
        assert!(optimized.len() < data.len());
        let mut d = WebPDecoder::new(Cursor::new(optimized)).unwrap();
        assert_eq!(d.xmp_metadata().unwrap().unwrap(), b"keep exact bytes");
        let mut actual = vec![0; px.len()];
        d.read_image(&mut actual).unwrap();
        assert_eq!(actual, px);
        let mut file = Vec::new();
        WebPEncoder::new(&mut file)
            .encode(&px, 64, 64, ColorType::Rgba8)
            .unwrap();
        let mut payload = vp8l_payload(&file).unwrap().to_vec();
        payload.push(71);
        let opaque = single_frame(&payload);
        assert_eq!(optimize(&opaque, true).unwrap(), opaque);
    }
    #[test]
    fn animated_webp_preserves_every_frame_duration_and_blend() {
        fn u24(n: u32) -> [u8; 3] {
            let b = n.to_le_bytes();
            [b[0], b[1], b[2]]
        }
        let mut chunks = Vec::new();
        let mut canvas = vec![0x12, 0, 0, 0];
        canvas.extend(u24(95));
        canvas.extend(u24(47));
        write_chunk(&mut chunks, b"VP8X", &canvas);
        write_chunk(&mut chunks, b"ANIM", &[5, 22, 33, 255, 3, 0]);
        for (n, x, delay, flags) in [(0u8, 0u32, 17u32, 2u8), (1, 16, 39, 1)] {
            let mut rgba = Vec::new();
            for y in 0..48u8 {
                for x in 0..64u8 {
                    rgba.extend([x + n, y, x ^ y, if x % 3 == 0 { 0 } else { 128 }]);
                }
            }
            let mut encoded = Vec::new();
            let mut encoder = WebPEncoder::new(&mut encoded);
            let mut params = EncoderParams::default();
            params.use_predictor_transform = false;
            encoder.set_params(params);
            encoder.encode(&rgba, 64, 48, ColorType::Rgba8).unwrap();
            let mut frame = Vec::new();
            frame.extend(u24(x));
            frame.extend(u24(0));
            frame.extend(u24(63));
            frame.extend(u24(47));
            frame.extend(u24(delay));
            frame.push(flags);
            frame.extend_from_slice(&encoded[12..]);
            write_chunk(&mut chunks, b"ANMF", &frame);
        }
        let mut original = b"RIFF".to_vec();
        original.extend(((4 + chunks.len()) as u32).to_le_bytes());
        original.extend(b"WEBP");
        original.extend(chunks);
        let output = optimize(&original, true).unwrap();
        assert!(output.len() < original.len());
        let mut old = WebPDecoder::new(Cursor::new(&original)).unwrap();
        let mut new = WebPDecoder::new(Cursor::new(&output)).unwrap();
        assert_eq!(old.num_frames(), 2);
        assert_eq!(new.num_frames(), 2);
        assert_eq!(new.loop_count(), old.loop_count());
        assert_eq!(new.background_color_hint(), old.background_color_hint());
        let mut a = vec![0; old.output_buffer_size().unwrap()];
        let mut b = vec![0; a.len()];
        for delay in [17, 39] {
            assert_eq!(old.read_frame(&mut a).unwrap(), delay);
            assert_eq!(new.read_frame(&mut b).unwrap(), delay);
            assert_eq!(a, b);
        }
    }
}
