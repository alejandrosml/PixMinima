//! Stable macOS releases. GitHub delivers the image; Apple authenticates its publisher.
use rustix::fs::{CWD, FlockOperation, RenameFlags, flock, renameat_with};
use semver::Version;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::{Builder, TempDir};

const ENDPOINT: &str = "https://api.github.com/repos/alejandrosml/PixMinima/releases/latest";
const RELEASES: &str = "https://github.com/alejandrosml/PixMinima/releases/download";
const ASSET: &str = "PixMinima-macOS-arm64.dmg";
const MAX_SIZE: u64 = 256 * 1024 * 1024;
const SIGNER: &str = "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"4FQA525DB9\"";
const APP_ID: &str = "io.github.alejandrosml.PixMinima";
type UpdateResult<T> = Result<T, String>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    pub version: String,
    digest: String,
    size: u64,
}

#[derive(Debug)]
pub struct Download {
    pub version: String,
    directory: TempDir,
    release: Release,
}

impl Download {
    pub fn matches(&self, release: &Release) -> bool {
        self.release == *release
    }
}

fn run(command: &mut Command) -> UpdateResult<Vec<u8>> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "{} ({})",
            Path::new(command.get_program()).display(),
            output.status
        ))
    }
}

fn curl() -> Command {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args([
        "-q",
        "--fail",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--location",
        "--max-redirs",
        "4",
        "--connect-timeout",
        "10",
        "--user-agent",
        "PixMinima-Updater",
    ]);
    cmd
}

fn stable(value: &str) -> UpdateResult<Version> {
    let v = Version::parse(value).map_err(|e| e.to_string())?;
    if !v.pre.is_empty() || !v.build.is_empty() {
        return Err("Expected a stable version".into());
    }
    Ok(v)
}

fn available(raw: &[u8], installed: &str) -> UpdateResult<Option<Release>> {
    let document: Value = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
    if document["draft"].as_bool() != Some(false) || document["prerelease"].as_bool() != Some(false)
    {
        return Err("Expected a published stable release".into());
    }
    let version = document["tag_name"]
        .as_str()
        .and_then(|tag| tag.strip_prefix('v'))
        .ok_or("Missing release tag")?;
    if stable(version)? <= stable(installed)? {
        return Ok(None);
    }
    let asset = document["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(ASSET) && a["state"].as_str() == Some("uploaded"))
        .ok_or("The Mac download is unavailable")?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Missing SHA-256 digest")?
        .to_ascii_lowercase();
    let size = asset["size"]
        .as_u64()
        .filter(|n| (1..=MAX_SIZE).contains(n))
        .ok_or("Invalid disk-image size")?;
    Ok(Some(Release {
        version: version.into(),
        digest,
        size,
    }))
}

pub fn check() -> UpdateResult<Option<Release>> {
    let bytes = run(curl().args([
        "--max-time",
        "25",
        "--max-filesize",
        "1048576",
        "--header",
        "Accept: application/vnd.github+json",
        ENDPOINT,
    ]))?;
    available(&bytes, env!("CARGO_PKG_VERSION"))
}

fn private_directory(parent: Option<&Path>) -> UpdateResult<TempDir> {
    let mut builder = Builder::new();
    builder
        .prefix(".pixminima-update-")
        .permissions(fs::Permissions::from_mode(0o700));
    match parent {
        Some(p) => builder.tempdir_in(p),
        None => builder.tempdir(),
    }
    .map_err(|e| e.to_string())
}

fn verify_signer(path: &Path, identifier: &str) -> UpdateResult<()> {
    let requirement = format!("=identifier \"{identifier}\" and {SIGNER}");
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--test-requirement"])
        .arg(requirement)
        .arg(path))?;
    Ok(())
}

fn verify_disk(path: &Path, release: &Release) -> UpdateResult<()> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != release.size {
        return Err("Incomplete disk image".into());
    }
    let hash = run(Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path))?;
    if hash.get(..64) != Some(release.digest.as_bytes()) {
        return Err("SHA-256 mismatch".into());
    }
    verify_signer(path, &format!("PixMinima-{}-macOS-arm64", release.version))?;
    run(Command::new("/usr/sbin/spctl")
        .args([
            "--assess",
            "--type",
            "open",
            "--context",
            "context:primary-signature",
        ])
        .arg(path))?;
    Ok(())
}

