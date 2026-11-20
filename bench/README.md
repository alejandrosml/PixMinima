# Reproducing the comparison

The Apple M1 Max run of 20 November 2026 includes the [per-suite medians](results/m1-max/summary.csv), [batch timings](results/m1-max/runs.csv), [per-file fidelity checks](results/m1-max/files.csv) and [machine configuration with binary hashes](results/m1-max/results.json).

This harness measures size, fidelity and elapsed time separately. All commands operate on fresh working copies. The 11-image primary corpus contains eight PNGs and three JPEGs from scikit-image v0.25.2. SHA-256 hashes and the versioned download location are in [corpus.json](corpus.json). A second set of six public files and a separate generated coverage suite exercise features absent from the primary corpus. Each image retains its source licence. [scikit-image sample data](https://scikit-image.org/docs/0.25.x/api/skimage.data.html)

The primary corpus also served as a regression check during development. Its results describe these files. Separate training images were used to select JPEG scan and entropy-coding strategies.

Opt-in lossy compression has a separate quality evaluation. It records the largest channel error, the worst channel PSNR and the result of a second compression pass. [JPEG, GIF and WebP measurements and verifier](lossy/formats.md)

## Run

Install Python 3, the FFmpeg tools, libjpeg-turbo's jpegtran, and libwebp's cwebp. On macOS, install the signed ImageOptim application and build the official Chisel v2.0.0 tag. The harness reports binary hashes and versions. Run from the PixMinima repository.

```sh
cargo build --release --locked -p pixminima
python3 bench/run.py --prepare-only --corpus /tmp/pixminima-corpus --suites primary extended coverage
python3 bench/run.py --corpus /tmp/pixminima-corpus --out /tmp/pixminima-report \
  --pixminima target/release/pixminima --chisel /path/to/chsl \
  --backends pixminima pixminima-default chisel chisel-high imageoptim-app imageoptim-engines \
  --suites primary extended coverage --repeat 3
```

The Mac used `MACOSX_DEPLOYMENT_TARGET=27.0` and the normal automatic compute policy. The four competitor modes were measured alongside a development build. All published PixMinima measurements use the Developer ID-signed helper from the notarized 0.1.0 distribution, `PixMinima.app/Contents/Helpers/pixminima`, SHA-256 `079386650a9bf6585f4eaa367c91bf0ca70f9f1bdb386c6da0cd527b4f4f42b8`. The packaging script built it with `cargo build --release --locked --package pixminima --package pixminima-desktop --features pixminima/metal`. Chisel was built in Release mode from commit `600e4b92d07ac56bc65a020942227a5839ecf266`, with its pinned submodules. [Chisel v2.0.0 source](https://github.com/Snesnopic/chisel/tree/600e4b92d07ac56bc65a020942227a5839ecf266)

Choose a fresh report directory on each invocation. Existing, hash-matching source images may be supplied via repeated `--source-dir` options. On Linux, omit the ImageOptim modes via `--backends pixminima pixminima-default chisel chisel-high`. For a two-file functional check, select `--suites smoke --no-timer`. [Chisel source](https://github.com/Snesnopic/chisel) · [ImageOptim command-line interface](https://imageoptim.com/command-line.html)

## What each mode measures

| Identifier | Processing |
| --- | --- |
| `pixminima` | PixMinima CLI, thorough preset, saving copies |
| `pixminima-default` | Same binary at its normal preset |
| `pixminima-maximum` | PixMinima CLI with the optional maximum-effort preset |
| `chisel` | Source-built Chisel with metadata retention and `--verify-checksums`; default Zopfli effort |
| `chisel-high` | Same binary/checks and 100 regular or 50 large-image Zopfli iterations |
| `imageoptim-app` | Original signed Mac application, synchronous CLI, factory defaults |
| `imageoptim-engines` | Separate calls to engines in the installed bundle. For PNG, oxipng at max with Zopfli and zopflipng retain named metadata. For JPEG, jpegtran copies markers and optimizes Huffman tables with two scan choices. For GIF, gifsicle runs at level 3. The smallest verified candidate competes against the original. |

The official macOS command accepts filenames only. Its bundled defaults remove metadata. The probe with isolated preference files did not cause the macOS preference system to load those settings. The harness runs the unmodified signed bundle with access to the live preferences denied, using a temporary home and the protection profile [imageoptim.sb](imageoptim.sb). It never writes a live preference or controls an already-running app. Commentary about ImageOptim's strict engine comparison applies to the explicit selection above. The suite records unchanged files even when a particular tool has no processor for their format.

The M1 Max run invoked `/Applications/ImageOptim.app/Contents/MacOS/ImageOptim`, the same executable used by the graphical application. The installed bundle identified itself as `net.pornel.ImageOptim` version 1.9.3. Its Apple Developer ID signature verified, with Kornel Lesinski as signer, team `59KZTZA4XR`, and code directory hash `b313f146f11f7abdac2e790ee2f5edc1f327a641`. All eleven primary working copies changed. For example, `coins.png` shrank from 75,825 to 72,962 bytes and passed every check. `astronaut.png` shrank from 791,555 to 422,012 bytes, retained its pixel samples and changed its metadata. Metadata was the only observed difference in the eight rejected outputs.

## Fidelity

The primary PNG oracle checks chunk CRCs, reconstructs scanlines and Adam7 passes independently, expands samples to unassociated RGBA16 and compares every channel, including RGB at zero opacity. It compares named ICC profiles after decompression, EXIF/orientation payloads, text, ancillary data, and semantic background/significant-bit metadata. Palettes are checked through the resulting samples. Unknown chunks marked unsafe to copy require byte identity.

JPEG checks use libjpeg-turbo's coefficient-preserving jpegtran with fixed output ordering and optimized progressive Huffman coding to canonicalize the DCT data. FFmpeg independently decodes stored pixels with autorotation disabled. Ordered APP/COM segments, including EXIF, ICC and XMP, are compared byte-for-byte. Composite/signed JPEGs and trailers require byte identity.

Coverage checks include APNG frame pixels and frame controls, GIF frame rectangles/palette colors/transparency and timing, and WebP frame pixels, blending/disposal flags, container metadata and timing. ICO/CUR compare all PNG members and directory characteristics. TIFF checks each page by decoding it separately and preserving sample metadata. Eight-bit RGB/palette images are independently decoded at their native depth, then expanded by 257 to RGBA16. SVG/SVGZ compare XML structure, text, comments and attributes. OpenRaster compares layer and merged PNGs as independent images, including their pixels, color profiles and metadata. It checks the ZIP structure, per-entry metadata and exact content of all other members. PNGs with opaque offset-sensitive chunks require byte identity. Additional generated cases exercise QOI, TGA and Netpbm. Each oracle applies to the variants in its suite.

## Reading results

`files.csv` records the source, proposed and accepted sizes and file hashes. `rejected` identifies a changed property. `missing-output`, `no-engine` and `unverified` retain the corresponding source bytes in the accepted total. A larger verified proposal also keeps the source size. `identical` means the observed bytes agree with the input. It does not establish format support.

`runs.csv` reports wall time per complete batch, including process startup and writes. Input copying and external verification are outside the timer. Each backend executes sequentially in seeded, shuffled order for every repetition. The standalone engine mode runs files sequentially, while the applications may use internal parallelism. `summary.csv` lists the median and range for each suite. `results.json` also contains the machine, versions and binary hashes. Corpus totals are byte weighted. Keep primary, extended and coverage results separate when reporting rankings.

For the shipping CLI, point `--pixminima` at `PixMinima.app/Contents/Helpers/pixminima`, select `--backends pixminima-default pixminima pixminima-maximum` and use a fresh `--out` directory. This second session remeasures all three presets, with opt-in lossy compression disabled. The following command combines the sessions after checking input hashes, machine and verification toolchain. The JSON retains each binary hash, the modes originally recorded, and the modes used in the comparison.

```sh
python3 bench/merge.py --out /tmp/pixminima-combined \
  --replace pixminima-default --replace pixminima /tmp/pixminima-report /tmp/pixminima-shipping
```

## Additional public-file and macOS replacement audit

The untimed audit downloads 23 [hash-pinned GIF, TIFF and WebP files](fidelity-sources.json) from Pillow, libtiff and libjxl. Four further inputs come from their public images using libwebp and libtiff tools. These exercise lossless WebP with EXIF orientation, ICC, XMP and transparency, animated WebP, and 16-bit TIFF orientation. Supply existing downloads with repeated `--source-dir` options. This audit replaces private working copies and records the input and output SHA-256, size, exit status and fidelity result for each file.

On macOS, `--mac-attributes` also assigns a Finder tag, a resource fork, an opaque extended attribute, a file ACL, the BSD hidden flag, mode 0640, and distinct creation and modification times to each copy. The audit compares them before and after replacement, along with the owner and group. It requires Apple's `SetFile`, FFmpeg, `gif2webp`, `dwebp`, `webpmux`, `cwebp`, `tiffcp` and `tiffset`.

```sh
python3 bench/fidelity.py --pixminima target/release/pixminima \
  --mac-attributes --effort standard --out /tmp/pixminima-filesystem-audit.json
```
