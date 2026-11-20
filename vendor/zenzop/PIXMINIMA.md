# PixMinima changes to zenzop 0.4.2

Based on the published Apache-2.0 crate, upstream revision `df24f3282c14150a06c38c2f0802104773cc444d`. The upstream license and contributor notices accompany this copy. [Upstream repository](https://github.com/imazen/zenzop)

Changes made on 20 November 2026 in the following files.

- `src/cache.rs` tracks positions without a recorded match result, including positions skipped by the long-run shortcut. It permits subsequent compression passes to skip dictionary updates when every position that can query the dictionary has sufficient cached information. The final two positions cannot form a minimum-length DEFLATE match.
- `src/deflate.rs` moves the bit writer out of the encoder during a compression call. If that call unwinds, the encoder's destructor sees an empty writer and cannot repeat the operation during unwinding.

PixMinima's PNG tests exercise the common zlib encoder with steady regions and transitions. EXR scanline groups, TIFF strips, PSD ZIP composites and PNG image streams share this encoder.
