use pixminima_compute::{ComputeMode, choose_rows};

fn reference(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .map(|y| {
            let mut score = [0u64; 5];
            for x in 0..w {
                for channel in 0..4 {
                    let i = (y * w + x) * 4 + channel;
                    let a = if x == 0 { 0 } else { data[i - 4] } as i32;
                    let b = if y == 0 { 0 } else { data[i - 4 * w] } as i32;
                    let c = if x == 0 || y == 0 {
                        0
                    } else {
                        data[i - 4 * w - 4]
                    } as i32;
                    let p = a + b - c;
                    let guess = [a, b, c].into_iter().min_by_key(|v| (p - v).abs()).unwrap();
                    for (k, predict) in [0, a, b, (a + b) / 2, guess].into_iter().enumerate() {
                        score[k] +=
                            u64::from((data[i].wrapping_sub(predict as u8) as i8).unsigned_abs());
                    }
                }
            }
            (0..5).min_by_key(|&k| score[k]).unwrap() as u8
        })
        .collect()
}

#[test]
#[cfg(all(target_os = "macos", feature = "metal"))]
#[ignore = "Requires a Metal device; run explicitly on the release Mac"]
fn exact_row_selection_on_metal() {
    check_scorer(ComputeMode::Metal);
}

#[test]
fn exact_row_selection_on_cpu() {
    check_scorer(ComputeMode::Cpu);
}

fn check_scorer(mode: ComputeMode) {
    for (w, h) in [(1, 1), (3, 5), (64, 3), (79, 67), (1024, 333)] {
        for pattern in 0..3 {
            let mut seed = 4351u32;
            let bytes: Vec<u8> = (0..w * h * 4)
                .map(|i| match pattern {
                    0 => 0,
                    1 => ((i / (4 * w) + i % 4) % 256) as u8,
                    _ => {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        (seed >> 24) as u8
                    }
                })
                .collect();
            assert_eq!(
                choose_rows(&bytes, w, h, mode).expect("row scorer unavailable"),
                reference(&bytes, w, h)
            );
        }
    }
}
