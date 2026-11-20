#![forbid(unsafe_code)]

use pixminima_core::{
    ComputeMode, Effort, FileReport, MAX_INPUT_BYTES, OptimizeOptions, collect_files,
    optimize_file_with_compute, optimize_in_place, optimize_with_report, restore_original,
};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "PixMinima — image optimization (lossless by default)

  pixminima [--thorough] [--json] --output FILE IMAGE
  pixminima [--thorough] [--json] --output-dir DIR FILE_OR_FOLDER ...
  pixminima [--thorough] [--json] --measure FILE_OR_FOLDER ...
  pixminima [--thorough] [--json] --in-place FILE_OR_FOLDER ...
  pixminima [--maximum] --overwrite FILE_OR_FOLDER ...
  pixminima --restore FILE ...
  pixminima archive-jpeg SOURCE.jpg NEW.jxl       (optional archive-jxl build)
  pixminima restore-jpeg ARCHIVE.jxl NEW.jpg      (optional archive-jxl build)

--measure        Check compression without writing files
--thorough       Try additional, slower compression strategies
--maximum        Search further for the smallest verified result
--lossy          Allow high-quality lossy compression on supported images
--compute MODE   auto (default), cpu, or metal for PNG predictor scoring
--output, -o     Create a single new file
--output-dir     Create copies and preserve nested directories
--in-place       Optimize in place and retain the original for undo
--overwrite      Permanently replace each input with its verified smaller output
--restore        Restore an original saved by --in-place
--json           Print one JSON object per file
--               End options (for paths beginning with a dash)

Copy modes keep existing destination files. Failed or unsupported files produce a nonzero exit status.";

enum Output {
    Measure,
    File(PathBuf),
    Directory(PathBuf),
    InPlace,
    Overwrite,
    Restore,
}
struct Args {
    inputs: Vec<PathBuf>,
    output: Output,
    json: bool,
    options: OptimizeOptions,
    compute: ComputeMode,
}

fn parse(mut iter: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut options = OptimizeOptions::default();
    let mut compute = ComputeMode::Auto;
    let mut json = false;
    let mut positional = false;
    while let Some(arg) = iter.next() {
        // Reserved for literal paths after an option terminator.
        if positional {
            inputs.push(PathBuf::from(arg));
            continue;
        }
        match arg.to_str() {
            Some("--") => positional = true,
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(None);
            }
            Some("-V" | "--version") => {
                println!("PixMinima {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some("--json") => json = true,
            Some("--lossy") => options.lossy = true,
            Some("--thorough") => options.effort = Effort::Thorough,
            Some("--maximum" | "--max") => options.effort = Effort::Maximum,
            Some("--compute") => {
                compute = match iter.next().as_deref().and_then(|v| v.to_str()) {
                    Some("auto") => ComputeMode::Auto,
                    Some("cpu") => ComputeMode::Cpu,
                    Some("metal") if cfg!(all(target_os = "macos", feature = "metal")) => {
                        ComputeMode::Metal
                    }
                    Some("metal") => return Err("This build has no Metal backend".into()),
                    _ => return Err("Use --compute auto, cpu or metal".into()),
                }
            }
            Some(
                "--measure" | "--in-place" | "--overwrite" | "--restore" | "-o" | "--output"
                | "--output-dir",
            ) => {
                if output.is_some() {
                    return Err("Choose a single output mode".into());
                }
                output = Some(if arg == "--measure" {
                    Output::Measure
                } else if arg == "--in-place" {
                    Output::InPlace
                } else if arg == "--overwrite" {
                    Output::Overwrite
                } else if arg == "--restore" {
                    Output::Restore
                } else {
                    let path = iter.next().ok_or("Missing output path")?.into();
                    if arg == "--output-dir" {
                        Output::Directory(path)
                    } else {
                        Output::File(path)
                    }
                });
            }
            Some(s) if s.starts_with('-') => return Err(format!("Unknown option {s}")),
            _ => inputs.push(arg.into()),
        }
    }
    if inputs.is_empty() {
        return Err(HELP.into());
    }
    Ok(Some(Args {
        inputs,
        output: output.unwrap_or(Output::Measure),
        options,
        json,
        compute,
    }))
}

fn relative_output(path: &Path, roots: &[PathBuf]) -> PathBuf {
    // Single folder inputs place their contents directly in the chosen folder.
    for root in roots {
        if let Ok(relative) = path.strip_prefix(root)
            && !relative.as_os_str().is_empty()
        {
            if roots.len() == 1 {
                return relative.to_owned();
            }
            if let Some(name) = root.file_name() {
                return Path::new(name).join(relative);
            }
            return relative.to_owned();
        }
    }
    path.file_name().map(PathBuf::from).unwrap_or_default()
}

fn execute(args: Args) -> Result<bool, String> {
    if matches!(args.output, Output::Restore) {
        let mut success = true;
        for path in &args.inputs {
            match restore_original(path) {
                Ok(()) if args.json => println!(
                    "{}",
                    serde_json::json!({"input":path.to_string_lossy(),"restored":true})
                ),
                Ok(()) => println!("Restored {}", path.display()),
                Err(e) => {
                    success = false;
                    if args.json {
                        println!(
                            "{}",
                            serde_json::json!({"input":path.to_string_lossy(),"error":e})
                        );
                    } else {
                        eprintln!("{}: {e}", path.display());
                    }
                }
            }
        }
        return Ok(success);
    }
    let files = collect_files(&args.inputs)?;
    if files.is_empty() {
        return Err("No supported files in the selection".into());
    }
    if matches!(args.output, Output::File(_)) && files.len() != 1 {
        return Err("--output expects one file; use --output-dir for batches".into());
    }
    let roots: Vec<_> = args
        .inputs
        .iter()
        .map(|p| p.canonicalize())
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let mut destinations = HashMap::new();
    if let Output::Directory(dir) = &args.output {
        let existing = dir.canonicalize().ok();
        for path in &files {
            if existing.as_ref().is_some_and(|p| path.starts_with(p)) {
                return Err("The input includes files from the chosen output directory".into());
            }
            let target = dir.join(relative_output(path, &roots));
            if destinations
                .insert(pixminima_core::output_name_key(&target), path.clone())
                .is_some()
            {
                return Err(format!(
                    "Two inputs would write {}. Choose distinct output roots.",
                    target.display()
                ));
            }
        }
    }
    let queue = pixminima_core::BatchQueue::with_effort(
        files.iter().map(PathBuf::as_path),
        args.options.effort,
    );
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..queue.worker_count() {
            let send = send.clone();
            let files = &files;
            let queue = &queue;
            let args = &args;
            let roots = &roots;
            scope.spawn(move || {
                while let Some(job) = queue.next() {
                    let index = job.index();
                    let path = &files[index];
                    let (destination, result) = process(args, path, roots);
                    if send.send((index, destination, result)).is_err() {
                        break;
                    }
                }
            });
        }
    });
    drop(send);
    let mut records: Vec<_> = recv.into_iter().collect();
    records.sort_by_key(|record| record.0);
    let mut success = true;
    for (index, destination, result) in records {
        let path = &files[index];
        match result {
            Ok(FileReport {
                format: kind,
                before,
                after,
                lossy_used,
            }) => {
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({"input":path.to_string_lossy(), "output":destination.as_ref().map(|p|p.to_string_lossy()), "format":kind, "before":before, "after":after,"saved":before-after,"lossy_used":lossy_used})
                    );
                } else {
                    let percent = if before == 0 {
                        0.0
                    } else {
                        (before - after) as f64 * 100.0 / before as f64
                    };
                    let mode = if lossy_used { " · lossy" } else { "" };
                    println!(
                        "{}  {before} → {after} B  ({percent:.1}% saved{mode})",
                        path.display()
                    );
                }
            }
            Err(e) => {
                success = false;
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({"input":path.to_string_lossy(),"error":e})
                    );
                } else {
                    eprintln!("{}: {e}", path.display());
                }
            }
        }
    }
    Ok(success)
}

