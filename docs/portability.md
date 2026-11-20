# Platform validation

Checks performed on 20 November 2026 with Rust 1.100.0 and the committed dependency lockfile. The distributed app targets Apple silicon.

| Target | Build and execution |
| --- | --- |
| macOS 27.2.1 Golden Gate, Apple M1 Max | Native macOS build. All ten file workflow tests passed on the development Mac, including the native ACL test. |
| Linux x86-64, musl | Cross-linked the static CLI and its file workflow tests. Executed both in a diskless QEMU guest running Alpine's Linux 6.12.115 kernel with a tmpfs volume. All nine workflow tests passed. The desktop crate and its UI dependencies also passed a Linux target check. |
| Windows x86-64 | Cross-linked the CLI and the desktop app as PE32+ executables with LLVM-MinGW. The desktop dependency tree and application also passed a Microsoft ABI target check. The Windows workflow test executable ran under Wine 10.7 in an isolated Linux guest. All eight applicable tests passed, including the hard-link and recovery cases. Native Windows desktop interaction and testing on NTFS remain to be qualified. |

On both guest systems the CLI also compressed the tagged RGB `coffee.png` crop from the optional palette validation set. It produced the same 13,400 bytes as the M1 Max from a 77,994-byte source. A repeat run used lossless compression and retained the same size. The Mac reference was decoded and checked with libpng and Pillow. [PNG quality measurements and input preparation](../bench/lossy/PNG.md)

## Files and recovery

The Linux guest ran the CLI against a filename containing an accented letter, Chinese characters and an emoji, then against a filename containing an invalid UTF-8 byte. Separate copies and restoration used the original OS filenames. Human-readable output and JSON may display a replacement character for a filename that cannot be represented as Unicode.

The guest verified a named POSIX access-control entry and a user-defined extended attribute after a separate copy, an in-place replacement and restoration. It also exercised refusal to overwrite an existing destination, refusal to restore over an independent edit, restoration of a missing image, and cleanup after an interruption between restoring the original and retiring its backup directory.

The workflow tests create hard links to a source, then to the optimized image and to its stored original. A separate output remains available. Replacement and restoration check the link count before renaming either image. Callers receive an error while the linked file and backup remain available. On Windows this check reads the open file handle through the safe interface of `winapi-util`. The crate calls Win32 internally. PixMinima’s separate unsafe-code audit covers the active image codec dependency graph.

## Reproducing the checks

Run `cargo test --locked -p pixminima-core --test workflow` on a volume that supports hard links. Run `cargo build --locked -p pixminima` for the CLI and `cargo build --locked -p pixminima-desktop` for the desktop app. The repository's CI configuration includes macOS, Linux and Windows runners.

The standard build uses Rust 2024. The locked Windows rename dependency declares Rust 1.98 and the desktop framework declares Rust 1.95. The compilation checks above used Rust 1.100.0. GPU throughput and image-size comparisons need separate measurements on each target.
