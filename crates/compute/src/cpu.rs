//! Minimum sum of absolute signed residuals, using the five PNG predictors.
//! A branch-free interior lets the compiler vectorize across adjacent channels.
#[inline]
fn add(scores: &mut [u32; 5], value: u8, left: u8, top: u8, corner: u8) {
    let a = i16::from(left);
    let b = i16::from(top);
    let c = i16::from(corner);
    let dx = (b - c).abs();
    let dy = (a - c).abs();
    let dz = (a + b - 2 * c).abs();
    let paeth = if dx <= dy && dx <= dz {
        left
    } else if dy <= dz {
        top
    } else {
        corner
    };
    let mean = ((a + b) / 2) as u8;
    for (score, prediction) in scores.iter_mut().zip([0, left, top, mean, paeth]) {
        *score += u32::from((value.wrapping_sub(prediction) as i8).unsigned_abs());
    }
}

pub(super) fn rows(input: &[u8], width: usize) -> Vec<u8> {
    let stride = width * 4;
    let zeros = vec![0; stride];
    let mut prev = zeros.as_slice();
    let mut selected = Vec::with_capacity(input.len() / stride);
    for row in input.chunks_exact(stride) {
        let mut scores = [0; 5];
        for i in 0..4 {
            add(&mut scores, row[i], 0, prev[i], 0);
        }
        for i in 4..stride {
            add(&mut scores, row[i], row[i - 4], prev[i], prev[i - 4]);
        }
        let mut best = 0;
        for i in 1..5 {
            if scores[i] < scores[best] {
                best = i;
            }
        }
        selected.push(best as u8);
        prev = row;
    }
    selected
}
