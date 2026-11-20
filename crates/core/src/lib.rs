#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
#[cfg(feature = "archive-jxl")]
use std::io::Write;
use std::path::{Path, PathBuf};

mod batch;
pub use batch::{BatchJob, BatchQueue};
mod recovery;
pub use pixminima_compute::ComputeMode;
#[cfg(all(target_os = "macos", feature = "metal"))]
pub use pixminima_compute::share_metal_device;
pub use recovery::{has_backup, optimize_in_place, replace_verified, restore_original};

/// Large compressed inputs are refused before reading or allocating them in full.
pub const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;
pub const EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "ico", "cur", "svg", "svgz",
    "ora", "kra", "tif", "tiff", "qoi", "tga", "targa", "pbm", "pgm", "ppm", "pnm", "psd", "psb",
    "exr", "avif", "heic", "heif", "icns", "jxl",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Effort {
    #[default]
    Standard,
    Thorough,
    Maximum,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OptimizeOptions {
    pub effort: Effort,
    /// Permit eligible encoders to change image samples. Defaults to false.
    pub lossy: bool,
}

#[derive(Clone, Debug)]
pub struct FileReport {
    pub before: u64,
    pub after: u64,
    pub format: String,
    /// True only when the selected candidate came from a lossy encoder.
    pub lossy_used: bool,
}

/// Returns the format, verified result, and never a payload larger than the source.
pub fn optimize_bytes(
    input: &[u8],
    options: &OptimizeOptions,
) -> Result<(&'static str, Vec<u8>), String> {
    optimize_bytes_with_compute(input, options, ComputeMode::Auto)
}

pub fn optimize_bytes_with_compute(
    input: &[u8],
    options: &OptimizeOptions,
    compute: ComputeMode,
) -> Result<(&'static str, Vec<u8>), String> {
    let (kind, bytes, _) = optimize_candidate(input, options, compute)?;
    Ok((kind, bytes))
}

/// Optimize once and report whether the winning encoding used lossy processing.
pub fn optimize_with_report(
    input: &[u8],
    options: &OptimizeOptions,
    compute: ComputeMode,
) -> Result<(FileReport, Vec<u8>), String> {
    let (kind, bytes, lossy_used) = optimize_candidate(input, options, compute)?;
    let report = FileReport {
        before: input.len() as u64,
        after: bytes.len() as u64,
        format: kind.into(),
        lossy_used,
    };
    Ok((report, bytes))
}

fn optimize_candidate(
    input: &[u8],
    options: &OptimizeOptions,
    compute: ComputeMode,
) -> Result<(&'static str, Vec<u8>, bool), String> {
    if input.len() as u64 > MAX_INPUT_BYTES {
        return Err("File exceeds the 256 MiB input limit".into());
    }
    let (kind, mut bytes) = optimize_round(input, options.effort, compute)?;
    // Further search can exploit a previously changed palette, scan or strip layout.
    if options.effort == Effort::Maximum
        && bytes.len() < input.len()
        && matches!(kind, "JPEG" | "WebP" | "GIF" | "TIFF")
    {
        let candidate = optimize_round(&bytes, Effort::Thorough, compute);
        if let Ok((_, further)) = candidate
            && further.len() < bytes.len()
        {
            bytes = further;
        }
    }
    if options.lossy {
        // Give each lossy encoder the original samples and metadata. It can
        // decline when its quality or format checks do not hold on this file.
        let thorough = options.effort != Effort::Standard;
        let proposal = match kind {
            "PNG" => pixminima_png::optimize_lossy(input, thorough),
            "JPEG" | "GIF" | "WebP" => pixminima_formats::optimize_lossy(input, thorough),
            _ => Ok(None),
        };
        if let Ok(Some(mut candidate)) = proposal
            && !candidate.is_empty()
        {
            // Reuse the full representation search on the already quality-checked
            // candidate. This pass must keep its newly chosen samples unchanged.
            if options.effort == Effort::Maximum
                && let Ok((_, smaller)) = optimize_round(&candidate, Effort::Maximum, compute)
                && smaller.len() < candidate.len()
            {
                candidate = smaller;
            }
            if candidate.len() < bytes.len() {
                return Ok((kind, candidate, true));
            }
        }
    }
    Ok((kind, bytes, false))
}

fn optimize_round(
    input: &[u8],
    effort: Effort,
    compute: ComputeMode,
) -> Result<(&'static str, Vec<u8>), String> {
    let thorough = effort != Effort::Standard;
    let (kind, bytes) = if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        let select =
            |bytes: &[u8], w: usize, h: usize| pixminima_compute::choose_rows(bytes, w, h, compute);
        let optimized = if effort == Effort::Maximum {
            pixminima_png::optimize_maximum_with_rows(input, Some(&select))?
        } else {
            pixminima_png::optimize_with_rows(input, thorough, Some(&select))?
        };
        ("PNG", optimized)
    } else if let Some(kind) = pixminima_formats::detect(input) {
        let bytes = if effort == Effort::Maximum {
            pixminima_formats::optimize_maximum(input)?
        } else {
            pixminima_formats::optimize(input, thorough)?
        };
        (kind, bytes)
    } else if let Some(kind) = pixminima_extra::detect(input) {
        (kind, pixminima_extra::optimize(input, thorough)?)
    } else {
        pixminima_containers::optimize(input, thorough)?
    };
    if bytes.is_empty() {
        return Err(format!("{kind} optimizer produced an empty file"));
    }
    Ok((
        kind,
        if bytes.len() < input.len() {
            bytes
        } else {
            input.to_vec()
        },
    ))
}

