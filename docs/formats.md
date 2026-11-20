# Formats and preservation

With the default lossless setting, PixMinima keeps the smallest verified candidate, including the input. Re-encoding a JPEG which was already lossy retains its existing image information. The standard optimizer keeps the file format and runs locally. A container-only optimization can save a handful of bytes even when its image stream cannot be reduced.

| Family | What the engine can change | What it checks or retains |
| --- | --- | --- |
| PNG, APNG | Reversible row filters, DEFLATE parsing and blocks, exact sample-depth and palette reductions where compatible, repeated animation streams | Full sample values including RGB under zero alpha, 16-bit channels, Adam7 passes, animation frame rectangles, timing and disposal. Metadata-bearing chunks constrain permissible layout changes. |
| JPEG | Huffman tables, progressive scan bands and successive refinements of existing coefficients | Visible quantized DCT blocks, quantizers, sampling geometry, components, decoded image and supported APP/COM markers. Unknown vendor, compound and signed layouts are kept. |
| GIF | Lossless LZW re-encoding and dictionary reset choices | Each frame’s palette indices, extensions, transparency, animation timing, disposal and loop data. |
| WebP | Lossless VP8L candidates, including lossless subframes inside an animation | Decoded RGBA for every changed frame, including RGB beneath transparency. Existing lossy VP8 image data is retained. |
| BMP | Palette or true-color layouts and minimum-byte RLE packet selection on supported indexed rows | Sample colors, resolution, supported header fields and orientation. |
| TIFF | Supported classic strip-based images, multiple directories, horizontal prediction, LZW, PackBits, DEFLATE and strip regrouping | Sample data before color conversion, byte order and recognized tags. Opaque offsets, unhandled compression and private layouts constrain editing. |
| QOI | Re-encoding pixel opcodes | Decoded samples, dimensions, channel count and color-space flag. |
| TGA | Exact row-packet selection for supported uncompressed and RLE images | Pixel samples, orientation, image ID and palette. Private or offset-dependent extensions constrain editing. |
| PBM, PGM, PPM, PNM | Plain Netpbm to the corresponding binary raster where the header can be retained; 8-bit and 16-bit samples as appropriate | Dimensions, range, individual samples and supported comments. |
| PSD, PSB | Composite-image compression through raw data, PackBits, ZIP, or ZIP with prediction | All bytes preceding the composite, including layers, masks and profiles. Composite samples retain their original bit depth. |
| OpenEXR | Supported single-part scanline images. Raw, RLE, ZIPS and ZIP inputs can be written as ZIP | Channel bits, including NaN payloads and signed zero for floating point. Header bytes outside the compression mode are preserved. Tiled, deep, multipart, subsampled and other unhandled layouts keep their input. |
| ICO, CUR, ICNS | PNG resources embedded in the icon family | Resource dimensions and pixel layout, icon directory entries, cursor hotspots and supported tables of contents. Legacy bitmap resources are retained. |
| SVG, SVGZ | XML tag punctuation in UTF-8 SVG; verified DEFLATE of the original XML stream in SVGZ | SVG text and attribute values, inline scripts and styles, comments, namespaces and parsed tree. SVGZ also keeps the original XML bytes and gzip fields. No path approximation or numeric rounding. |
| ORA, KRA | DEFLATE streams in supported OpenRaster and Krita ZIP members | Every uncompressed member byte, ordering, names, compression methods, comments, supported attributes and CRCs. Files are processed in memory without extracting paths onto disk. |
| HEIC, HEIF, AVIF | Box headers, local item-location tables and explicit zero-filled padding in supported still images | Every media byte, every item extent and supported property payloads. Sequences, external data references, unknown properties, provenance boxes and opaque metadata retain their layout. Codec payloads stay bit-identical. |
| JPEG XL | Adjacent codestream fragments, redundant wrapping and eligible Brotli metadata boxes | The concatenated JPEG XL codestream and original uncompressed metadata bytes. Frame indexes, unsupported brands, opaque and signed boxes retain their layout. A bare codestream is passed through. |

Standalone files are identified from their contents. Directory scans select the registered extensions. The input limit is 256 MiB per file. Individual codecs also limit expanded samples, dimensions, frames, chunks or member counts. The app reports malformed or over-limit input and preserves the source. An unsupported variant with a known conservative pass-through can report zero savings.

## Optional lossy compression

Lossy compression is a separate setting. Turning up the search effort alone always retains the default lossless policy. With the option enabled, the original and the best verified lossless encoding remain candidates. A lossy candidate must also pass the format-specific checks and be strictly smaller. The format stays the same. The CLI reports whether a selected candidate used the lossy path in the JSON field `lossy_used`.

| Eligible input | Additional search when explicitly enabled |
| --- | --- |
| Static, noninterlaced 8-bit PNG | Fits a smaller palette, checks decoded colour errors, and preserves every alpha byte and hidden pixel colour. Existing indexed images, high bit depths, APNG and unsupported metadata stay on the lossless path. |
| Supported JPEG | Tests bounded changes to high-frequency quantized coefficients while retaining low-frequency detail. Decoded error checks apply globally and to local blocks. |
| Supported GIF | Merges close palette colours and checks rendered animation frames, transparency and timing. |
| Supported lossless WebP | Reduces RGB precision within a checked error bound. Alpha samples, RGB under zero alpha, and supported metadata are retained. Existing VP8 lossy payloads are preserved. |

The numerical checks limit changes, and the image can still look different. Other formats continue to use their lossless path. [PNG quality measurements](../bench/lossy/PNG.md) · [JPEG, GIF and WebP quality measurements](../bench/lossy/formats.md)

## Saving and restoring

- **Separate copies** creates an output directory and allocates new names for collisions in the graphical app. The command line refuses a collision.
- **Replace with backup** stages and verifies the new bytes, prepares a separate local copy of the original and records SHA-256 fingerprints before the atomic replacement. The hidden directory is named `.pixminima-originals`. Restore compares the active file with its recorded contents. A later content edit is protected.
- **Optimize and replace** is the graphical app’s default. Starting the batch authorizes replacement of its selected files without a backup. Each replacement requires a smaller verified result. The staged file retains the source modification time on supported filesystems. The selected saving preference persists across batches.
- On macOS, staging retains ordinary extended attributes, resource forks, file permissions and access-control lists. On APFS, cloning explicitly copies the source access-control list. A deliberate backup or copy of a file on another device still serves a different purpose than the adjacent undo directory.

The optional reversible JPEG-to-JPEG-XL archive build changes the file format and can reconstruct the original JPEG byte for byte. Its dependency and platform scope are documented separately. [Archive backend](../crates/archive-jxl/README.md)

## Technical references

The preservation checks follow the structure of the source formats. [PNG specification](https://www.w3.org/TR/png-3/) · [WebP container](https://developers.google.com/speed/webp/docs/riff_container) · [Adobe Photoshop file formats](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/) · [OpenEXR layout](https://openexr.com/en/latest/OpenEXRFileLayout.html) · [AVIF specification](https://aomediacodec.github.io/av1-avif/) · [JPEG XL format](https://github.com/libjxl/libjxl/blob/main/doc/format_overview.md)