type Outcome = Result<FileReport, String>;
fn process(args: &Args, path: &Path, roots: &[PathBuf]) -> (Option<PathBuf>, Outcome) {
    let destination = match &args.output {
        Output::File(p) => Some(p.clone()),
        Output::Directory(p) => Some(p.join(relative_output(path, roots))),
        Output::InPlace | Output::Overwrite => Some(path.to_owned()),
        Output::Measure => None,
        Output::Restore => unreachable!(),
    };
    let result = if matches!(args.output, Output::Overwrite) {
        pixminima_core::replace_verified(path, &args.options, args.compute)
    } else if matches!(args.output, Output::InPlace) {
        optimize_in_place(path, &args.options, args.compute)
    } else if let Some(out) = &destination {
        optimize_file_with_compute(path, out, &args.options, args.compute)
    } else {
        (|| {
            let file = fs::File::open(path).map_err(|e| e.to_string())?;
            if file.metadata().map_err(|e| e.to_string())?.len() > MAX_INPUT_BYTES {
                return Err("Input exceeds size limit".into());
            }
            let mut buf = Vec::new();
            file.take(MAX_INPUT_BYTES + 1)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            let (report, _) = optimize_with_report(&buf, &args.options, args.compute)?;
            Ok(report)
        })()
    };
    (destination, result)
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let status = match args.first().and_then(|v| v.to_str()) {
        Some(cmd @ ("archive-jpeg" | "restore-jpeg")) => {
            if let [_, input, output] = args.as_slice() {
                let direction = if cmd == "archive-jpeg" {
                    pixminima_core::ArchiveDirection::Archive
                } else {
                    pixminima_core::ArchiveDirection::Restore
                };
                pixminima_core::convert_jpeg_archive(Path::new(input), Path::new(output), direction)
                    .map(|r| {
                        println!(
                            "{}  {} → {} B",
                            Path::new(output).display(),
                            r.before,
                            r.after
                        );
                        true
                    })
            } else {
                Err(format!("Usage: pixminima {cmd} INPUT NEW-OUTPUT"))
            }
        }
        _ => parse(args.into_iter()).and_then(|a| a.map_or(Ok(true), execute)),
    };
    match status {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