pub fn download(release: &Release) -> UpdateResult<Download> {
    stable(&release.version)?;
    let directory = private_directory(None)?;
    let path = directory.path().join(ASSET);
    let url = format!("{RELEASES}/v{}/{ASSET}", release.version);
    run(curl()
        .args(["--max-time", "300", "--max-filesize"])
        .arg(release.size.to_string())
        .arg("--output")
        .arg(&path)
        .arg(url))?;
    verify_disk(&path, release)?;
    Ok(Download {
        directory,
        version: release.version.clone(),
        release: release.clone(),
    })
}

struct Mounted(PathBuf);
impl Drop for Mounted {
    fn drop(&mut self) {
        let mut command = Command::new("/usr/bin/hdiutil");
        if run(command.args(["detach", "-quiet"]).arg(&self.0)).is_err() {
            let _ = run(Command::new("/usr/bin/hdiutil")
                .args(["detach", "-quiet", "-force"])
                .arg(&self.0));
        }
    }
}

fn app_version(app: &Path) -> UpdateResult<String> {
    let value = run(Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist")))?;
    String::from_utf8(value)
        .map(|s| s.trim().to_owned())
        .map_err(|e| e.to_string())
}

fn verify_app(app: &Path, expected: &str) -> UpdateResult<()> {
    if !fs::symlink_metadata(app)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err("Invalid application bundle".into());
    }
    verify_signer(app, APP_ID)?;
    if app_version(app)? != expected {
        return Err("Release and application versions differ".into());
    }
    run(Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .arg(app))?;
    run(Command::new("/usr/bin/lipo")
        .args(["-verify_arch", "arm64"])
        .arg(app.join("Contents/MacOS/PixMinima")))?;
    Ok(())
}

fn swap(app: &Path, staged: &Path) -> UpdateResult<()> {
    renameat_with(CWD, app, CWD, staged, RenameFlags::EXCHANGE).map_err(|e| e.to_string())
}

