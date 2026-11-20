use super::{ComputeMode, FileReport, MAX_INPUT_BYTES, OptimizeOptions, optimize_with_report};
use sha2_const_stable::Sha256;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const BACKUPS: &str = ".pixminima-originals";
const LOCK: &str = "transaction";
const ORIGINAL: &str = "original";
const JOURNAL: &str = "digests";

fn folder(input: &Path) -> Result<PathBuf, String> {
    let parent = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .join(BACKUPS)
        .join(input.file_name().ok_or("Missing input filename")?))
}

fn check_directory(path: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("Backup directory is not a regular directory".into());
    }
    Ok(())
}
fn read_regular(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_file() || meta.len() > MAX_INPUT_BYTES {
        return Err("Expected a regular file within the size limit".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("File grew beyond the size limit".into());
    }
    Ok(bytes)
}
fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::new().update(data).finalize()
}
fn system_error(e: std::io::Error) -> String {
    e.to_string()
}

pub(super) struct StagedImage {
    // Keep an ordinary file in a unique private directory, so native publishing
    // operations retain its attributes on every platform.
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl StagedImage {
    fn path(&self) -> &Path {
        &self.path
    }
    fn replace(self, target: &Path) -> Result<(), String> {
        // A link may have been added while the encoder was running.
        require_single_link(&File::open(target).map_err(system_error)?)?;
        fs::rename(self.path(), target).map_err(system_error)
    }
    pub(super) fn publish_copy(self, target: &Path) -> Result<(), String> {
        #[cfg(windows)]
        renamest::rename_exclusive(self.path(), target).map_err(system_error)?;
        #[cfg(not(windows))]
        tempfile::TempPath::try_from_path(self.path())
            .map_err(system_error)?
            .persist_noclobber(target)
            .map_err(|e| e.error.to_string())?;
        Ok(())
    }
}

fn copy_with_attributes(input: &Path, output: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use rustix::fs::{CWD, CloneFlags, fclonefileat};
        // sys/clonefile.h defines CLONE_ACL as 0x0004. A default APFS clone
        // inherits the destination ACL instead of retaining the source ACL.
        let copy_acl = CloneFlags::from_bits_retain(0x0004);
        let source = File::open(input).map_err(system_error)?;
        if fclonefileat(&source, CWD, output, copy_acl).is_ok() {
            return Ok(());
        }
        // On a different volume or a filesystem without cloning, use the
        // standard library's full-metadata fcopyfile path on an existing file.
        File::create_new(output).map_err(system_error)?;
    }
    fs::copy(input, output).map_err(system_error)?;
    #[cfg(target_os = "linux")]
    copy_linux_attributes(input, output)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_linux_attributes(input: &Path, output: &Path) -> Result<(), String> {
    use rustix::fs::{XattrFlags, fgetxattr, flistxattr, fsetxattr};
    use std::os::unix::ffi::OsStrExt;
    let source = File::open(input).map_err(system_error)?;
    let target = File::open(output).map_err(system_error)?;
    let mut names = vec![0; 65536];
    let count = match flistxattr(&source, names.as_mut_slice()) {
        Ok(n) => n,
        Err(rustix::io::Errno::NOTSUP) => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut contents = vec![0; 65536];
    let mut existing = vec![0; 65536];
    for name in names[..count].split(|b| *b == 0).filter(|n| !n.is_empty()) {
        // Integrity signatures and executable capabilities cannot be applied
        // to a different byte stream merely because its pixels match.
        if matches!(
            name,
            b"security.ima" | b"security.evm" | b"security.capability"
        ) {
            return Err("File has security attributes tied to its original bytes".into());
        }
        let attr = std::ffi::OsStr::from_bytes(name);
        let n = fgetxattr(&source, attr, contents.as_mut_slice()).map_err(|e| e.to_string())?;
        if fgetxattr(&target, attr, existing.as_mut_slice())
            .is_ok_and(|m| m == n && existing[..m] == contents[..n])
        {
            continue;
        }
        fsetxattr(&target, attr, &contents[..n], XattrFlags::empty())
            .map_err(|e| format!("Cannot preserve file attribute {attr:?}: {e}"))?;
    }
    Ok(())
}

fn stage_replacement(input: &Path, after: &[u8]) -> Result<StagedImage, String> {
    let parent = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    stage_copy(input, after, parent)
}

fn private_directory(parent: &Path, prefix: &str) -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent).map_err(system_error)
}

