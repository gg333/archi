use archive_app_lib::archive::{
    bundled_engine, create_archive, extract_archive, extract_entries, list_archive,
    prepare_creation, test_archive, ArchiveFormat, CompressionLevel,
};
use std::{
    fs,
    path::PathBuf,
    process::{self, Command},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests/fixtures/archives")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "archive-app-contract-{name}-{}-{nanos}",
        process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn lists_and_extracts_zip_7z_and_rar_fixtures() {
    let engine = bundled_engine().unwrap();
    for name in ["sample.zip", "sample.7z", "sample.rar"] {
        let archive = fixture(name);
        let destination = scratch(name);
        let entries = list_archive(&engine, &archive, None).unwrap();
        assert!(!entries.is_empty(), "{name} should contain entries");
        test_archive(&engine, &archive, None).unwrap();
        extract_archive(&engine, &archive, &destination, None).unwrap();
        assert!(fs::read_dir(&destination).unwrap().next().is_some());
        fs::remove_dir_all(destination).unwrap();
    }
}

#[test]
fn preserves_unicode_names_and_empty_folders() {
    let entries = list_archive(&bundled_engine().unwrap(), &fixture("sample.7z"), None).unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path == "résumé = नमस्ते.txt"));
    assert!(entries
        .iter()
        .any(|entry| entry.path == "empty" && entry.is_directory));
}

#[test]
fn extracts_only_the_selected_archive_entry() {
    let destination = scratch("selected-entry");
    extract_entries(
        &bundled_engine().unwrap(),
        &fixture("sample.zip"),
        &destination,
        &["hello.txt".to_string()],
        None,
    )
    .unwrap();
    assert!(destination.join("hello.txt").is_file());
    assert!(!destination.join("résumé = नमस्ते.txt").exists());
    fs::remove_dir_all(destination).unwrap();
}

