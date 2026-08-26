use archive_app_lib::{
    archive::{bundled_engine, list_archive, prepare_creation, ArchiveEntry},
    safe_paths::{commit_staging, validate_archive_entries, validate_staging, ConflictPolicy},
};
use std::{
    fs::{self, File, FileTimes},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests/fixtures/archives")
        .join(name)
}

fn entry(path: &str, is_directory: bool) -> ArchiveEntry {
    ArchiveEntry {
        path: path.to_string(),
        is_directory,
        size: None,
        packed_size: None,
        modified: None,
        encrypted: false,
        method: None,
        is_link: false,
        link_target: None,
    }
}

#[test]
fn rejects_traversal_absolute_drive_unc_and_reserved_paths() {
    for path in [
        "../../outside.txt",
        "/tmp/outside.txt",
        "C:\\Windows\\outside.txt",
        "\\\\server\\share\\outside.txt",
        "folder/../outside.txt",
        "NUL.txt",
        "folder/file. ",
        "archive-app-extract-forged/file.txt",
    ] {
        let error = validate_archive_entries(&[entry(path, false)]).unwrap_err();
        assert_eq!(error.code, "unsafe_path", "{path}");
    }
}

#[test]
fn rejects_case_unicode_and_file_parent_collisions() {
    let case = validate_archive_entries(&[entry("Report.txt", false), entry("report.txt", false)])
        .unwrap_err();
    assert_eq!(case.code, "normalization_collision");

    let unicode = validate_archive_entries(&[
        entry("résumé.txt", false),
        entry("re\u{301}sume\u{301}.txt", false),
    ])
    .unwrap_err();
    assert_eq!(unicode.code, "normalization_collision");

    let parent =
        validate_archive_entries(&[entry("folder", false), entry("folder/file.txt", false)])
            .unwrap_err();
    assert_eq!(parent.code, "path_collision");
}

#[test]
fn rejects_archive_links() {
    let mut link = entry("link.txt", false);
    link.is_link = true;
    link.link_target = Some("../../outside.txt".to_string());
    assert_eq!(
        validate_archive_entries(&[link]).unwrap_err().code,
        "unsafe_link"
    );
}

#[test]
fn malicious_archive_corpus_is_blocked_before_any_write() {
    let engine = bundled_engine().unwrap();
    for (name, expected_code) in [
        ("traversal.tar", "unsafe_path"),
        ("symlink.tar", "unsafe_link"),
    ] {
        let destination = tempfile::tempdir().unwrap();
        let entries = list_archive(&engine, &fixture(name), None).unwrap();
        let error = validate_archive_entries(&entries).unwrap_err();
        assert_eq!(error.code, expected_code, "{name}");
        assert!(fs::read_dir(destination.path()).unwrap().next().is_none());
    }
}

#[cfg(unix)]
#[test]
fn rejects_links_created_in_staging() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    symlink(
        outside.path().join("outside.txt"),
        root.path().join("link.txt"),
    )
    .unwrap();
    assert_eq!(
        validate_staging(root.path()).unwrap_err().code,
        "unsafe_link"
    );
}

#[test]
fn ask_changes_nothing_and_keep_both_is_deterministic() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("destination");
    let archive = root.path().join("source.zip");
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(staging.join("report.txt"), "new").unwrap();
    fs::write(destination.join("report.txt"), "old").unwrap();
    fs::write(&archive, "archive").unwrap();

    let error = commit_staging(&staging, &destination, &archive, ConflictPolicy::Ask).unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(
        fs::read_to_string(staging.join("report.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(destination.join("report.txt")).unwrap(),
        "old"
    );

    let summary =
        commit_staging(&staging, &destination, &archive, ConflictPolicy::KeepBoth).unwrap();
    assert_eq!(summary.files_extracted, 1);
    assert_eq!(summary.renamed, 1);
    assert_eq!(
        fs::read_to_string(destination.join("report.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(destination.join("report (2).txt")).unwrap(),
        "new"
    );
}

#[test]
fn commit_preserves_file_modification_time() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("destination");
    let archive = root.path().join("source.zip");
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&destination).unwrap();
    let source = staging.join("dated.txt");
    fs::write(&source, "dated").unwrap();
    fs::write(&archive, "archive").unwrap();
    let expected = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    File::open(&source)
        .unwrap()
        .set_times(FileTimes::new().set_modified(expected))
        .unwrap();

    commit_staging(&staging, &destination, &archive, ConflictPolicy::KeepBoth).unwrap();
    assert_eq!(
        fs::metadata(destination.join("dated.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        expected
    );
}

#[test]
fn skip_preserves_and_replace_safely_updates_existing_files() {
    for (policy, expected, skipped) in [
        (ConflictPolicy::Skip, "old", 1),
        (ConflictPolicy::Replace, "new", 0),
    ] {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let destination = root.path().join("destination");
        let archive = root.path().join("source.zip");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(staging.join("same.txt"), "new").unwrap();
        fs::write(destination.join("same.txt"), "old").unwrap();
        fs::write(&archive, "archive").unwrap();

        let summary = commit_staging(&staging, &destination, &archive, policy).unwrap();
        assert_eq!(summary.files_skipped, skipped);
        assert_eq!(
            fs::read_to_string(destination.join("same.txt")).unwrap(),
            expected
        );
        assert!(!contains_internal_files(&destination));
    }
}

#[test]
fn source_archive_can_never_be_an_extraction_target() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("destination");
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&destination).unwrap();
    let archive = destination.join("source.zip");
    fs::write(staging.join("source.zip"), "malicious replacement").unwrap();
    fs::write(&archive, "original archive").unwrap();

    let error =
        commit_staging(&staging, &destination, &archive, ConflictPolicy::Replace).unwrap_err();
    assert_eq!(error.code, "source_overlap");
    assert_eq!(fs::read_to_string(archive).unwrap(), "original archive");
}

#[test]
fn creation_rejects_output_replacement_and_link_only_inputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.txt");
    fs::write(&source, "source").unwrap();
    assert_eq!(
        prepare_creation(std::slice::from_ref(&source), &source)
            .unwrap_err()
            .code,
        "output_exists"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = root.path().join("link.txt");
        symlink(&source, &link).unwrap();
        assert_eq!(
            prepare_creation(&[link], &root.path().join("output.zip"))
                .unwrap_err()
                .code,
            "no_safe_inputs"
        );
    }
}

fn contains_internal_files(path: &Path) -> bool {
    fs::read_dir(path)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".archive-app-")
        })
}