pub(super) fn stage_copy(input: &Path, after: &[u8], parent: &Path) -> Result<StagedImage, String> {
    let input_metadata = fs::metadata(input).map_err(system_error)?;
    let directory = private_directory(parent, ".pixminima-")?;
    let candidate = StagedImage {
        path: directory.path().join("image"),
        _directory: directory,
    };
    // The filesystem copy carries native attributes, resource forks and mode
    // bits where supported. Only the staged data fork receives the new bytes.
    copy_with_attributes(input, candidate.path())?;
    let permissions = fs::metadata(candidate.path())
        .map_err(system_error)?
        .permissions();
    if permissions.readonly() {
        let mut writable = permissions.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            writable.set_mode(writable.mode() | 0o200);
        }
        #[cfg(not(unix))]
        writable.set_readonly(false);
        fs::set_permissions(candidate.path(), writable).map_err(system_error)?;
    }
    let mut staged = OpenOptions::new()
        .write(true)
        .open(candidate.path())
        .map_err(system_error)?;
    staged.set_len(0).map_err(system_error)?;
    staged.rewind().map_err(system_error)?;
    staged.write_all(after).map_err(system_error)?;
    if let Ok(modified) = input_metadata.modified() {
        staged
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .map_err(system_error)?;
    }
    if permissions.readonly() {
        staged.set_permissions(permissions).map_err(system_error)?;
    }
    staged.sync_all().map_err(system_error)?;
    drop(staged);
    if read_regular(candidate.path())? != after {
        return Err("Staged output could not be verified".into());
    }
    Ok(candidate)
}

#[cfg(windows)]
fn windows_lock(input: &Path) -> Result<File, String> {
    // Keeping the handle outside the image and the backup directory permits
    // atomic renames on Windows, even when restoring a missing input.
    let parent = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let normalized = parent
        .canonicalize()
        .map_err(system_error)?
        .join(input.file_name().ok_or("Missing filename")?);
    let key = digest(normalized.to_string_lossy().to_lowercase().as_bytes());
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let directory = std::env::temp_dir().join("PixMinima-locks");
    fs::create_dir_all(&directory).map_err(system_error)?;
    let control = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(hex))
        .map_err(system_error)?;
    control
        .try_lock()
        .map_err(|e| format!("File is busy or cannot be locked: {e}"))?;
    Ok(control)
}

fn require_single_link(file: &File) -> Result<(), String> {
    #[cfg(unix)]
    let links = {
        use std::os::unix::fs::MetadataExt;
        file.metadata().map_err(system_error)?.nlink()
    };
    #[cfg(windows)]
    let links = winapi_util::file::information(file)
        .map_err(system_error)?
        .number_of_links();
    #[cfg(any(unix, windows))]
    if links != 1 {
        return Err("Use copy output for an input with multiple hard links".into());
    }
    #[cfg(not(any(unix, windows)))]
    let _ = file;
    Ok(())
}

fn lock_input(input: &Path) -> Result<File, String> {
    if fs::symlink_metadata(input)
        .map_err(system_error)?
        .permissions()
        .readonly()
    {
        return Err("Read-only input; choose separate copies".into());
    }
    #[cfg(windows)]
    let guard = windows_lock(input)?;
    let file = File::open(input).map_err(system_error)?;
    #[cfg(unix)]
    file.try_lock()
        .map_err(|e| format!("File is busy or cannot be locked: {e}"))?;
    require_single_link(&file)?;
    #[cfg(windows)]
    return Ok(guard);
    #[cfg(not(windows))]
    Ok(file)
}