#[test]
fn reports_encrypted_headers_and_damaged_archives_with_typed_errors() {
    let engine = bundled_engine().unwrap();
    let root = scratch("errors");
    let source = root.join("source");
    let encrypted = root.join("encrypted.7z");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("secret.txt"), "secret").unwrap();
    let plan = prepare_creation(&[source.join("secret.txt")], &encrypted).unwrap();
    create_archive(
        &engine,
        &encrypted,
        &plan,
        ArchiveFormat::SevenZip,
        CompressionLevel::Normal,
        None,
        Some("contract-password"),
    )
    .unwrap();

    let password_error = list_archive(&engine, &encrypted, None).unwrap_err();
    assert!(matches!(
        password_error.code.as_str(),
        "password_required" | "wrong_password"
    ));
    assert_eq!(
        list_archive(&engine, &encrypted, Some("contract-password")).unwrap()[0].path,
        "secret.txt"
    );

    let damaged = root.join("damaged.zip");
    let bytes = fs::read(fixture("sample.zip")).unwrap();
    fs::write(&damaged, &bytes[..bytes.len() / 3]).unwrap();
    let damaged_error = list_archive(&engine, &damaged, None).unwrap_err();
    assert!(matches!(
        damaged_error.code.as_str(),
        "damaged_archive" | "invalid_archive" | "engine_failed"
    ));

    let disguised = root.join("wrong-extension.txt");
    fs::copy(fixture("sample.zip"), &disguised).unwrap();
    assert!(!list_archive(&engine, &disguised, None).unwrap().is_empty());
    let fake = root.join("not-an-archive.zip");
    fs::write(&fake, "plain text").unwrap();
    assert_eq!(
        list_archive(&engine, &fake, None).unwrap_err().code,
        "invalid_archive"
    );

    let invalid_method = root.join("invalid-method.zip");
    let mut invalid_method_bytes = bytes.clone();
    for index in 0..invalid_method_bytes.len().saturating_sub(4) {
        if invalid_method_bytes[index..].starts_with(b"PK\x03\x04") {
            invalid_method_bytes[index + 8..index + 10].copy_from_slice(&99u16.to_le_bytes());
        } else if invalid_method_bytes[index..].starts_with(b"PK\x01\x02") {
            invalid_method_bytes[index + 10..index + 12].copy_from_slice(&99u16.to_le_bytes());
        }
    }
    fs::write(&invalid_method, invalid_method_bytes).unwrap();
    // 7-Zip reports a fabricated method ID as damaged data. Its real
    // "Unsupported Method" output is covered by the classifier unit test.
    assert_eq!(
        test_archive(&engine, &invalid_method, None)
            .unwrap_err()
            .code,
        "damaged_archive"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn reads_native_macos_tar_stream_and_ar_formats() {
    let engine = bundled_engine().unwrap();
    let root = scratch("native-formats");
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("hello.txt"), "native compatibility").unwrap();

    for (name, flag) in [
        ("native.tar", "-cf"),
        ("native.tgz", "-czf"),
        ("native.tbz2", "-cjf"),
        ("native.txz", "-cJf"),
    ] {
        assert!(Command::new("/usr/bin/bsdtar")
            .args([
                flag,
                root.join(name).to_str().unwrap(),
                "-C",
                source.to_str().unwrap(),
                "hello.txt"
            ])
            .status()
            .unwrap()
            .success());
    }
    assert!(Command::new("/usr/bin/bsdtar")
        .args([
            "-cf",
            root.join("native.cpio").to_str().unwrap(),
            "--format",
            "cpio",
            "-C",
            source.to_str().unwrap(),
            "hello.txt"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("/usr/bin/ar")
        .args([
            "rcs",
            root.join("native.ar").to_str().unwrap(),
            source.join("hello.txt").to_str().unwrap()
        ])
        .status()
        .unwrap()
        .success());
    for (program, name) in [
        ("/usr/bin/gzip", "native.gz"),
        ("/usr/bin/bzip2", "native.bz2"),
    ] {
        let output = Command::new(program)
            .args(["-c", source.join("hello.txt").to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(root.join(name), output.stdout).unwrap();
    }
    assert!(Command::new(&engine)
        .current_dir(&source)
        .args([
            "a",
            "-txz",
            root.join("native.xz").to_str().unwrap(),
            "hello.txt",
        ])
        .status()
        .unwrap()
        .success());

    for name in [
        "native.tar",
        "native.tgz",
        "native.tbz2",
        "native.txz",
        "native.cpio",
        "native.ar",
        "native.gz",
        "native.bz2",
        "native.xz",
    ] {
        let archive = root.join(name);
        let destination = root.join(format!("extract-{name}"));
        let entries = list_archive(&engine, &archive, None)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert!(!entries.is_empty(), "{name}");
        test_archive(&engine, &archive, None).unwrap();
        extract_archive(&engine, &archive, &destination, None).unwrap();
        assert!(
            fs::read_dir(destination).unwrap().next().is_some(),
            "{name}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn creates_mixed_zip_layout_compatible_with_system_unzip() {
    let engine = bundled_engine().unwrap();
    let root = scratch("create-zip");
    let source = root.join("source");
    let folder = source.join("folder");
    let empty = folder.join("empty");
    let output = root.join("mixed.zip");
    fs::create_dir_all(&empty).unwrap();
    fs::write(source.join("loose.txt"), "loose").unwrap();
    fs::write(folder.join("nested.txt"), "nested").unwrap();
    let plan = prepare_creation(&[source.join("loose.txt"), folder], &output).unwrap();
    create_archive(
        &engine,
        &output,
        &plan,
        ArchiveFormat::Zip,
        CompressionLevel::Normal,
        None,
        None,
    )
    .unwrap();
    test_archive(&engine, &output, None).unwrap();
    let paths = list_archive(&engine, &output, None)
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"loose.txt".to_string()));
    assert!(paths.contains(&"folder/nested.txt".to_string()));
    assert!(paths.contains(&"folder/empty".to_string()));

    if let Ok(status) = std::process::Command::new("unzip")
        .args(["-t", output.to_str().unwrap()])
        .status()
    {
        assert!(
            status.success(),
            "system unzip should accept the created ZIP"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn creates_tarballs_and_single_file_streams() {
    let engine = bundled_engine().unwrap();
    let root = scratch("create-streams");
    let source = root.join("source");
    let file = source.join("hello.txt");
    fs::create_dir_all(&source).unwrap();
    fs::write(&file, "portable stream compatibility").unwrap();

    for (name, format) in [
        ("bundle.tar.gz", ArchiveFormat::TarGzip),
        ("bundle.tar.xz", ArchiveFormat::TarXz),
        ("bundle.tar.zst", ArchiveFormat::TarZstd),
    ] {
        let output = root.join(name);
        let plan = prepare_creation(std::slice::from_ref(&source), &output).unwrap();
        create_archive(
            &engine,
            &output,
            &plan,
            format,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();
        test_archive(&engine, &output, None).unwrap();
        let outer = root.join(format!("outer-{name}"));
        extract_archive(&engine, &output, &outer, None).unwrap();
        let tar = fs::read_dir(&outer)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let extracted = root.join(format!("extracted-{name}"));
        extract_archive(&engine, &tar, &extracted, None).unwrap();
        assert_eq!(
            fs::read_to_string(extracted.join("source/hello.txt")).unwrap(),
            "portable stream compatibility"
        );
    }

    for (name, format) in [
        ("hello.gz", ArchiveFormat::Gzip),
        ("hello.xz", ArchiveFormat::Xz),
        ("hello.zst", ArchiveFormat::Zstd),
    ] {
        let output = root.join(name);
        let plan = prepare_creation(std::slice::from_ref(&file), &output).unwrap();
        create_archive(
            &engine,
            &output,
            &plan,
            format,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();
        test_archive(&engine, &output, None).unwrap();
        let extracted = root.join(format!("extracted-{name}"));
        extract_archive(&engine, &output, &extracted, None).unwrap();
        let restored = fs::read_dir(&extracted)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(restored).unwrap(),
            "portable stream compatibility"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn creation_skips_symbolic_links_without_following_them() {
    use std::os::unix::fs::symlink;

    let engine = bundled_engine().unwrap();
    let root = scratch("create-symlink");
    let source = root.join("source");
    let outside = root.join("outside.txt");
    let output = root.join("safe.zip");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("included.txt"), "included").unwrap();
    fs::write(&outside, "must not follow").unwrap();
    symlink(&outside, source.join("outside-link.txt")).unwrap();

    let plan = prepare_creation(&[source], &output).unwrap();
    assert_eq!(plan.skipped_links, 1);
    create_archive(
        &engine,
        &output,
        &plan,
        ArchiveFormat::Zip,
        CompressionLevel::Normal,
        None,
        None,
    )
    .unwrap();
    let paths = list_archive(&engine, &output, None)
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"source/included.txt".to_string()));
    assert!(!paths.iter().any(|path| path.contains("outside-link")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn creates_encrypted_zip_and_7z_with_typed_password_failures() {
    let engine = bundled_engine().unwrap();
    let root = scratch("create-encrypted");
    let source = root.join("secret.txt");
    fs::write(&source, "secret").unwrap();
    for (name, format) in [
        ("secret.zip", ArchiveFormat::Zip),
        ("secret.7z", ArchiveFormat::SevenZip),
    ] {
        let output = root.join(name);
        let plan = prepare_creation(std::slice::from_ref(&source), &output).unwrap();
        create_archive(
            &engine,
            &output,
            &plan,
            format,
            CompressionLevel::Normal,
            None,
            Some("private-password"),
        )
        .unwrap();
        let error = test_archive(&engine, &output, Some("wrong-password")).unwrap_err();
        assert_eq!(error.code, "wrong_password");
        test_archive(&engine, &output, Some("private-password")).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "Sprint 6 stress gate; run explicitly"]
fn lists_fixture_1000_times() {
    let engine = bundled_engine().unwrap();
    let archive = fixture("sample.zip");
    let started = Instant::now();
    for _ in 0..1000 {
        assert_eq!(list_archive(&engine, &archive, None).unwrap().len(), 3);
    }
    assert!(started.elapsed() < Duration::from_secs(120));
    eprintln!("1,000 archive listings: {:?}", started.elapsed());
}

#[test]
#[ignore = "Sprint 6 reference benchmark; run explicitly"]
fn benchmarks_creation_listing_testing_and_extraction() {
    let engine = bundled_engine().unwrap();
    let root = scratch("benchmark");
    let source = root.join("payload.bin");
    let archive = root.join("payload.zip");
    let destination = root.join("extracted");
    fs::write(&source, vec![0x5a; 16 * 1024 * 1024]).unwrap();

    let plan = prepare_creation(std::slice::from_ref(&source), &archive).unwrap();
    let started = Instant::now();
    create_archive(
        &engine,
        &archive,
        &plan,
        ArchiveFormat::Zip,
        CompressionLevel::Normal,
        None,
        None,
    )
    .unwrap();
    let create_elapsed = started.elapsed();

    let started = Instant::now();
    assert_eq!(list_archive(&engine, &archive, None).unwrap().len(), 1);
    let list_elapsed = started.elapsed();
    let started = Instant::now();
    test_archive(&engine, &archive, None).unwrap();
    let test_elapsed = started.elapsed();
    let started = Instant::now();
    extract_archive(&engine, &archive, &destination, None).unwrap();
    let extract_elapsed = started.elapsed();

    assert_eq!(
        fs::metadata(destination.join("payload.bin")).unwrap().len(),
        16 * 1024 * 1024
    );
    eprintln!(
        "16 MiB ZIP: create {create_elapsed:?}, list {list_elapsed:?}, test {test_elapsed:?}, extract {extract_elapsed:?}"
    );
    fs::remove_dir_all(root).unwrap();
}
