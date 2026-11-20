//! Run on the release Mac using --release --features metal.
use pixminima_compute::{ComputeMode, choose_rows};
use std::hint::black_box;
use std::time::Instant;

fn measure(data: &[u8], w: usize, h: usize, mode: ComputeMode) -> (Vec<u8>, u128) {
    let start = Instant::now();
    let result = choose_rows(black_box(data), w, h, mode).expect("row scorer unavailable");
    (result, start.elapsed().as_nanos())
}

fn main() {
    println!("width,height,bytes,first_metal_ns,cpu_median_ns,metal_median_ns,repetitions");
    for (w, h) in [
        (32, 32),
        (64, 64),
        (128, 128),
        (256, 256),
        (512, 256),
        (512, 512),
        (1024, 512),
        (1024, 1024),
        (2048, 1024),
        (2048, 1536),
        (2048, 2048),
        (3072, 2048),
        (3840, 2160),
        (4096, 2160),
        (1024, 8192),
        (8192, 1024),
        (64, 8192),
        (8192, 64),
        (8, 16384),
        (16384, 8),
    ] {
        let mut state = 9911_u32;
        let data: Vec<u8> = (0..w * h * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let (expected, _) = measure(&data, w, h, ComputeMode::Cpu);
        let (first, cold_ns) = measure(&data, w, h, ComputeMode::Metal);
        assert_eq!(first, expected);
        let mut cpu = Vec::new();
        let mut metal = Vec::new();
        for i in 0..11 {
            let mut modes = [ComputeMode::Cpu, ComputeMode::Metal];
            if i % 2 == 1 {
                modes.reverse();
            }
            for mode in modes {
                let (result, ns) = measure(&data, w, h, mode);
                assert_eq!(result, expected);
                if mode == ComputeMode::Cpu {
                    cpu.push(ns);
                } else {
                    metal.push(ns);
                }
            }
        }
        cpu.sort_unstable();
        metal.sort_unstable();
        println!(
            "{w},{h},{},{cold_ns},{},{},{}",
            data.len(),
            cpu[cpu.len() / 2],
            metal[metal.len() / 2],
            cpu.len()
        );
    }
}