fn install_at(
    download: &Download,
    app: &Path,
    launch: impl FnOnce(&Path) -> UpdateResult<()>,
) -> UpdateResult<()> {
    let parent = app.parent().ok_or("Missing application folder")?;
    // Serialize installers using the stable parent inode across the exchange.
    let parent_fd = fs::File::open(parent).map_err(|e| e.to_string())?;
    flock(&parent_fd, FlockOperation::NonBlockingLockExclusive).map_err(|e| e.to_string())?;
    verify_signer(app, APP_ID)?;
    if stable(&download.version)? <= stable(&app_version(app)?)? {
        return Err("This version is already installed or newer".into());
    }
    let staging = private_directory(Some(parent))?;
    let path = download.directory.path().join(ASSET);
    verify_disk(&path, &download.release)?;
    let mount = download.directory.path().join("volume");
    fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    run(Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-quiet",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(path))?;
    let mounted = Mounted(mount);
    let source = mounted.0.join("PixMinima.app");
    verify_app(&source, &download.version)?;
    let staged = staging.path().join("PixMinima.app");
    run(Command::new("/usr/bin/ditto").arg(source).arg(&staged))?;
    verify_app(&staged, &download.version)?;
    drop(mounted);
    swap(app, &staged)?;
    if let Err(error) = launch(app) {
        if let Err(rollback) = swap(app, &staged) {
            let rescue = staging.keep();
            return Err(format!(
                "{error}. Previous application saved in {} ({rollback})",
                rescue.display()
            ));
        }
        return Err(error);
    }
    Ok(())
}

pub fn install(download: &Download) -> UpdateResult<()> {
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| e.to_string())?;
    if !exe.ends_with("Contents/MacOS/PixMinima") {
        return Err("Open the installed PixMinima application".into());
    }
    let app = exe
        .ancestors()
        .nth(3)
        .ok_or("Cannot locate this application")?;
    if app.extension().is_none_or(|e| e != "app") {
        return Err("Cannot locate this application bundle".into());
    }
    install_at(download, app, |p| {
        run(Command::new("/usr/bin/open").args(["-n", "-a"]).arg(p)).map(|_| ())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release() -> Value {
        json!({"tag_name":"v0.10.0","draft":false,"prerelease":false,"assets":[{
            "name":ASSET, "state":"uploaded", "size": 2000, "digest":format!("sha256:{}", "ab".repeat(32)),
            "browser_download_url":"https://example.invalid/ignored"
        }]})
    }

    #[test]
    fn stable_upgrades_require_an_uploaded_checksummed_arm64_image() {
        let mut doc = release();
        let parse =
            |value: &Value, installed: &str| available(value.to_string().as_bytes(), installed);
        assert_eq!(parse(&doc, "0.9.9").unwrap().unwrap().version, "0.10.0");
        for installed in ["0.10.0", "1.0.0"] {
            assert!(parse(&doc, installed).unwrap().is_none());
        }
        for tag in [
            "v0.10.0-beta.1",
            "v1.1.0+unknown",
            "../app",
            "v0.1.0/anything",
        ] {
            doc["tag_name"] = json!(tag);
            assert!(parse(&doc, "0.1.0").is_err());
        }
        for (field, val) in [
            ("digest", json!("md5:abc")),
            ("size", json!(MAX_SIZE + 1)),
            ("state", json!("new")),
            ("name", json!("Other.dmg")),
        ] {
            doc = release();
            doc["assets"][0][field] = val;
            assert!(parse(&doc, "0.1.0").is_err());
        }
        doc = release();
        doc["draft"] = json!(true);
        assert!(parse(&doc, "0.1.0").is_err());
    }

    #[test]
    fn an_atomic_swap_can_restore_the_original_application() {
        let dir = private_directory(None).unwrap();
        let a = dir.path().join("old.app");
        let b = dir.path().join("new.app");
        for (path, name) in [(&a, "old"), (&b, "new")] {
            fs::create_dir(path).unwrap();
            fs::write(path.join(name), b"contents").unwrap();
        }
        swap(&a, &b).unwrap();
        assert!(a.join("new").is_file() && b.join("old").is_file());
        swap(&a, &b).unwrap();
        assert!(a.join("old").is_file() && b.join("new").is_file());
    }
    /// Uses disposable copies of a local Developer ID-signed older bundle and a
    /// publisher-signed, notarized production disk image. No installed app is touched.
    #[test]
    #[ignore = "requires two signed local release fixtures and macOS disk images"]
    fn signed_disk_upgrade_rollback_and_repeat_install() {
        let disk =
            PathBuf::from(std::env::var_os("PIXMINIMA_QA_SIGNED_DMG").expect("signed test disk"));
        let old = PathBuf::from(
            std::env::var_os("PIXMINIMA_QA_OLD_APP").expect("signed older app fixture"),
        );
        let version = env!("CARGO_PKG_VERSION").to_owned();
        let dir = private_directory(None).unwrap();
        let copy = dir.path().join(ASSET);
        fs::copy(&disk, &copy).unwrap();
        let sha = run(Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&copy))
        .unwrap();
        let mut release = Release {
            version: version.clone(),
            digest: String::from_utf8(sha[..64].to_vec()).unwrap(),
            size: fs::metadata(&disk).unwrap().len(),
        };
        verify_disk(&copy, &release).unwrap();
        let original_digest = release.digest.clone();
        release.digest = "00".repeat(32);
        assert!(verify_disk(&copy, &release).is_err());
        assert!(verify_signer(&copy, APP_ID).is_err());
        release.digest = original_digest;
        let downloaded = Download {
            directory: dir,
            version: version.clone(),
            release: release.clone(),
        };
        assert!(downloaded.matches(&release));
        release.digest = "ab".repeat(32);
        assert!(!downloaded.matches(&release));
        let installed = private_directory(None).unwrap();
        let app = installed.path().join("PixMinima.app");
        run(Command::new("/usr/bin/ditto").arg(&old).arg(&app)).unwrap();
        let old_version = app_version(&app).unwrap();
        assert!(stable(&old_version).unwrap() < stable(&version).unwrap());
        let cached = downloaded.directory.path().join(ASSET);
        fs::write(&cached, b"interrupted download").unwrap();
        let refused = install_at(&downloaded, &app, |_| panic!("must reject a damaged image"));
        assert_eq!(refused.unwrap_err(), "Incomplete disk image");
        assert_eq!(app_version(&app).unwrap(), old_version);
        fs::copy(&disk, &cached).unwrap();
        let fail = install_at(&downloaded, &app, |_| {
            Err("deliberate launch failure".into())
        });
        assert_eq!(fail.unwrap_err(), "deliberate launch failure");
        assert_eq!(app_version(&app).unwrap(), old_version);
        install_at(&downloaded, &app, |app| verify_app(app, &version)).unwrap();
        assert_eq!(app_version(&app).unwrap(), version);
        assert!(
            install_at(&downloaded, &app, |_| panic!(
                "cannot relaunch same version"
            ))
            .is_err()
        );
    }
}