/// Explicit permanent replacement. Publish the verified temporary file using
/// the same atomic rename as backed-up mode, and create no backup.
pub fn replace_verified(
    input: &Path,
    opts: &OptimizeOptions,
    mode: ComputeMode,
) -> Result<FileReport, String> {
    if folder(input)?.exists() {
        return Err("An earlier backup already exists for this file".into());
    }
    let _guard = lock_input(input)?;
    let before = read_regular(input)?;
    let (report, after) = optimize_with_report(&before, opts, mode)?;
    if after.len() < before.len() {
        let candidate = stage_replacement(input, &after)?;
        if read_regular(input)? != before {
            return Err("Input changed while being processed".into());
        }
        candidate.replace(input)?;
        sync_directory(
            input
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
    }
    Ok(report)
}

pub fn has_backup(input: &Path) -> bool {
    folder(input).is_ok_and(|p| p.join(LOCK).is_file() && p.join(JOURNAL).is_file())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(system_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Prepare a complete independent backup and its checksums before publishing
/// the backup directory, then replace the input with one atomic rename.
pub fn optimize_in_place(
    input: &Path,
    opts: &OptimizeOptions,
    mode: ComputeMode,
) -> Result<FileReport, String> {
    let saved = folder(input)?;
    if saved.try_exists().map_err(system_error)? {
        return Err("An earlier backup already exists for this file".into());
    }
    let _guard = lock_input(input)?;
    let before = read_regular(input)?;
    let (report, after) = optimize_with_report(&before, opts, mode)?;
    if before.len() <= after.len() {
        return Ok(report);
    }
    let candidate = stage_replacement(input, &after)?;
    let backup_parent = saved.parent().unwrap();
    let parent = backup_parent
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match fs::create_dir(backup_parent) {
        Ok(()) => sync_directory(parent)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => check_directory(backup_parent)?,
        Err(e) => return Err(e.to_string()),
    }
    // A crash during preparation can leave a temporary directory, but never a
    // partial named backup and never a hard link that could block future edits.
    let preparing = private_directory(backup_parent, ".preparing-")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(preparing.path().join(LOCK))
        .map_err(system_error)?;
    lock.try_lock().map_err(|e| e.to_string())?;
    let backup = preparing.path().join(ORIGINAL);
    copy_with_attributes(input, &backup)?;
    let copied = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&backup)
        .map_err(system_error)?;
    if let Ok(modified) = fs::metadata(input).and_then(|m| m.modified()) {
        copied
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .map_err(system_error)?;
    }
    copied.sync_all().map_err(system_error)?;
    drop(copied);
    if read_regular(&backup)? != before {
        return Err("Input changed while preparing its backup".into());
    }
    let mut journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(preparing.path().join(JOURNAL))
        .map_err(system_error)?;
    journal
        .write_all(&digest(&before))
        .and_then(|_| journal.write_all(&digest(&after)))
        .and_then(|_| journal.sync_all())
        .map_err(system_error)?;
    drop(journal);
    sync_directory(preparing.path())?;
    if saved.try_exists().map_err(system_error)? || read_regular(input)? != before {
        return Err("Input or backup changed while being processed".into());
    }
    #[cfg(windows)]
    drop(lock);
    fs::rename(preparing.path(), &saved).map_err(system_error)?;
    sync_directory(backup_parent)?;
    if read_regular(input)? != before {
        return Err("Input changed; the verified backup is available".into());
    }
    candidate.replace(input)?;
    sync_directory(parent)?;
    Ok(report)
}

/// Undo only a known transaction. If the user edited the optimized file, both
/// files are retained so the user can choose which version to keep.
pub fn restore_original(input: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let _external = windows_lock(input)?;
    let saved = folder(input)?;
    check_directory(saved.parent().unwrap())?;
    check_directory(&saved)?;
    let mutex = OpenOptions::new()
        .read(true)
        .write(true)
        .open(saved.join(LOCK))
        .map_err(system_error)?;
    mutex
        .try_lock()
        .map_err(|_| "File is currently being processed".to_string())?;
    let journal = read_regular(&saved.join(JOURNAL))?;
    if journal.len() != 64 {
        return Err("Incomplete backup journal; original retained in the backup folder".into());
    }
    let original = saved.join(ORIGINAL);
    let active = match fs::symlink_metadata(input) {
        Ok(m) if m.file_type().is_file() => Some(digest(&read_regular(input)?)),
        Ok(_) => return Err("The original path now contains a different type of file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if active
        .as_ref()
        .is_some_and(|v| v.as_slice() != &journal[32..] && v.as_slice() != &journal[..32])
    {
        return Err(
            "File changed after optimization; original is available in its backup folder".into(),
        );
    }
    if original.try_exists().map_err(system_error)? {
        if digest(&read_regular(&original)?).as_slice() != &journal[..32] {
            return Err("Backup content differs from its recorded checksum".into());
        }
        // Publishing the backup must neither break an existing link set nor
        // turn an independent recovery copy into a linked working file.
        require_single_link(&File::open(&original).map_err(system_error)?)?;
        if active.is_some() {
            require_single_link(&File::open(input).map_err(system_error)?)?;
        }
        fs::rename(&original, input).map_err(system_error)?;
        sync_directory(
            input
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
    } else if active
        .as_ref()
        .is_none_or(|v| v.as_slice() != &journal[..32])
    {
        return Err("Backup file is missing; the current file was kept".into());
    }
    // Once the source has been restored, retire the reservation atomically.
    // A retry between the two renames can finish using the original digest.
    let parent = saved.parent().unwrap();
    let retired = private_directory(parent, ".restored-")?;
    let contents = retired.path().join("transaction");
    #[cfg(windows)]
    drop(mutex);
    fs::rename(&saved, &contents).map_err(system_error)?;
    // Once the directory has moved, cleanup must only ever remove known files.
    let retirement_root = retired.keep();
    sync_directory(parent)?;
    #[cfg(not(windows))]
    drop(mutex);
    for name in [LOCK, JOURNAL, ORIGINAL] {
        match fs::remove_file(contents.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    // Retain any additional files a user placed in their backup directory.
    if fs::read_dir(&contents)
        .map_err(system_error)?
        .next()
        .is_none()
    {
        fs::remove_dir(&contents).map_err(system_error)?;
        fs::remove_dir(&retirement_root).map_err(system_error)?;
    }
    Ok(())
}
