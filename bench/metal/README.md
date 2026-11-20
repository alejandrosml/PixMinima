# CPU and Metal on Apple M1 Max

Measured locally on a 10-core CPU, 32-core GPU M1 Max with 64 GB unified memory under macOS 27.2.1. The row test and the complete optimizer measure different parts of the same workload.

## Selecting PNG row filters

The five PNG predictors can be scored independently by row and position against the previous unfiltered row. The Metal compute pass combines partial scores and returns one filter choice per row. The host still has to prepare the buffer, schedule the command, wait for completion and read the choices. The warm times below include the host calls, transfer and readback for that scoring pass. The CPU uses an optimized Rust implementation. Eleven repetitions alternate the measurement order and check identical row selections.

| RGBA8 dimensions | Decoded bytes | Optimized CPU | Warm Metal |
| --- | ---: | ---: | ---: |
| 512 × 512 | 1,048,576 | 0.416 ms | 0.467 ms |
| 1024 × 512 | 2,097,152 | 0.873 ms | 0.561 ms |
| 3840 × 2160 | 33,177,600 | 11.034 ms | 2.603 ms |
| 64 × 8192 | 2,097,152 | 0.816 ms | 0.897 ms |
| 8192 × 64 | 2,097,152 | 0.710 ms | 0.692 ms |

For a 3840 × 2160 image, warm scoring was about 4.2 times as fast on Metal. Even at the same 2 MiB input size, shape influenced the result. Very narrow rows, short images, or a newly created Metal device can change which processor finishes first. The first hardware call includes initialization. [All dimensions and first-call timings](rows-m1-max.csv) · [Hardware benchmark](../../crates/compute/examples/row_bench.rs)

## Optimizing complete files

The test uses thirteen distinct PNGs, including a subset of nine 4K images. The corpus builder makes uncompressed RGBA8 PNGs from pinned photographic and artwork sources. This exposes filter scoring and makes every engine start with the same image representation. At startup each measured process uses a fresh automatic calibration. Copies are made before timing. The timer includes reading, recompressing at fast effort, verifying, staging and replacing the files on disk. The test compares the SHA-256 of every output across the three modes and all repetitions.

| Corpus | Files | Mode | Median | Range of three runs | Final bytes |
| --- | ---: | --- | ---: | ---: | ---: |
| Mixed sizes | 13 | cpu | 13.62 s | 13.55–14.88 s | 51,405,148 |
| Mixed sizes | 13 | metal | 15.01 s | 12.97–16.41 s | 51,405,148 |
| Mixed sizes | 13 | auto | 14.13 s | 14.08–14.35 s | 51,405,148 |
| 4K subset | 9 | cpu | 13.72 s | 13.52–13.90 s | 49,140,140 |
| 4K subset | 9 | metal | 13.52 s | 13.37–14.23 s | 49,140,140 |
| 4K subset | 9 | auto | 13.24 s | 13.18–14.52 s | 49,140,140 |

The rows report elapsed times on this Mac for the specified files and profile. The PNG size, row width and length, CPU load, GPU availability, host transfers and time spent in DEFLATE influence the complete batch. Additional images provide more opportunities to overlap CPU and GPU work. A large batch composed solely of small images can still run faster on the CPU. [Individual timings](end-to-end-m1-max.csv) · [Output hashes](output-hashes-m1-max.csv)

## Automatic selection

The application groups eligible decoded RGBA8 images by size and aspect ratio. For inputs at least 2 MiB with at least 64 rows and columns, it first gathers CPU samples, then measures both scorers on matching data. Metal is enabled for the group after two of three comparisons show a reduction of at least 20%. Automatic mode can fall back to the CPU if the GPU is busy. Groups are sampled again after repeated use. The queue separately limits concurrent files according to available memory. The GPU does not run the entire compressor. DEFLATE searches, JPEG processing and file validation use the CPU. [Selection policy](../../crates/compute/src/automatic.rs) · [GPU implementation](../../crates/compute/src/metal.rs)

## Reproduction

The row benchmark requires a Metal Mac. The full comparison additionally requires FFmpeg to prepare the referenced sources. The script verifies the source, decoded pixels and generated PNGs against the corpus manifest. Last modified times and preexisting compression can differ in real libraries.

```sh
cargo run --locked --release -p pixminima-compute --features metal --example row_bench
python3 bench/metal/run.py /path/to/PixMinima.app/Contents/Helpers/pixminima /tmp/pixminima-corpus --runs 3 --write /tmp/m1-times.csv --hashes /tmp/m1-hashes.csv
```

[Corpus sources and fingerprints](corpus.json)
