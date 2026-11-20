use pixminima_core::{
    ComputeMode, OptimizeOptions, collect_files, has_backup, optimize_bytes, optimize_file,
    optimize_in_place, restore_original,
};
use std::fs;

const SVG: &[u8] = br#"<?xml version="1.0"?>
<svg    xmlns = "http://www.w3.org/2000/svg" viewBox ="0 0 20 20" >
  <!-- a = b --> <text   id = 'a'   > x   y </text   >
  <script><![CDATA[ const a = "< test >"; ]]></script>
</svg   >"#;

#[test]
fn vector_changes_only_tag_punctuation_and_preserves_script() {
    let (kind, result) = optimize_bytes(SVG, &OptimizeOptions::default()).unwrap();
    assert_eq!(kind, "SVG");
    assert!(result.len() < SVG.len());
    let source = std::str::from_utf8(&result).unwrap();
    assert!(source.contains(" x   y "));
    assert!(source.contains("<!-- a = b -->"));
    assert!(source.contains("<![CDATA[ const a = \"< test >\"; ]]>"));
    assert_eq!(
        optimize_bytes(&result, &OptimizeOptions::default())
            .unwrap()
            .1,
        result
    );
}

#[test]
fn copy_output_is_atomic_and_never_overwrites_an_existing_destination() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("original.SVG");
    let out = temp.path().join("copies").join("small.svg");
    fs::write(&src, SVG).unwrap();
    let res = optimize_file(&src, &out, &OptimizeOptions::default()).unwrap();
    assert!(res.before > res.after);
    assert_eq!(fs::read(&src).unwrap(), SVG);
    fs::write(&out, b"existing content").unwrap();
    assert!(optimize_file(&src, &out, &OptimizeOptions::default()).is_err());
    assert_eq!(fs::read(&out).unwrap(), b"existing content");
    let bad = temp.path().join("bad.png");
    fs::write(&bad, b"\x89PNG\r\n\x1a\n").unwrap();
    let path = temp.path().join("invalid-result.png");
    assert!(optimize_file(&bad, &path, &OptimizeOptions::default()).is_err());
    assert!(!path.exists());
}

#[test]
fn inplace_preserves_original_and_refuses_to_overwrite_later_edits() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("image.svg");
    fs::write(&src, SVG).unwrap();
    let r = optimize_in_place(&src, &OptimizeOptions::default(), ComputeMode::Cpu).unwrap();
    assert!(r.after < r.before && has_backup(&src));
    let optimized = fs::read(&src).unwrap();
    fs::write(&src, b"new independent edit").unwrap();
    assert!(restore_original(&src).is_err());
    assert!(has_backup(&src));
    fs::write(&src, &optimized).unwrap();
    restore_original(&src).unwrap();
    assert!(!has_backup(&src));
    assert_eq!(fs::read(&src).unwrap(), SVG);
    optimize_in_place(&src, &OptimizeOptions::default(), ComputeMode::Cpu).unwrap();
    fs::remove_file(&src).unwrap();
    restore_original(&src).unwrap();
    assert_eq!(fs::read(&src).unwrap(), SVG);
}

#[test]
fn walking_directories_skips_nested_output_and_deduplicates_files() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("a.svg");
    fs::write(&input, SVG).unwrap();
    for folder in [
        "PixMinima Output",
        "PixMinima Output 2",
        ".pixminima-originals",
    ] {
        let dir = root.path().join(folder);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("file.svg"), SVG).unwrap();
    }
    assert_eq!(
        collect_files(&[root.path().to_owned(), input.clone()]).unwrap(),
        vec![input.canonicalize().unwrap()]
    );
}

#[test]
fn concurrent_copy_outputs_have_one_winner() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("a.svg");
    let dest = root.path().join("b.svg");
    fs::write(&input, SVG).unwrap();
    let winners = std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for _ in 0..4 {
            jobs.push(
                scope.spawn(|| optimize_file(&input, &dest, &OptimizeOptions::default()).is_ok()),
            );
        }
        jobs.into_iter()
            .filter_map(|j| j.join().ok())
            .filter(|v| *v)
            .count()
    });
    assert_eq!(winners, 1);
    assert!(dest.metadata().unwrap().len() < SVG.len() as u64);
}

