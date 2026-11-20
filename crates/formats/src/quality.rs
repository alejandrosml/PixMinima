//! Bounds are expressed in unassociated, eight-bit decoded samples.

/// Every channel must have MSE <= 6.25, every 8×8 tile/channel MSE <= 36,
/// and every individual channel error <= 20.
pub(crate) fn close(original: &[u8], candidate: &[u8], width: usize, channels: usize) -> bool {
    if !matches!(channels, 1 | 3)
        || width == 0
        || original.len() != candidate.len()
        || original.is_empty()
        || !original.len().is_multiple_of(width * channels)
    {
        return false;
    }
    let height = original.len() / (width * channels);
    let mut whole = [0u64; 3];
    for y in (0..height).step_by(8) {
        let bottom = (y + 8).min(height);
        for x in (0..width).step_by(8) {
            let right = (x + 8).min(width);
            let mut tile = [0u64; 3];
            for row in y..bottom {
                let start = (row * width + x) * channels;
                let end = (row * width + right) * channels;
                for (index, (&a, &b)) in original[start..end]
                    .iter()
                    .zip(&candidate[start..end])
                    .enumerate()
                {
                    let delta = a.abs_diff(b);
                    if delta > 20 {
                        return false;
                    }
                    tile[index % channels] += u64::from(delta) * u64::from(delta);
                }
            }
            for c in 0..channels {
                if tile[c] > 36 * ((right - x) * (bottom - y)) as u64 {
                    return false;
                }
                whole[c] += tile[c];
            }
        }
    }
    whole[..channels]
        .iter()
        .all(|&sum| sum * 4 <= 25 * (width * height) as u64)
}
