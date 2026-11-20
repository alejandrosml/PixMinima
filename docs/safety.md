# Rust, graphics and the safety boundary

The standard Mac release performs image compression in process. Its format encoders, validators, file workflow, desktop application and Metal host code are Rust. A WGSL compute shader implements five reversible PNG predictors on the GPU. The build, packaging, comparison and verification scripts also use Python and shell.

## Compiled codec graph

The project's crates forbid `unsafe_code`. The audit starts a fresh Cargo build of the four image codec and container crates, removes Cargo's dependency lint cap and passes `-Funsafe-code` to the compiler for their complete active dependency graph. This includes the JPEG, PNG, APNG, GIF, WebP, TIFF, BMP, Photoshop, OpenEXR, icon, container and metadata paths.

```sh
./scripts/audit-codecs.py
```

The audit applies to the locked, active configuration. Rust’s precompiled standard library has its own implementation. Desktop rendering, accessibility, native file panels, filesystem helpers and WGPU's interaction with Metal use platform dependencies which contain unsafe Rust and system interfaces. The application as a whole therefore includes transitive unsafe code.

The local JPEG backend is derived from smallpeg and extends its coefficient-preserving scan planner. Brotli uses a local subset of alloc-no-stdlib. The unused C allocator callbacks and raw-pointer destructor are removed, so the remaining allocator traits can pass the same compiler audit. The Zenzop fork also fixes reuse of incomplete match caches in DEFLATE and prevents a second compression attempt while unwinding. Upstream attribution and license notices accompany all three local forks. [JPEG fork](../crates/formats/vendor/smallpeg/README.md) · [Allocator fork](../vendor/alloc-no-stdlib/README.md) · [Zenzop changes](../vendor/zenzop/PIXMINIMA.md)

## Hardware selection

A batch snapshots available system memory. The scheduler estimates workspace from file size, dimensions, animation and effort, and admits up to eight workers against half the reported available RAM with a 12 GiB ceiling on the estimate. Images whose structure is hard to size receive larger reservations. A single job can still run on a smaller host. Actual memory depends on the codec and the image. [Platform validation](portability.md)

The optional Metal backend scores PNG predictors for packed, noninterlaced RGBA8 rows. It uploads source samples through shared buffers and reads one 32-bit selection per row. The CPU applies the chosen filters, searches DEFLATE encodings and verifies the output. The optimized CPU scorer and the shader use the same rule to resolve equal scores.

Automatic mode learns on the current Mac during a session. Below 2 MiB of decoded RGBA, or with fewer than 64 rows or columns, the CPU handles selection. Larger jobs are grouped by decoded size and aspect ratio. The first two arrivals in a group stay on CPU. On three eligible comparisons the app times both scorers on the same pixels. Two measured GPU wins with at least a 20% time reduction enable Metal for that group. Busy GPU work falls back to CPU. The app periodically resamples long-running groups. These cutoffs bound the cost of calibration. They do not define a universal number of PNGs at which a GPU becomes faster. [Benchmarks](../bench/metal/)

## Separate archival feature

The optional `archive-jxl` feature uses a different JPEG XL encoder and decoder, with dependencies containing unsafe code. The standard signed app omits that feature. Existing JPEG XL containers can still be optimized by the standard engine. [Archive backend](../crates/archive-jxl/README.md)