#[test]
fn permanent_replacement_publishes_only_a_successful_smaller_file() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("a.svg");
    fs::write(&path, SVG).unwrap();
    let historical = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(historical))
        .unwrap();
    let r = pixminima_core::replace_verified(&path, &OptimizeOptions::default(), ComputeMode::Cpu)
        .unwrap();
    assert!(r.after < r.before);
    assert!(!has_backup(&path));
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), historical);
    assert_eq!(
        fs::read(&path).unwrap(),
        optimize_bytes(SVG, &OptimizeOptions::default()).unwrap().1
    );
    fs::write(&path, b"unsupported data").unwrap();
    assert!(
        pixminima_core::replace_verified(&path, &OptimizeOptions::default(), ComputeMode::Auto)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"unsupported data");
}

#[test]
fn restore_can_finish_after_an_interruption_after_the_atomic_rename() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("image.svg");
    fs::write(&path, SVG).unwrap();
    optimize_in_place(&path, &OptimizeOptions::default(), ComputeMode::Cpu).unwrap();
    let saved = t.path().join(".pixminima-originals/image.svg");
    fs::rename(saved.join("original"), &path).unwrap();
    restore_original(&path).unwrap();
    assert!(!saved.exists());
    assert_eq!(fs::read(&path).unwrap(), SVG);
    optimize_in_place(&path, &OptimizeOptions::default(), ComputeMode::Cpu).unwrap();
    assert!(has_backup(&path));
}

#[cfg(unix)]
#[test]
fn read_only_input_can_produce_a_copy_with_matching_mode_and_date() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("source.svg");
    let output = dir.path().join("copy.svg");
    fs::write(&input, SVG).unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    fs::File::open(&input)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o444)).unwrap();
    let report = optimize_file(&input, &output, &OptimizeOptions::default()).unwrap();
    assert!(report.after < report.before);
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o444
    );
    assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), old);
    assert!(
        pixminima_core::replace_verified(&input, &OptimizeOptions::default(), ComputeMode::Cpu)
            .is_err()
    );
    assert_eq!(fs::read(&input).unwrap(), SVG);
}

#[cfg(any(unix, windows))]
#[test]
fn hard_links_leave_working_files_and_recovery_copies_intact() {
    use pixminima_core::replace_verified;
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.svg");
    let alias = temp.path().join("alias.svg");
    let copy = temp.path().join("independent.svg");
    let backup = temp.path().join(".pixminima-originals/source.svg/original");
    let opts = OptimizeOptions::default();
    fs::write(&input, SVG).unwrap();
    fs::hard_link(&input, &alias).unwrap();
    assert!(optimize_in_place(&input, &opts, ComputeMode::Cpu).is_err());
    assert!(replace_verified(&input, &opts, ComputeMode::Cpu).is_err());
    assert!(optimize_file(&input, &copy, &opts).is_ok());
    assert_eq!(fs::read(&alias).unwrap(), SVG);
    fs::remove_file(&alias).unwrap();
    optimize_in_place(&input, &opts, ComputeMode::Cpu).unwrap();
    fs::hard_link(&input, &alias).unwrap();
    assert!(restore_original(&input).is_err());
    assert_eq!(fs::read(&alias).unwrap(), fs::read(&copy).unwrap());
    assert_eq!(fs::read(&backup).unwrap(), SVG);
    fs::remove_file(&alias).unwrap();
    fs::hard_link(&backup, &alias).unwrap();
    assert!(restore_original(&input).is_err());
    assert_eq!(fs::read(&input).unwrap(), fs::read(&copy).unwrap());
    fs::remove_file(&alias).unwrap();
    restore_original(&input).unwrap();
    assert_eq!(fs::read(&input).unwrap(), SVG);
}

#[cfg(target_os = "macos")]
#[test]
fn mac_access_control_entries_survive_copy_replacement_and_restore() {
    use std::path::Path;
    use std::process::Command;
    fn entries(path: &Path) -> Vec<String> {
        let result = Command::new("/bin/ls")
            .env("LC_ALL", "C")
            .arg("-led")
            .arg(path)
            .output()
            .unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect()
    }
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("image.svg");
    let out = temp.path().join("smaller.svg");
    fs::write(&src, SVG).unwrap();
    let status = Command::new("/bin/chmod")
        .args(["+a", "group:everyone allow readattr,readextattr"])
        .arg(&src)
        .status()
        .unwrap();
    assert!(status.success());
    let expected = entries(&src);
    assert!(!expected.is_empty());
    let opts = OptimizeOptions::default();
    optimize_file(&src, &out, &opts).unwrap();
    assert_eq!(entries(&out), expected);
    optimize_in_place(&src, &opts, ComputeMode::Cpu).unwrap();
    assert_eq!(entries(&src), expected);
    restore_original(&src).unwrap();
    assert_eq!(entries(&src), expected);
    pixminima_core::replace_verified(&src, &opts, ComputeMode::Cpu).unwrap();
    assert_eq!(entries(&src), expected);
}
