use pixminima_core::{
    BatchQueue, ComputeMode, Effort, FileReport, OptimizeOptions, collect_files_with_filter,
    optimize_file_with_compute,
};
use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Task {
    #[default]
    Optimize,
    #[cfg(feature = "archive-jxl")]
    Archive,
    #[cfg(feature = "archive-jxl")]
    RestoreJpeg,
}

impl Task {
    fn output_name(self, path: &Path) -> PathBuf {
        match self {
            Self::Optimize => path.to_owned(),
            #[cfg(feature = "archive-jxl")]
            Self::Archive => path.with_extension("jxl"),
            #[cfg(feature = "archive-jxl")]
            Self::RestoreJpeg => path.with_extension("jpg"),
        }
    }

    fn accepts(self, path: &Path) -> bool {
        let _ = path;
        match self {
            Self::Optimize => true,
            #[cfg(feature = "archive-jxl")]
            Self::Archive => extension_matches(path, &["jpg", "jpeg", "jpe", "jfif"]),
            #[cfg(feature = "archive-jxl")]
            Self::RestoreJpeg => extension_matches(path, &["jxl"]),
        }
    }
}

#[cfg(feature = "archive-jxl")]
fn extension_matches(path: &Path, extensions: &[&str]) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    })
}

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub enum WriteMode {
    Copies,
    Backup,
    #[default]
    Overwrite,
}

