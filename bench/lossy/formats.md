# Optional bounded-error JPEG, GIF and WebP compression

PixMinima uses lossless compression by default. These measurements exercise the optional lossy codec directly at standard effort. The application also considers its regular lossless candidate and can further optimize entropy coding. Its final sizes can therefore be smaller. These images were used during development. The measured savings and quality apply to this set.

## Measured samples

| Input or development set | Input bytes | Selected bytes | Worst channel PSNR | Largest channel error |
|---|---:|---:|---:|---:|
| 20 JPEG inputs from 3 photographs, re-encodings and generated patterns | 1,290,457 | 1,080,117 | 43.75 dB | 18 |
| JPEG with EXIF orientation 6, ICC, XMP and comment | 51,091 | 43,223 | 47.15 dB | 8 |
| WebP photo with ICC, EXIF and XMP | 23,000 | 13,978 | 40.51 dB | 4 |
| WebP with transparency | 29,348 | 21,562 | 45.72 dB | 4 |
| Two-frame WebP with different rectangles and durations | 49,084 | 32,276 | 40.51 dB | 4 |
| GIF with a second frame referencing the global palette | 29,116 | 28,532 | 47.89 dB | 4 |
| Animated traffic-light GIF | 1,088 | 1,078 | 58.72 dB | 4 |
| Transparent GIF | 9,678 | 9,528 | 56.14 dB | 4 |

Sixteen of the twenty JPEGs became smaller. One of those sixteen retained identical decoded samples. Three of eight GIFs and two of seven WebPs in the public fixture set became smaller. The annotated JPEG and two-frame WebP are additional cases. A second call to the lossy codec found no further candidate on any of these 37 inputs. Per-file SHA-256, frame counts, sizes and measurements are in [codec-quality.json](codec-quality.json).

## Acceptance criteria

- **JPEG.** The encoder rounds existing quantized DCT coefficients onto absolute target tables. It keeps DC and the five lowest AC frequencies, sampling factors, dimensions and ordered APP/COM payloads. The same thresholds apply on subsequent runs. A separate JPEG decoder checks the candidate against its input. Each RGB or grayscale channel must have whole-image mean squared error (MSE) ≤ 6.25, MSE ≤ 36 in every 8 × 8 tile and a peak absolute error ≤ 20 on an eight-bit scale. Compound JPEGs, offset-sensitive attachments and unsupported variants retain the lossless path. [Implementation and quantizers](../../crates/formats/vendor/smallpeg/src/requantize.rs)
- **GIF.** Pixels can be remapped to a nearby existing palette entry, with peak absolute RGB error ≤ 4 and MSE ≤ 6.25 per frame and channel. All palette bytes, the transparent index, pixels underneath transparency, frame coordinates, timing, disposal and extension payloads are preserved.
- **WebP.** The optional path uses an absolute eight-step RGB grid on VP8L frames. It checks peak channel error ≤ 4 and MSE ≤ 6.25 per frame and channel. Every alpha value and all RGB values underneath full transparency must match exactly. VP8 frames keep their original compressed data. Animation frame control bytes, backgrounds, ICC, EXIF and XMP are preserved.

## Independent verification

The verifier decodes JPEG and WebP with FFmpeg and reads ordered JPEG markers or WebP container metadata separately. For animated WebP it decodes each individual frame rectangle, including pixels that the final composited animation hides. For GIF an independent Python LZW implementation reads each frame's palette indices and compares alpha, hidden colors, exact palettes, extension metadata and controls. The lowest PSNR across all channels and frames uses an eight-bit peak of 255. A null PSNR in the JSON means all decoded color channels match exactly. The peak column reports the largest absolute difference in a single channel. These metrics quantify sample differences. Perceptual visibility also depends on the content and viewing conditions. [Verifier](../quality.py)

The JPEG development photographs came from scikit-image's Chelsea, coffee and immunohistochemistry samples, including encodes at qualities 90, 95, 98 and 100. The GIF and WebP sources come from Pillow, libtiff and libjxl. The WebP derivatives were prepared with libwebp utilities. The animated example combines the photographic and transparent VP8L inputs with 40 ms and 90 ms delays. [scikit-image sample provenance](https://scikit-image.org/docs/0.25.x/api/skimage.data.html) · [Pinned public images](../fidelity-sources.json) · [Derived fixtures](../fidelity.py)

To verify a release CLI on your own set, use directories containing JPEG, GIF or WebP images. Process the originals into a new output directory, then compare independent decodes. Repeat on the output and compare the two passes to measure stability.

```sh
pixminima --lossy --output-dir /tmp/pixminima-first originals
python3 bench/quality.py --originals originals --outputs /tmp/pixminima-first --out /tmp/pixminima-quality.json
pixminima --lossy --output-dir /tmp/pixminima-second /tmp/pixminima-first
python3 bench/quality.py --originals /tmp/pixminima-first --outputs /tmp/pixminima-second --out /tmp/pixminima-repeat.json
```
