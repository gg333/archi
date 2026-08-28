use archive_app_lib::archive::{
    bundled_engine, create_archive, extract_entries, list_archive, prepare_creation, ArchiveFormat,
    CompressionLevel,
};
use std::fs;

#[test]
fn literal_wildcards_are_never_expanded_by_7zip() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("report?.txt");
    let neighbor = root.path().join("report1.txt");
    let star = root.path().join("literal*.txt");
    fs::write(&selected, "selected").unwrap();
    fs::write(&neighbor, "neighbor").unwrap();
    fs::write(&star, "star").unwrap();

    let engine = bundled_engine().unwrap();
    let archive = root.path().join("literal.zip");
    let plan = prepare_creation(&[selected.clone(), star.clone()], &archive).unwrap();
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

    let mut paths = list_archive(&engine, &archive, None)
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths, ["literal*.txt", "report?.txt"]);

    let all_archive = root.path().join("all.zip");
    let plan = prepare_creation(
        &[selected.clone(), neighbor.clone(), star.clone()],
        &all_archive,
    )
    .unwrap();
    create_archive(
        &engine,
        &all_archive,
        &plan,
        ArchiveFormat::Zip,
        CompressionLevel::Normal,
        None,
        None,
    )
    .unwrap();

    let destination = root.path().join("selected-output");
    extract_entries(
        &engine,
        &all_archive,
        &destination,
        &["report?.txt".to_string()],
        None,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("report?.txt")).unwrap(),
        "selected"
    );
    assert!(!destination.join("report1.txt").exists());
}
