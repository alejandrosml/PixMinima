# ECT and Oxipng on the primary corpus

This supplementary size comparison uses the eleven originals pinned in the [corpus manifest](../corpus.json). ECT processes all eight PNGs and three JPEGs. Oxipng processes the eight PNGs. Compare PNG totals over those same eight inputs.

Both tools receive fresh copies. The runner checks their source SHA-256 hashes, then uses the main benchmark's `signature`, `require_identity` and `compare` functions. It records both the proposed file size and the accepted size. An unsupported change or an unverified output contributes the original byte count. For JPEG this includes quantized DCT coefficients, decoded pixels and ordered APP/COM metadata. For PNG it includes the full unassociated RGBA16 samples and each class of metadata defined by the main oracle. [Verification details](../README.md#fidelity)

## Results on Apple M1 Max

| Scope | Original bytes | ECT proposed | ECT accepted | Oxipng proposed | Oxipng accepted | PixMinima Maximum accepted |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 8 PNG | 1,260,294 | 835,928 | 871,418 | 839,909 | 1,216,615 | 838,594 |
| 3 JPEG | 910,029 | 861,202 | 891,966 | — | — | 854,571 |
| 11 files | 2,170,323 | 1,697,130 | 1,763,384 | — | — | 1,693,165 |

ECT produced the smallest proposed PNG total. The oracle rejected its `bw_text.png` for removing the background-color chunk and its `logo.png` for removing significant-bit information. For `hubble_deep_field.jpg`, ECT added an APP0 segment and preserved the five existing APP segments. [ECT results by file](results/ect.csv)

Oxipng replaced the ICC profile of `astronaut.png` with an sRGB declaration, changed the ordering of ancillary chunks in `bw_text.png` and renamed the embedded ICC profile in `page.png` from “ICC Profile” to “icc”. The uncompressed profile bytes in the last file were identical. These three outputs failed the metadata contract. All eight had matching decoded pixels. PixMinima's Maximum column uses the identical inputs and validation contract from the primary run. [Oxipng results by file](results/oxipng.csv) · [Primary benchmark](../results/m1-max/files.csv)

## Commands and binaries

| Tool | Sources | Batch settings |
| --- | --- | --- |
| ECT 0.9.5 | 11 PNG/JPEG files | `-9 --strict --mt-file=8 -progressive` |
| Oxipng 10.2.1 | 8 PNG files | `-o max --zopfli --threads 8 --keep bKGD,iCCP,iTXt,pHYs,sBIT,sRGB,tEXt,tIME` |

ECT uses level 9 with strict losslessness and allows progressive JPEG encoding. Oxipng uses its maximum filter preset with Zopfli's default of 15 iterations. The keep list comes from the actual ancillary chunk types in the source images. Palette transparency is checked through the resulting pixel values. These settings do not exhaust the optional palette permutations of ECT or all possible Zopfli iteration counts.

The ECT binary was built for ARM64 on macOS 27.2.1 with Apple Clang 21.0.0, CMake Release, multithreading and the submodules from commit `9aabc23d73899ae55c1de292592fed6eb6217f66`. The macOS 27 SDK lacks the legacy `fp.h` included by its copy of libpng. The build supplies that header through an extra include directory containing only `#include <math.h>`. The measured binary SHA-256 is `4e13606a873d4e08b3e20b3d0081bfdd8130f6ed3d0907c5f68f2c32c1efd702`. [ECT source](https://github.com/fhanau/Efficient-Compression-Tool/tree/9aabc23d73899ae55c1de292592fed6eb6217f66)

Oxipng is the upstream ARM64 macOS release at revision `36f3ef8aac65ecf1761739ea2f2e530281f1312b`. The release archive SHA-256 is `7039fcfc78e8aa1ed2b57d848057a0296f082e92b3e1807ac65402d10d926764`, checked against GitHub's release-asset digest. Its executable SHA-256 is `275a2049091caf576be74b838df4675fbb179c9f2d118cec44ea1521e54fc45e`. [Official Oxipng release](https://github.com/oxipng/oxipng/releases/tag/v10.2.1)

## Reproduction

Install FFmpeg and libjpeg-turbo's jpegtran for the oracle. Run from the repository root with a directory where the main benchmark may prepare its corpus.

```sh
python3 bench/run.py --prepare-only --corpus /tmp/pixminima-corpus
python3 bench/specialists/run.py ect --binary /path/to/ect \
  --corpus /tmp/pixminima-corpus --out /tmp/pixminima-specialists
python3 bench/specialists/run.py oxipng --binary /path/to/oxipng \
  --corpus /tmp/pixminima-corpus --out /tmp/pixminima-specialists
```

Build ECT on a Mac using the compiler supplied by Xcode and a recent CMake installation.

```sh
git clone --branch v0.9.5 --recurse-submodules https://github.com/fhanau/Efficient-Compression-Tool.git
mkdir -p ect-compat
printf '%s\n' '#include <math.h>' > ect-compat/fp.h
cmake -S Efficient-Compression-Tool/src -B ect-build \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POLICY_VERSION_MINIMUM=3.5
CPATH="$PWD/ect-compat" cmake --build ect-build --parallel
```

The runner allows 1,200 seconds per batch. The per-tool JSON includes the exit or timeout status, measured time, exact flags and binary hash. Timings describe one run per specialist and exclude source verification and output validation. The main app comparison uses a separate set of repeated timings.
