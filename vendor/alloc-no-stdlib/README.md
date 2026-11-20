# Safe allocator subset

This directory is derived from alloc-no-stdlib 2.0.4, by Daniel Reiter Horn under BSD-3-Clause. PixMinima retains the safe slice traits, stack allocator, and indexing macros used by the standard Rust Brotli encoder and decoder. The unused C allocation callbacks, manual destructor and global-pool construction macros have been removed. The crate forbids unsafe Rust.

The empty compatibility feature satisfies Cargo feature resolution. It introduces no allocation mode. The bundled product builds Brotli with the standard-library allocator.

[Upstream release](https://crates.io/crates/alloc-no-stdlib/2.0.4)
