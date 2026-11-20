# Optional PNG palette compression

PixMinima tries a full-color lossless result first. With the high-quality lossy option enabled, it also tries an indexed representation and keeps the smaller validated output. The choice is made per file. In this 14-image validation set, ten images used a lossy palette and four retained exact samples. Thorough mode produced **152,193 bytes** with the option enabled, compared with **408,990 bytes** in strict mode and **540,665 bytes** for the originals. These measurements describe small validation crops and two generated patterns. The primary benchmark ranks lossless compression separately. [Lossless comparison](../README.md)

## End-to-end results

All sizes are bytes. PSNR measures mean squared error across the three stored 8-bit RGB components of every pixel, including fully transparent ones. “Exact” indicates that every stored RGB and opacity sample is preserved. Quantized outputs retained every opacity value and all RGB underneath transparent pixels.

| Input | Original | Thorough lossless | Fast with option | Thorough with option | Thorough RGB PSNR, dB |
| --- | ---: | ---: | ---: | ---: | ---: |
| alpha.png | 1,664 | 952 | 1,225 | 952 | exact |
| brick.png | 18,639 | 16,625 | 6,785 | 5,361 | 42.35 |
| cell.png | 10,252 | 8,078 | 3,812 | 3,232 | 41.52 |
| chelsea.png | 85,297 | 83,056 | 28,548 | 26,415 | 36.11 |
| coffee.png | 77,994 | 73,024 | 13,400 | 12,298 | 35.23 |
| color.png | 36,682 | 28,458 | 36,267 | 28,458 | exact |
| grass.png | 36,319 | 35,746 | 23,073 | 19,768 | 40.85 |
| gravel.png | 32,489 | 32,040 | 19,343 | 17,117 | 40.82 |
| ihc.png | 78,570 | 74,627 | 23,516 | 20,111 | 35.52 |
| moon.png | 7,751 | 6,880 | 3,268 | 2,800 | 42.14 |
| phantom.png | 2,113 | 1,484 | 2,107 | 1,484 | exact |
| rgba-metadata.png | 98,961 | 906 | 1,069 | 906 | exact |
| rocket.png | 36,184 | 30,407 | 8,517 | 5,813 | 36.94 |
| text.png | 17,750 | 16,707 | 9,154 | 7,478 | 41.01 |
| **All files** | **540,665** | **408,990** | **180,084** | **152,193** | — |

Fast lossless produced 441,441 bytes. For the ten quantized images, the minimum RGB PSNR was 35.16 dB in Fast and 35.23 dB in Thorough. Their respective medians were both 40.84 dB. Photo content matters. The cat and coffee crops benefited, while the color chart kept its lossless representation. The transparent gradient with an ICC profile, title and physical pixel size gave a valid 1,181-byte lossy candidate. The 906-byte lossless candidate won. The measured constituent candidates were independently decoded using both libpng and Pillow.

## Selection and acceptance

The quantizer groups 5-bit RGB histogram bins by exact 8-bit opacity. It seeds palettes using weighted farthest-point selection in Oklab, then refines cluster centers by weighted K-means using Exoquant. The coordinate transform models sRGB during selection. For files with an embedded RGB ICC profile, PixMinima preserves the original profile and tests native encoded samples. The bounds below describe code values and cannot guarantee perceptual invisibility on every display. Each selected palette is encoded as a real PNG using PixMinima's predictor and DEFLATE search. [Exoquant](https://github.com/exoticorn/exoquant-rs) · [PNG implementation](../../crates/png-engine/src/lossy.rs)

Fast tries palettes with 64, 128 and 256 entries and four K-means iterations. Thorough tries 32, 64, 128, 192 and 256 entries and eight iterations. Pure gray images use at most 64 entries. After decoding each prospective output, the engine checks dimensions, opacity, hidden RGB, and payloads and ordering of supported auxiliary chunks. A lossy candidate must change at least one RGB sample and be smaller than the source. Its error must meet all five limits.

| Measure | Acceptance limit |
| --- | ---: |
| Mean squared RGB error per component | ≤ 20, equivalent to RGB PSNR ≥ 35.12 dB |
| Mean squared encoded-luma error over the whole image | ≤ 12 |
| Mean squared encoded-luma error over every 32 × 32 tile, including smaller edge tiles | ≤ 49 |
| Pixels with any absolute channel error greater than 24 | ≤ 1% |
| Largest absolute channel error in any pixel | ≤ 64 |

Luma uses the fixed-point RGB weights (54, 183, 19) / 256. Average error can hide local variation, which the per-tile and peak checks constrain. A 256-entry palette may be too small for photographs with difficult color transitions or many distinct RGB and alpha combinations. If every palette fails, the normal lossless winner remains available.

The optional path supports static, non-interlaced 8-bit gray, gray-alpha, RGB and RGBA PNGs up to 12 million pixels and 131,072 occupied RGB/alpha histogram bins. Source transparency keys are expanded to their effective opacity. Text, timestamps, physical size, gamma, chromaticity, sRGB and compatible RGB ICC chunks are retained. Existing palette hints, background or significant-bit metadata, unknown chunks, signed content, 16-bit data and APNG do not enter quantization. Reopening an indexed output never triggers a second lossy quantization. If a later strict optimizer expands the indexed output as gray or RGB, the restricted color counts still protect it from further color loss. Further strict compression may change the file bytes.

## Reproduction

The source photographs and patterns come from scikit-image 0.25.2. The fixture generator verifies the SHA-256 of the twelve source files and takes their centered square crops, up to 208 pixels per side. The two RGBA gradients are algorithmically generated. An sRGB profile with a fixed creation timestamp is attached to RGB crops and gradients. The measured fixtures used Pillow 13.0.0, Little CMS 2.19 and Pillow's zlib-ng backend reporting zlib 1.3.1.zlib-ng. Different PNG or JPEG dependencies can affect the initial file bytes or generated profile. [Source images and licensing information](https://github.com/scikit-image/scikit-image/tree/v0.25.2/skimage/data) · [Fixture generator](png-fixtures.py)

Use a Python environment with Pillow and its Little CMS support. From the project root, set up a fresh directory and run the release CLI. Each output directory is new. The independent verifier checks decoded RGBA, metadata, peak errors, the RGB and luma means and each tile.

```sh
cargo build --release --locked -p pixminima
workdir="$(mktemp -d)"
python3 bench/lossy/png-fixtures.py "$workdir/input"
target/release/pixminima --compute cpu --json --output-dir "$workdir/strict-fast" "$workdir/input/"*.png
target/release/pixminima --thorough --compute cpu --json --output-dir "$workdir/strict" "$workdir/input/"*.png
target/release/pixminima --lossy --compute cpu --json --output-dir "$workdir/fast" "$workdir/input/"*.png
target/release/pixminima --lossy --thorough --compute cpu --json --output-dir "$workdir/thorough" "$workdir/input/"*.png
python3 bench/quality.py --originals "$workdir/input" --outputs "$workdir/fast" --out "$workdir/fast.json"
python3 bench/quality.py --originals "$workdir/input" --outputs "$workdir/thorough" --out "$workdir/thorough.json"
target/release/pixminima --lossy --thorough --compute cpu --json --output-dir "$workdir/second" "$workdir/thorough/"*.png
```

The CLI reports `lossy_used` separately for every file. Running Thorough with the option on the 14 Thorough outputs performed zero new quantizations and saved zero additional bytes on this set. The decoder tests also exercise the second pass after a palette has been expanded into a truecolor PNG. [Per-file checksums and measured quality](png-quality.json) · [PNG regression tests](../../crates/png-engine/src/tests.rs)
