# PixMinima, ImageOptim and Chisel on M1 Max

[Français](comparison.fr.md) · [Use PixMinima](../README.md)

On the primary collection of eleven images, PixMinima Maximum produces **1,693,165 bytes**, **2,355 bytes smaller than high-effort Chisel**. PixMinima wins on 8 individual files and Chisel wins on 3. Their median batch times are 95.42 and 57.40 seconds. Total reduction is calculated from the sum of the file sizes.

## Eleven photos and illustrations

Eight public PNGs and three public JPEGs from scikit-image total 2,170,323 input bytes. Three runs of every configuration start from fresh copies on the same M1 Max. The final column counts candidates for which the benchmark substituted the original size after its checks. An identical file returned by a tool counts normally toward its total.

| Application and setting | Verified size (bytes) | Total reduction | Median time | Originals counted after checks |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 1,693,165 | 21.99% | 95.42 s | 0 |
| Chisel, high effort | 1,695,520 | 21.88% | 57.40 s | 0 |
| PixMinima, thorough | 1,700,035 | 21.67% | 23.54 s | 0 |
| Chisel, standard | 1,707,276 | 21.34% | 6.04 s | 0 |
| ImageOptim, engines configured to retain data | 1,715,047 | 20.98% | 56.23 s | 0 |
| PixMinima, fast | 1,809,018 | 16.65% | 0.15 s | 0 |
| ImageOptim, factory defaults | 2,154,212 | 0.74% | 20.04 s | 8 |

The publisher-signed ImageOptim 1.9.3 Mac application produced 1,674,780 bytes with factory defaults. It preserved image pixels on all eleven files. Eight files changed metadata, so the table includes their original sizes. ImageOptim also offers a preference to retain metadata. The configured-engines row measures an explicit selection of its bundled executables. The benchmarks ran with factory defaults isolated from the user's preferences. [ImageOptim features](https://imageoptim.com/mac) · [Official synchronous interface](https://imageoptim.com/command-line.html)

The eight PNGs total 838,594 bytes with PixMinima Maximum and 840,999 with high-effort Chisel. The three JPEGs total 854,571 and 854,521 bytes, respectively. The modest overall lead depends on the mix of images.

## Six additional public images

This separate collection includes a 16-bit multipage TIFF. The total input size is 251,696 bytes.

| Application and setting | Verified size (bytes) | Total reduction | Median time | Originals counted after checks |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 146,143 | 41.94% | 30.00 s | 0 |
| Chisel, high effort | 147,936 | 41.22% | 7.11 s | 0 |
| PixMinima, thorough | 149,442 | 40.63% | 12.49 s | 0 |
| Chisel, standard | 150,842 | 40.07% | 1.09 s | 0 |
| ImageOptim, engines configured to retain data | 229,508 | 8.82% | 5.78 s | 2 |
| PixMinima, fast | 159,967 | 36.44% | 0.18 s | 0 |
| ImageOptim, factory defaults | 247,601 | 1.63% | 5.22 s | 2 |

## Twenty preservation cases

This generated 35,000-byte suite includes animation, transparency, hidden RGB, profiles, text and image containers. It exercises specific properties of individual formats. The format mix differs from that of a typical photo library.

| Application and setting | Verified size (bytes) | Total reduction | Median time | Originals counted after checks |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 12,383 | 64.62% | 0.37 s | 0 |
| Chisel, high effort | 18,878 | 46.06% | 0.65 s | 3 |
| PixMinima, thorough | 12,399 | 64.57% | 0.14 s | 0 |
| Chisel, standard | 18,881 | 46.05% | 0.07 s | 3 |
| ImageOptim, engines configured to retain data | 25,413 | 27.39% | 0.86 s | 15 |
| PixMinima, fast | 12,520 | 64.23% | 0.11 s | 0 |
| ImageOptim, factory defaults | 35,000 | 0.00% | 5.64 s | 9 |

Recompressing PNG layers inside an OpenRaster archive earns credit when decoded layers, their metadata and archive properties are preserved. The GIF test also checks the frame count and individual timings. The PPM test retains comments. A JPEG with an opaque trailer requires whole-file identity to preserve the locations of its trailing data. This conservative check can reject a result that preserved the pixels and the trailer bytes.

## CPU and Metal on the same Mac

For one 3,840 × 2,160 RGBA image, selecting a filter per row took 11.03 ms on the CPU and 2.60 ms on a warm GPU. Complete optimization also includes reading, DEFLATE, verification and replacement. Across nine distinct 4K PNGs, median whole-batch times were 13.72 s with CPU, 13.52 s with forced Metal and 13.24 s with automatic selection. For thirteen images of mixed sizes, the medians were 13.62 s, 15.01 s and 14.13 s respectively. All 18 runs produced byte-identical outputs for the same input files. Three runs per setting leave room for variation, particularly when times are close. [Timings and per-file hashes](../bench/metal/)

## Choosing a tool

- **PixMinima** combines in-place replacement without backups, per-file savings, language settings and several search efforts. Its engine retains supported metadata and includes processing for Photoshop composites, icon resources and layered image containers. HEIC, HEIF, AVIF and JPEG XL have standard container optimizations. [Formats and limits](formats.md)
- **ImageOptim** offers a drag-and-drop Mac interface and supports versions of macOS earlier than Golden Gate. Its Cocoa application includes Objective-C code and runs several image optimizers. The application is licensed under GPL version 2 or later. The bundled PNGOUT component has its own license. [Features and license](https://imageoptim.com/mac) · [Source](https://github.com/ImageOptim/ImageOptim)
- **Chisel** is an MIT-licensed command-line optimizer written primarily in C++. It covers many file families beyond images. Its high-effort setting processed both public collections faster than PixMinima Maximum on this Mac. [Source and license](https://github.com/Snesnopic/chisel)
- **Other open-source projects.** Oxipng specializes in PNG and APNG optimization and offers a Rust library. Efficient Compression Tool is written in C++ and supports PNG, JPEG, ZIP and GZIP. Across the eight primary PNGs, ECT produced 835,928 bytes before metadata checks and 871,418 under the benchmark retention policy. Oxipng produced 839,909 and 1,216,615 bytes respectively. PixMinima Maximum produced 838,594 verified bytes. [Measurements and metadata differences](../bench/specialists/) [Oxipng](https://github.com/oxipng/oxipng) · [Efficient Compression Tool](https://github.com/fhanau/Efficient-Compression-Tool)

The size comparisons above use lossless PixMinima settings. Optional lossy encoding has separate quality measurements. [PNG results](../bench/lossy/PNG.md) · [JPEG, GIF and WebP results](../bench/lossy/formats.md)

Results were collected on 20 November 2026 on an M1 Max running macOS 27.2.1. The primary collection also served as a development regression check. Per-file outcomes, fingerprints, medians, all individual timings and fidelity rules are available alongside the protocol. [Benchmark data](../bench/) · [CPU and Metal measurements](../bench/metal/)
