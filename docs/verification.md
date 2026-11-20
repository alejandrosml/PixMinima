# Validation on Apple silicon

The development Mac was an Apple M1 Max with 10 CPU cores, a 32-core GPU and 64 GiB of unified memory, running macOS 27.2.1. The distribution targets macOS 27.0 or later. All write tests used disposable files.

## Image fidelity in the default mode

| Input | Independent check | Observation |
| --- | --- | --- |
| PNG and APNG | libpng on 35 PngSuite inputs and 35 animation fixtures covering 226 frame rectangles | Compared decoded samples, including 16-bit and hidden RGB, and animation frame controls. A separate training set compared the thorough and Maximum searches. |
| JPEG | libjpeg-turbo's jpegtran and FFmpeg | Verified DCT coefficients after canonical encoding, decoded pixels and ordered application/comment metadata on the benchmark set and separate training files. |
| WebP, GIF and TIFF | Reference cwebp/webpmux, external decoders, public Pillow and libtiff samples | Checked hidden RGB, alpha, animation timing, ICC/XMP/EXIF and TIFF sample values. The 27-file audit included eleven smaller verified files and sixteen byte-identical ones. |
| PSD and PSB | psd-tools on 73 public native files plus generated 8-, 16- and 32-bit images | Compared composite samples and retained all preceding document data. |
| OpenEXR | OpenEXR reference decoder on 42 fixtures and real files | Compared original channel bits and header attributes. A 725,142-byte grid compressed to 4,564 bytes at Maximum. |
| HEIC, HEIF and AVIF | Public file collection and independent decoded sample checks | Six of 44 public files saved between 4 and 16 bytes. The other 38 retained their source bytes. An additional HEIC from the native Mac encoder retained the same independently decoded image. |
| ICNS | Apple's iconutil and a separate FFmpeg decode for every PNG in the extracted icon set | An icon containing ten PNG renditions fell from 92,832 to 51,794 bytes at standard effort. Every decoded RGBA64 rendition matched. |
| JPEG XL container | Reference cjxl/djxl encode, decode and XMP extraction | A fixture with 23,403 bytes of repetitive XML shrank from 23,634 to 394 bytes. Uncompressed XML and decoded PPM matched. An archive with JPEG reconstruction returned the same JPEG hash. |

Synthetic examples with repeated data exercise specific encoders. Their savings describe the named fixtures. The separate comparison uses public photographic and illustration inputs. [Comparative evidence](../bench/)

## Optional lossy compression

The lossless comparisons above leave the optional lossy setting off. A separate independent check decoded the selected outputs, compared metadata and tracked the maximum channel difference, transparency and second-pass stability. On 14 PNGs, thorough lossless compression produced 408,990 bytes. Enabling the optional palette search brought this to 152,193 bytes. Ten files used a quantized image and four took the smaller lossless result. For JPEG, GIF and WebP, the published examples distinguish the codec's own candidates from the final portfolio selection. [PNG collection and quality figures](../bench/lossy/PNG.md) · [Other bounded-error codecs](../bench/lossy/formats.md)

## Files on macOS

The tests verified atomic replacement only after a smaller equivalent result was available, preservation of an unchanged inode on a repeat pass, separate copies, optional undo and recovery after an interrupted restore. Read-only, malformed and multiply linked originals were protected. A separate copy of a read-only source retained its mode.

External checks compared creation and modification dates, owner, group, permissions, extended ACLs, Finder tags, a custom extended attribute, flags and the resource fork. APFS staging explicitly copies the source ACL. Workflow tests also check Unicode names, overlapping batch writes and refusal to restore over later edits. [Linux and Windows qualification](portability.md)

## Release and updates

The distributed Apple silicon disk image is 10,770,745 bytes. Its SHA-256 is `5882c80912c06e37c2aac05d5121178a3a02f6159afa2eade59ddb4f957ae19f`. Apple notarized the application and the disk image independently. Gatekeeper accepted both the downloaded disk format and the application mounted from it with the status **Notarized Developer ID**. The executable declares a minimum of macOS 27.0. The updater was exercised on disposable copies with real publisher signatures, an older installed bundle and a notarized newer disk image. [Update validation](updates.md)

## Reproduce

```sh
cargo test --workspace --locked
cargo test --locked -p pixminima-jxl --features archive
cargo clippy --workspace --locked --all-targets --no-deps -- -D warnings
./scripts/audit-codecs.py
```

The CPU and Metal row scorers have a common independent scalar reference. On a Mac with Metal access, run the following hardware test in addition to the normal workspace suite.

```sh
cargo test --locked -p pixminima-compute --features metal --test metal -- --ignored
```

For the distribution, Apple's standard checks can be run against the downloaded disk image and the application inside it.

```sh
shasum -a 256 -c PixMinima-macOS-arm64.dmg.sha256
xcrun stapler validate PixMinima-macOS-arm64.dmg
codesign --verify --deep --strict /Volumes/PixMinima/PixMinima.app
spctl --assess --type execute --verbose=2 /Volumes/PixMinima/PixMinima.app
```
