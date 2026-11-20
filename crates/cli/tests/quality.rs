use serde_json::Value;
use std::{fs, path::Path, process::Command};

const JPEG: &[u8] = include_bytes!("../../formats/tests/fixtures/requantize.jpg");

fn invoke(path: &Path, flags: &[&str]) -> Value {
    let run = Command::new(env!("CARGO_BIN_EXE_pixminima"))
        .args(["--compute", "cpu", "--json"])
        .args(flags)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    serde_json::from_slice(&run.stdout).unwrap()
}

#[test]
fn lossy_is_explicit_and_optional_backups_restore_original_jpeg_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("image.jpg");
    fs::write(&file, JPEG).unwrap();
    let strict = invoke(&file, &["--measure"]);
    assert_eq!(strict["lossy_used"], false);
    let optional = invoke(&file, &["--measure", "--lossy"]);
    assert_eq!(optional["lossy_used"], true);
    assert!(optional["after"].as_u64().unwrap() < strict["after"].as_u64().unwrap());
    assert_eq!(fs::read(&file).unwrap(), JPEG);
    let saved = invoke(&file, &["--lossy", "--in-place"]);
    assert_eq!(saved["after"], optional["after"]);
    assert_eq!(saved["lossy_used"], true);
    assert!(invoke(&file, &["--restore"])["restored"].as_bool().unwrap());
    assert_eq!(fs::read(&file).unwrap(), JPEG);
    invoke(&file, &["--overwrite", "--lossy"]);
    let once = fs::read(&file).unwrap();
    invoke(&file, &["--overwrite", "--lossy"]);
    assert_eq!(fs::read(&file).unwrap(), once);
}

#[test]
fn enabling_lossy_leaves_lossless_only_formats_on_their_normal_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("diagram.svg");
    fs::write(
        &file,
        br#"<svg    xmlns = "http://www.w3.org/2000/svg" ><path d = "M0 0L8 8" /></svg>"#,
    )
    .unwrap();
    let strict = invoke(&file, &["--measure", "--maximum"]);
    let optional = invoke(&file, &["--measure", "--maximum", "--lossy"]);
    assert_eq!(strict, optional);
    assert_eq!(optional["lossy_used"], false);
}