impl WriteMode {
    pub fn stored(self) -> &'static str {
        match self {
            Self::Copies => "copies",
            Self::Backup => "backup",
            Self::Overwrite => "overwrite",
        }
    }
    pub fn from_saved(mode: Option<&str>, old_replace: Option<&str>) -> Self {
        match mode {
            Some("copies") => Self::Copies,
            Some("backup") => Self::Backup,
            Some("overwrite") => Self::Overwrite,
            _ if old_replace == Some("false") => Self::Copies,
            _ if old_replace == Some("true") => Self::Backup,
            _ => Self::Overwrite,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Options {
    pub effort: Effort,
    pub lossy: bool,
    pub compute: ComputeMode,
    pub mode: WriteMode,
    pub task: Task,
}

pub enum Event {
    Ready {
        sources: Vec<PathBuf>,
        destination: Option<PathBuf>,
    },
    Started(PathBuf),
    File {
        source: PathBuf,
        result: Result<(FileReport, PathBuf), String>,
    },
    Skipped(PathBuf),
    Failed(String),
    Empty,
    Finished {
        cancelled: bool,
    },
}

fn numbered(name: &OsStr, number: usize, is_dir: bool) -> OsString {
    if number == 1 {
        return name.to_owned();
    }
    let path = Path::new(name);
    let extension = if is_dir { None } else { path.extension() };
    let mut out = if extension.is_some() {
        path.file_stem().unwrap_or(name).to_owned()
    } else {
        name.to_owned()
    };
    out.push(format!(" {number}"));
    if let Some(ext) = extension {
        out.push(".");
        out.push(ext);
    }
    out
}

// Creating each root directory atomically also handles concurrent app instances.
fn new_directory(parent: &Path, name: &OsStr) -> Result<PathBuf, String> {
    for counter in 1.. {
        let path = parent.join(numbered(name, counter, true));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    unreachable!()
}

fn discover(
    input: &[PathBuf],
    emit: &impl Fn(Event),
    cancel: &AtomicBool,
    task: Task,
) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut unique = BTreeMap::new();
    for original in input {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let meta = match fs::symlink_metadata(original) {
            Ok(meta) => meta,
            Err(e) => {
                emit(Event::File {
                    source: original.clone(),
                    result: Err(e.to_string()),
                });
                continue;
            }
        };
        if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
            emit(Event::Skipped(original.clone()));
            continue;
        }
        match original.canonicalize() {
            Ok(path) => {
                unique.insert(path, meta.is_dir());
            }
            Err(e) => emit(Event::File {
                source: original.clone(),
                result: Err(e.to_string()),
            }),
        }
    }
    let mut covered = Vec::<PathBuf>::new();
    let mut groups = Vec::new();
    for (path, directory) in unique {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        if covered.iter().any(|parent| path.starts_with(parent)) {
            continue;
        }
        let eligible = |p: &Path| {
            if task == Task::Optimize {
                pixminima_core::has_supported_extension(p)
            } else {
                task.accepts(p)
            }
        };
        match collect_files_with_filter(std::slice::from_ref(&path), eligible) {
            Ok(files) => {
                if files.is_empty() {
                    emit(Event::Skipped(path));
                    continue;
                }
                if directory {
                    covered.push(path.clone());
                }
                groups.push((path, files));
            }
            Err(detail) => emit(Event::File {
                source: path,
                result: Err(detail),
            }),
        }
    }
    groups
}

fn destinations(
    groups: Vec<(PathBuf, Vec<PathBuf>)>,
    output: &Path,
    task: Task,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut queue = Vec::new();
    let mut names = HashSet::new();
    for (root, files) in groups {
        let directory = root.is_dir();
        let source_name = root.file_name().unwrap_or_else(|| OsStr::new("Images"));
        let directory_root = if directory {
            let folder = reserve_name(output, source_name, true, &mut names)?;
            fs::create_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            Some(folder)
        } else {
            None
        };
        let mut directories = BTreeMap::new();
        for file in files {
            let candidate = if let Some(folder) = &directory_root {
                let relative = file.strip_prefix(&root).map_err(|e| e.to_string())?;
                let mut parent = folder.clone();
                let mut original = PathBuf::new();
                for component in relative.parent().unwrap_or(Path::new("")).components() {
                    original.push(component);
                    parent = if let Some(existing) = directories.get(&original) {
                        PathBuf::clone(existing)
                    } else {
                        // A changed extension may collide with an input subfolder name.
                        let unique =
                            reserve_name(&parent, component.as_os_str(), true, &mut names)?;
                        directories.insert(original.clone(), unique.clone());
                        unique
                    };
                }
                task.output_name(&parent.join(relative.file_name().ok_or("No input filename")?))
            } else {
                task.output_name(&output.join(source_name))
            };
            let parent = candidate.parent().ok_or("No output directory")?;
            let name = candidate.file_name().ok_or("No output filename")?;
            let target = reserve_name(parent, name, false, &mut names)?;
            queue.push((file, target));
        }
    }
    Ok(queue)
}

// Reserve every final filename before starting parallel workers. macOS commonly
// treats canonically equivalent and differently capitalized paths as identical.
fn reserve_name(
    parent: &Path,
    name: &OsStr,
    directory: bool,
    names: &mut HashSet<String>,
) -> Result<PathBuf, String> {
    for number in 1.. {
        let candidate = parent.join(numbered(name, number, directory));
        let normalized = pixminima_core::output_name_key(&candidate);
        if names.insert(normalized) && !candidate.try_exists().map_err(|e| e.to_string())? {
            return Ok(candidate);
        }
    }
    unreachable!()
}

pub fn process(
    input: &[PathBuf],
    parent: Option<&Path>,
    options: Options,
    cancel: &AtomicBool,
    sender: &Sender<Event>,
) {
    let emit = |event| {
        let _ = sender.send(event);
    };
    let groups = discover(input, &emit, cancel, options.task);
    if cancel.load(Ordering::Acquire) {
        emit(Event::Finished { cancelled: true });
        return;
    }
    if groups.is_empty() {
        emit(Event::Empty);
        return;
    }
    let (jobs, folder) = if options.mode != WriteMode::Copies && options.task == Task::Optimize {
        (
            groups
                .into_iter()
                .flat_map(|(_, files)| files.into_iter().map(|p| (p.clone(), p)))
                .collect(),
            None,
        )
    } else {
        let suggested = input.iter().find_map(|p| {
            p.canonicalize()
                .ok()
                .and_then(|p| p.parent().map(Path::to_owned))
        });
        let Some(folder) = parent.or(suggested.as_deref()) else {
            emit(Event::Failed("No output directory".into()));
            return;
        };
        let dir = match new_directory(folder, OsStr::new("PixMinima Output")) {
            Ok(dir) => dir,
            Err(e) => {
                emit(Event::Failed(e));
                return;
            }
        };
        match destinations(groups, &dir, options.task) {
            Ok(jobs) => (jobs, Some(dir)),
            Err(e) => {
                emit(Event::Failed(e));
                return;
            }
        }
    };
    emit(Event::Ready {
        sources: jobs.iter().map(|(path, _)| path.clone()).collect(),
        destination: folder,
    });
    let queue = BatchQueue::with_effort(
        jobs.iter().map(|(source, _)| source.as_path()),
        options.effort,
    );
    std::thread::scope(|scope| {
        for _ in 0..queue.worker_count() {
            scope.spawn(|| {
                while !cancel.load(Ordering::Acquire) {
                    let Some(lease) = queue.next() else { break };
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let (source, dest) = &jobs[lease.index()];
                    emit(Event::Started(source.clone()));
                    let profile = OptimizeOptions {
                        effort: options.effort,
                        lossy: options.lossy,
                    };
                    let result = match options.task {
                        Task::Optimize if options.mode == WriteMode::Overwrite => {
                            pixminima_core::replace_verified(source, &profile, options.compute)
                        }
                        Task::Optimize if options.mode == WriteMode::Backup => {
                            pixminima_core::optimize_in_place(source, &profile, options.compute)
                        }
                        Task::Optimize => {
                            optimize_file_with_compute(source, dest, &profile, options.compute)
                        }
                        #[cfg(feature = "archive-jxl")]
                        Task::Archive => pixminima_core::convert_jpeg_archive(
                            source,
                            dest,
                            pixminima_core::ArchiveDirection::Archive,
                        ),
                        #[cfg(feature = "archive-jxl")]
                        Task::RestoreJpeg => pixminima_core::convert_jpeg_archive(
                            source,
                            dest,
                            pixminima_core::ArchiveDirection::Restore,
                        ),
                    }
                    .map(|report| (report, dest.clone()));
                    emit(Event::File {
                        source: source.clone(),
                        result,
                    });
                }
            });
        }
    });
    emit(Event::Finished {
        cancelled: cancel.load(Ordering::Acquire),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "pixminima-gui-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn collisions_and_subfolders_are_preserved() {
        let workspace = Temp::new();
        let a = workspace.0.join("a");
        let b = workspace.0.join("b");
        let c = workspace.0.join("c");
        for dir in [&a, &b, &c] {
            fs::create_dir(dir).unwrap();
        }
        let p = a.join("icon.png");
        let q = b.join("icon.png");
        fs::write(&p, b"data").unwrap();
        fs::write(&q, b"data").unwrap();
        let jobs = destinations(
            vec![
                (p.clone(), vec![p]),
                (q.clone(), vec![q]),
                (c.clone(), vec![c.join("nested").join("other.png")]),
            ],
            &workspace.0,
            Task::Optimize,
        )
        .unwrap();
        assert_ne!(jobs[0].1, jobs[1].1);
        assert!(jobs[1].1.ends_with("icon 2.png"));
        assert!(jobs[2].1.ends_with("c 2/nested/other.png"));
    }
    #[test]
    fn cancel_before_scan_leaves_no_output() {
        let workspace = Temp::new();
        let (tx, rx) = std::sync::mpsc::channel();
        process(
            std::slice::from_ref(&workspace.0),
            Some(&workspace.0),
            Options {
                effort: Effort::Thorough,
                lossy: false,
                compute: ComputeMode::Cpu,
                mode: WriteMode::Copies,
                task: Task::Optimize,
            },
            &AtomicBool::new(true),
            &tx,
        );
        assert!(matches!(
            rx.recv().unwrap(),
            Event::Finished { cancelled: true }
        ));
        assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
    }
    #[test]
    fn replacing_and_restoring_respects_edits_and_preserves_the_original() {
        let workspace = Temp::new();
        let input = workspace.0.join("flat.png");
        let original = include_bytes!("../tests/fixtures/flat.png");
        fs::write(&input, original).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        process(
            std::slice::from_ref(&input),
            Some(&workspace.0),
            Options {
                effort: Effort::Standard,
                lossy: false,
                compute: ComputeMode::Cpu,
                mode: WriteMode::Backup,
                task: Task::Optimize,
            },
            &AtomicBool::new(false),
            &tx,
        );
        let report = rx
            .try_iter()
            .find_map(|event| match event {
                Event::File { result, .. } => Some(result),
                _ => None,
            })
            .expect("a file report")
            .expect("successful safe replacement");
        assert_eq!(report.1, input.canonicalize().unwrap());
        assert!(report.0.after < report.0.before);
        assert!(pixminima_core::has_backup(&input));
        let optimized = fs::read(&input).unwrap();
        fs::write(&input, b"later edits").unwrap();
        assert!(pixminima_core::restore_original(&input).is_err());
        assert_eq!(fs::read(&input).unwrap(), b"later edits");
        fs::write(&input, optimized).unwrap();
        pixminima_core::restore_original(&input).unwrap();
        assert_eq!(fs::read(&input).unwrap(), original);
        assert!(!pixminima_core::has_backup(&input));
        assert!(!workspace.0.join("PixMinima Output").exists());
    }
    #[cfg(feature = "archive-jxl")]
    #[test]
    fn archive_outputs_with_equivalent_stems_are_distinct_and_restoration_finds_jxl() {
        let workspace = Temp::new();
        let src = workspace.0.join("photos");
        fs::create_dir(&src).unwrap();
        let a = src.join("café.jpg");
        let b = src.join("cafe\u{301}.jpeg");
        let output = workspace.0.join("out");
        fs::create_dir(&output).unwrap();
        let jobs = destinations(vec![(src.clone(), vec![a, b])], &output, Task::Archive).unwrap();
        assert!(jobs[0].1.ends_with("photos/café.jxl"));
        assert!(jobs[1].1.ends_with("photos/cafe\u{301} 2.jxl"));
        let nested = destinations(
            vec![(
                src.clone(),
                vec![src.join("name.jpg"), src.join("name.jxl/nested.jpg")],
            )],
            &output,
            Task::Archive,
        )
        .unwrap();
        assert!(!nested[1].1.starts_with(&nested[0].1));
        assert!(nested[1].1.ends_with("name.jxl 2/nested.jxl"));
        fs::write(src.join("FILE.JXL"), b"placeholder").unwrap();
        let groups = discover(&[src], &|_| {}, &AtomicBool::new(false), Task::RestoreJpeg);
        assert_eq!(groups[0].1.len(), 1);
        assert!(groups[0].1[0].ends_with("FILE.JXL"));
    }
    #[test]
    fn subsequent_jobs_cannot_replace_the_first_output() {
        let workspace = Temp::new();
        let a = new_directory(&workspace.0, OsStr::new("PixMinima Output")).unwrap();
        let b = new_directory(&workspace.0, OsStr::new("PixMinima Output")).unwrap();
        assert_ne!(a, b);
        assert!(b.ends_with("PixMinima Output 2"));
    }
}