/// Expand folders in deterministic order, skipping symlinks and output folders
/// generated by the desktop app. Explicit files can be detected by signature.
pub fn collect_files(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    collect_files_with_filter(inputs, has_supported_extension)
}

pub fn collect_files_with_filter(
    inputs: &[PathBuf],
    accepts: impl Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, String> {
    let mut results = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut pending: Vec<_> = inputs.iter().map(|p| (p.to_owned(), true)).collect();
    while let Some((path, explicit)) = pending.pop() {
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if !explicit
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(is_output_folder)
            {
                continue;
            }
            let canonical = path
                .canonicalize()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if directories.insert(canonical) {
                let entries =
                    fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                for entry in entries {
                    pending.push((entry.map_err(|e| e.to_string())?.path(), false));
                }
            }
        } else if meta.is_file() && (explicit || accepts(&path)) {
            results.insert(
                path.canonicalize()
                    .map_err(|e| format!("{}: {e}", path.display()))?,
            );
        }
    }
    Ok(results.into_iter().collect())
}

/// Conservative collision key for case-insensitive and normalizing output volumes.
pub fn output_name_key(path: &Path) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.to_string_lossy()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

pub fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| EXTENSIONS.iter().any(|ext| s.eq_ignore_ascii_case(ext)))
}

fn is_output_folder(name: &str) -> bool {
    name == ".pixminima-originals"
        || name == "PixMinima Output"
        || name
            .strip_prefix("PixMinima Output ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Creates an independent output atomically and refuses to replace any existing
/// file. A failed decode, validation or final write leaves the source untouched.
pub fn optimize_file(
    input: &Path,
    output: &Path,
    options: &OptimizeOptions,
) -> Result<FileReport, String> {
    optimize_file_with_compute(input, output, options, ComputeMode::Auto)
}

pub fn optimize_file_with_compute(
    input: &Path,
    output: &Path,
    options: &OptimizeOptions,
    compute: ComputeMode,
) -> Result<FileReport, String> {
    if input == output {
        return Err("Choose a new output path; the source file is preserved".into());
    }
    let stat = fs::symlink_metadata(input).map_err(|e| format!("{}: {e}", input.display()))?;
    if !stat.file_type().is_file() {
        return Err(format!("{} is not a regular file", input.display()));
    }
    if stat.len() > MAX_INPUT_BYTES {
        return Err("File exceeds the 256 MiB input limit".into());
    }
    if output.try_exists().map_err(|e| e.to_string())? {
        return Err(format!("Output already exists: {}", output.display()));
    }
    let mut original = Vec::new();
    File::open(input)
        .map_err(|e| format!("{}: {e}", input.display()))?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut original)
        .map_err(|e| e.to_string())?;
    let (report, result) = optimize_with_report(&original, options, compute)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let temp = recovery::stage_copy(input, &result, parent)?;
    temp.publish_copy(output)?;
    Ok(report)
}

#[cfg(feature = "archive-jxl")]
fn write_new(output: &Path, result: &[u8]) -> Result<(), String> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    tmp.write_all(result).map_err(|e| e.to_string())?;
    tmp.as_file().sync_all().map_err(|e| e.to_string())?;
    tmp.persist_noclobber(output)
        .map_err(|e| format!("{}: {}", output.display(), e.error))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveDirection {
    Archive,
    Restore,
}

pub const JXL_ARCHIVE_AVAILABLE: bool = cfg!(feature = "archive-jxl");

pub fn convert_jpeg_archive(
    input: &Path,
    output: &Path,
    direction: ArchiveDirection,
) -> Result<FileReport, String> {
    #[cfg(not(feature = "archive-jxl"))]
    {
        let _ = (input, output, direction);
        Err("JPEG XL archival is available in the optional archive-jxl build".into())
    }
    #[cfg(feature = "archive-jxl")]
    {
        let meta = fs::symlink_metadata(input).map_err(|e| e.to_string())?;
        if !meta.file_type().is_file() || meta.len() > MAX_INPUT_BYTES {
            return Err("Input is not a regular file within the size limit".into());
        }
        if output.try_exists().map_err(|e| e.to_string())? {
            return Err(format!("Output already exists: {}", output.display()));
        }
        let mut source = Vec::new();
        File::open(input)
            .map_err(|e| e.to_string())?
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut source)
            .map_err(|e| e.to_string())?;
        if source.len() as u64 > MAX_INPUT_BYTES {
            return Err("Input grew beyond the size limit".into());
        }
        let (kind, result) = match direction {
            ArchiveDirection::Archive => {
                let data = pixminima_jxl::archive_jpeg(&source)?;
                if data.len() >= source.len() {
                    return Err("The reversible JPEG XL archive would not be smaller".into());
                }
                ("JPEG XL", data)
            }
            ArchiveDirection::Restore => ("JPEG", pixminima_jxl::restore_jpeg(&source)?),
        };
        write_new(output, &result)?;
        Ok(FileReport {
            before: source.len() as u64,
            after: result.len() as u64,
            format: kind.into(),
            lossy_used: false,
        })
    }
}
