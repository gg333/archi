use crate::archive::{ArchiveEntry, ArchiveError};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

const MAX_PAGE_SIZE: usize = 500;

#[derive(Clone, Default)]
pub(crate) struct ArchiveStore(Arc<Mutex<Option<OpenArchive>>>);

struct OpenArchive {
    path: PathBuf,
    entries: Arc<[ArchiveEntry]>,
    fingerprint: Fingerprint,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveDocument {
    pub path: String,
    pub name: String,
    pub engine_version: String,
    pub entry_count: usize,
    pub total_bytes: u64,
    pub encrypted: bool,
    pub skipped_links: usize,
    pub comment: Option<String>,
    pub can_modify: bool,
    pub volume_count: usize,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SortKey {
    Name,
    Type,
    Size,
    PackedSize,
    Ratio,
    Modified,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntryPage {
    pub folder: String,
    pub entries: Vec<ArchiveEntry>,
    pub file_types: Vec<String>,
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveFolder {
    pub path: String,
    pub name: String,
    pub has_children: bool,
}

impl ArchiveStore {
    pub(crate) fn install(
        &self,
        path: String,
        engine_version: String,
        entries: Vec<ArchiveEntry>,
    ) -> Result<ArchiveDocument, ArchiveError> {
        reject_duplicate_paths(&entries)?;
        let path_buf = PathBuf::from(&path);
        let fingerprint = fingerprint(&path_buf)?;
        let name = path_buf
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let document = summary(&path, &name, &engine_version, &entries);
        *self.0.lock().map_err(lock_error)? = Some(OpenArchive {
            path: path_buf,
            entries: entries.into(),
            fingerprint,
        });
        Ok(document)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn page(
        &self,
        path: &str,
        folder: &str,
        query: &str,
        file_type: &str,
        sort: SortKey,
        descending: bool,
        page: usize,
        page_size: usize,
        show_hidden: bool,
    ) -> Result<EntryPage, ArchiveError> {
        validate_folder(folder)?;
        let entries = {
            let guard = self.0.lock().map_err(lock_error)?;
            Arc::clone(&current(&guard, path)?.entries)
        };
        Ok(build_page(
            &entries,
            folder,
            query,
            file_type,
            sort,
            descending,
            page,
            page_size,
            show_hidden,
        ))
    }

    pub(crate) fn folders(
        &self,
        path: &str,
        folder: &str,
        show_hidden: bool,
    ) -> Result<Vec<ArchiveFolder>, ArchiveError> {
        validate_folder(folder)?;
        let entries = {
            let guard = self.0.lock().map_err(lock_error)?;
            Arc::clone(&current(&guard, path)?.entries)
        };
        Ok(child_folders(&entries, folder, show_hidden))
    }

    pub(crate) fn changed(&self, path: &str) -> Result<bool, ArchiveError> {
        let guard = self.0.lock().map_err(lock_error)?;
        let archive = current(&guard, path)?;
        Ok(fingerprint(&archive.path)? != archive.fingerprint)
    }

    #[cfg(test)]
    fn install_for_test(&self, path: PathBuf, entries: Vec<ArchiveEntry>) {
        *self.0.lock().unwrap() = Some(OpenArchive {
            fingerprint: fingerprint(&path).unwrap(),
            path,
            entries: entries.into(),
        });
    }
}

fn reject_duplicate_paths(entries: &[ArchiveEntry]) -> Result<(), ArchiveError> {
    let mut seen = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !seen.insert(entry.path.as_str()) {
            return Err(ArchiveError::new(
                "duplicate_entry",
                format!(
                    "This archive contains a duplicate entry path that cannot be selected safely: {}",
                    entry.path
                ),
            ));
        }
    }
    Ok(())
}

fn current<'a>(
    archive: &'a Option<OpenArchive>,
    path: &str,
) -> Result<&'a OpenArchive, ArchiveError> {
    let archive = archive.as_ref().ok_or_else(|| {
        ArchiveError::new(
            "archive_not_open",
            "Open the archive before browsing its entries",
        )
    })?;
    if archive.path != Path::new(path) {
        return Err(ArchiveError::new(
            "archive_session_changed",
            "The open archive changed; reopen it and try again",
        ));
    }
    Ok(archive)
}

fn summary(
    path: &str,
    name: &str,
    engine_version: &str,
    entries: &[ArchiveEntry],
) -> ArchiveDocument {
    ArchiveDocument {
        path: path.to_string(),
        name: name.to_string(),
        engine_version: engine_version.to_string(),
        entry_count: entries.len(),
        total_bytes: entries.iter().filter_map(|entry| entry.size).sum(),
        encrypted: entries.iter().any(|entry| entry.encrypted),
        skipped_links: 0,
        comment: None,
        can_modify: crate::archive::writable_format(Path::new(path)).is_some(),
        volume_count: volume_count(Path::new(path)),
    }
}

fn volume_count(path: &Path) -> usize {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return 1;
    };
    let Some(prefix) = name.strip_suffix("001") else {
        return 1;
    };
    let Some(parent) = path.parent() else {
        return 1;
    };
    (2..=999)
        .take_while(|number| parent.join(format!("{prefix}{number:03}")).is_file())
        .count()
        + 1
}

#[allow(clippy::too_many_arguments)]
fn build_page(
    entries: &[ArchiveEntry],
    folder: &str,
    query: &str,
    file_type: &str,
    sort: SortKey,
    descending: bool,
    requested_page: usize,
    requested_page_size: usize,
    show_hidden: bool,
) -> EntryPage {
    let query = query.trim().to_lowercase();
    let mut visible = if query.is_empty() {
        folder_entries(entries, folder, show_hidden)
    } else {
        entries
            .iter()
            .filter(|entry| show_hidden || !is_hidden(&entry.path))
            .filter(|entry| leaf_name(&entry.path).to_lowercase().contains(&query))
            .cloned()
            .collect()
    };
    let file_types = visible
        .iter()
        .map(file_type_key)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let file_type = file_type.trim().to_lowercase();
    if !file_type.is_empty() {
        visible.retain(|entry| file_type_key(entry) == file_type);
    }
    visible.sort_by(|left, right| compare_entries(left, right, sort, descending));

    let page_size = requested_page_size.clamp(25, MAX_PAGE_SIZE);
    let total = visible.len();
    let total_pages = total.div_ceil(page_size).max(1);
    let page = requested_page.clamp(1, total_pages);
    let start = (page - 1) * page_size;
    let entries = visible.into_iter().skip(start).take(page_size).collect();
    EntryPage {
        folder: folder.to_string(),
        entries,
        file_types,
        page,
        page_size,
        total,
        total_pages,
    }
}

fn folder_entries(entries: &[ArchiveEntry], folder: &str, show_hidden: bool) -> Vec<ArchiveEntry> {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut children = BTreeMap::<String, ArchiveEntry>::new();
    for entry in entries {
        if !show_hidden && is_hidden(&entry.path) {
            continue;
        }
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        if let Some((name, _)) = rest.split_once('/') {
            let path = format!("{prefix}{name}");
            let child = children
                .entry(path.clone())
                .or_insert_with(|| ArchiveEntry {
                    path,
                    is_directory: true,
                    size: Some(0),
                    packed_size: Some(0),
                    modified: None,
                    encrypted: false,
                    method: None,
                    is_link: false,
                    link_target: None,
                });
            child.size = add_optional(child.size, entry.size);
            child.packed_size = add_optional(child.packed_size, entry.packed_size);
            child.encrypted |= entry.encrypted;
        } else if entry.is_directory && children.contains_key(&entry.path) {
            let child = children.get_mut(&entry.path).expect("checked above");
            child.modified.clone_from(&entry.modified);
            child.method.clone_from(&entry.method);
            child.is_link = entry.is_link;
            child.link_target.clone_from(&entry.link_target);
        } else {
            children.insert(entry.path.clone(), entry.clone());
        }
    }
    children.into_values().collect()
}

fn child_folders(entries: &[ArchiveEntry], folder: &str, show_hidden: bool) -> Vec<ArchiveFolder> {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut children = BTreeMap::<&str, bool>::new();
    for entry in entries {
        if !show_hidden && is_hidden(&entry.path) {
            continue;
        }
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let (name, has_children) = match rest.split_once('/') {
            Some((name, remainder)) if !name.is_empty() => (name, !remainder.is_empty()),
            None if entry.is_directory && !rest.is_empty() => (rest, false),
            _ => continue,
        };
        children
            .entry(name)
            .and_modify(|current| *current |= has_children)
            .or_insert(has_children);
    }
    children
        .into_iter()
        .map(|(name, has_children)| ArchiveFolder {
            path: if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}{name}")
            },
            name: name.to_string(),
            has_children,
        })
        .collect()
}

fn compare_entries(
    left: &ArchiveEntry,
    right: &ArchiveEntry,
    sort: SortKey,
    descending: bool,
) -> Ordering {
    let folders = right.is_directory.cmp(&left.is_directory);
    if folders != Ordering::Equal {
        return folders;
    }
    let order = match sort {
        SortKey::Name => leaf_name(&left.path)
            .to_lowercase()
            .cmp(&leaf_name(&right.path).to_lowercase()),
        SortKey::Type => file_type_key(left).cmp(&file_type_key(right)),
        SortKey::Size => left.size.cmp(&right.size),
        SortKey::PackedSize => left.packed_size.cmp(&right.packed_size),
        SortKey::Ratio => ratio(left).total_cmp(&ratio(right)),
        SortKey::Modified => left.modified.cmp(&right.modified),
    };
    let order = if descending { order.reverse() } else { order };
    order.then_with(|| left.path.cmp(&right.path))
}

fn file_type_key(entry: &ArchiveEntry) -> String {
    if entry.is_link {
        return "__link__".to_string();
    }
    if entry.is_directory {
        return "__folder__".to_string();
    }
    let name = leaf_name(&entry.path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            extension.to_lowercase()
        }
        _ => "__file__".to_string(),
    }
}

fn ratio(entry: &ArchiveEntry) -> f64 {
    match (entry.size, entry.packed_size) {
        (Some(size), Some(packed)) if size > 0 => packed as f64 / size as f64,
        _ => 0.0,
    }
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        _ => None,
    }
}

fn is_hidden(path: &str) -> bool {
    path.split('/')
        .any(|part| part.starts_with('.') && part != ".")
}

fn leaf_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn validate_folder(folder: &str) -> Result<(), ArchiveError> {
    if folder.starts_with('/')
        || folder.starts_with('\\')
        || folder.contains('\0')
        || folder.split('/').any(|part| part == "..")
    {
        Err(ArchiveError::new(
            "invalid_folder",
            "The archive folder path is invalid",
        ))
    } else {
        Ok(())
    }
}

fn fingerprint(path: &Path) -> Result<Fingerprint, ArchiveError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ArchiveError::new(
            "archive_not_found",
            format!("Could not inspect the open archive: {error}"),
        )
    })?;
    Ok(Fingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> ArchiveError {
    ArchiveError::new("internal_error", "Archive browser state is unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        mem,
        time::{Duration, Instant},
    };

    fn entry(path: String, directory: bool, size: u64) -> ArchiveEntry {
        ArchiveEntry {
            path,
            is_directory: directory,
            size: Some(size),
            packed_size: Some(size / 2),
            modified: None,
            encrypted: false,
            method: None,
            is_link: false,
            link_target: None,
        }
    }

    #[test]
    fn frontend_archive_contract_uses_camel_case_fields() {
        let item = entry("file.txt".to_string(), false, 2);
        let document = ArchiveDocument {
            path: "/tmp/test.zip".to_string(),
            name: "test.zip".to_string(),
            engine_version: "7-Zip test".to_string(),
            entry_count: 1,
            total_bytes: 2,
            encrypted: false,
            skipped_links: 0,
            comment: None,
            can_modify: true,
            volume_count: 1,
        };
        let page = EntryPage {
            folder: String::new(),
            entries: vec![item],
            file_types: vec!["txt".to_string()],
            page: 1,
            page_size: 200,
            total: 1,
            total_pages: 1,
        };
        let folder = ArchiveFolder {
            path: "folder".to_string(),
            name: "folder".to_string(),
            has_children: true,
        };

        assert_eq!(
            serde_json::to_value(document).unwrap(),
            serde_json::json!({
                "path": "/tmp/test.zip",
                "name": "test.zip",
                "engineVersion": "7-Zip test",
                "entryCount": 1,
                "totalBytes": 2,
                "encrypted": false,
                "skippedLinks": 0,
                "comment": null,
                "canModify": true,
                "volumeCount": 1
            })
        );
        assert_eq!(
            serde_json::to_value(page).unwrap(),
            serde_json::json!({
                "folder": "",
                "entries": [{
                    "path": "file.txt",
                    "isDirectory": false,
                    "size": 2,
                    "packedSize": 1,
                    "modified": null,
                    "encrypted": false,
                    "method": null,
                    "isLink": false,
                    "linkTarget": null
                }],
                "fileTypes": ["txt"],
                "page": 1,
                "pageSize": 200,
                "total": 1,
                "totalPages": 1
            })
        );
        assert_eq!(
            serde_json::to_value(folder).unwrap(),
            serde_json::json!({
                "path": "folder",
                "name": "folder",
                "hasChildren": true
            })
        );
    }

    #[test]
    fn lists_only_immediate_visible_folder_children() {
        let entries = vec![
            entry("docs/readme.txt".to_string(), false, 1),
            entry("docs/guides/start.txt".to_string(), false, 1),
            entry("docs/empty".to_string(), true, 0),
            entry(".private/secret.txt".to_string(), false, 1),
            entry("docs/.drafts/note.txt".to_string(), false, 1),
        ];

        assert_eq!(
            child_folders(&entries, "", false),
            vec![ArchiveFolder {
                path: "docs".to_string(),
                name: "docs".to_string(),
                has_children: true,
            }]
        );
        assert_eq!(
            child_folders(&entries, "docs", false),
            vec![
                ArchiveFolder {
                    path: "docs/empty".to_string(),
                    name: "empty".to_string(),
                    has_children: false,
                },
                ArchiveFolder {
                    path: "docs/guides".to_string(),
                    name: "guides".to_string(),
                    has_children: true,
                },
            ]
        );
        assert_eq!(child_folders(&entries, "", true).len(), 2);
        assert_eq!(child_folders(&entries, "docs", true).len(), 3);
    }

    #[test]
    fn counts_only_contiguous_archive_volumes() {
        let root = tempfile::tempdir().unwrap();
        for suffix in ["001", "002", "004"] {
            fs::write(root.path().join(format!("split.7z.{suffix}")), suffix).unwrap();
        }
        assert_eq!(volume_count(&root.path().join("split.7z.001")), 2);
        assert_eq!(volume_count(&root.path().join("single.7z")), 1);
    }

    #[test]
    fn pages_virtual_folders_search_and_large_listings() {
        let started = Instant::now();
        let mut entries = vec![
            entry("folder/first.txt".to_string(), false, 10),
            entry("folder/nested/second.txt".to_string(), false, 20),
            entry(".hidden".to_string(), false, 1),
        ];
        entries.extend(
            (0..100_000).map(|index| entry(format!("bulk/file-{index:06}.txt"), false, index)),
        );
        let root = build_page(&entries, "", "", "", SortKey::Name, false, 1, 200, false);
        assert_eq!(
            root.entries
                .iter()
                .map(|value| value.path.as_str())
                .collect::<Vec<_>>(),
            ["bulk", "folder"]
        );
        let bulk = build_page(
            &entries,
            "bulk",
            "",
            "",
            SortKey::Name,
            false,
            500,
            200,
            false,
        );
        assert_eq!(bulk.total, 100_000);
        assert_eq!(bulk.entries.len(), 200);
        let search = build_page(
            &entries,
            "",
            "second",
            "",
            SortKey::Name,
            false,
            1,
            200,
            false,
        );
        assert_eq!(search.entries[0].path, "folder/nested/second.txt");
        let root_folders = child_folders(&entries, "", false);
        assert_eq!(root_folders.len(), 2);
        assert_eq!(child_folders(&entries, "bulk", false), []);
        let estimated_bytes = entries.len() * mem::size_of::<ArchiveEntry>()
            + entries
                .iter()
                .map(|entry| entry.path.capacity())
                .sum::<usize>();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(estimated_bytes < 300 * 1024 * 1024);
        eprintln!(
            "100k browse: {:?}, estimated Rust archive memory: {} MiB",
            started.elapsed(),
            estimated_bytes / 1024 / 1024
        );
    }

    #[test]
    fn filters_and_sorts_by_file_type() {
        let entries = vec![
            entry("photo.PNG".to_string(), false, 3),
            entry("notes.txt".to_string(), false, 2),
            entry("README".to_string(), false, 1),
            entry("folder".to_string(), true, 0),
        ];
        let filtered = build_page(&entries, "", "", "png", SortKey::Type, false, 1, 200, false);
        assert_eq!(filtered.entries[0].path, "photo.PNG");
        assert_eq!(
            filtered.file_types,
            ["__file__", "__folder__", "png", "txt"]
        );
    }

    #[test]
    fn detects_external_archive_changes() {
        let mut archive = tempfile::NamedTempFile::new().unwrap();
        archive.write_all(b"one").unwrap();
        let store = ArchiveStore::default();
        store.install_for_test(archive.path().to_path_buf(), vec![]);
        assert!(!store.changed(archive.path().to_str().unwrap()).unwrap());
        archive.write_all(b"two").unwrap();
        archive.flush().unwrap();
        assert!(store.changed(archive.path().to_str().unwrap()).unwrap());
    }

    #[test]
    fn rejects_duplicate_paths_before_they_reach_the_table() {
        let archive = tempfile::NamedTempFile::new().unwrap();
        let store = ArchiveStore::default();
        let error = store
            .install(
                archive.path().to_string_lossy().into_owned(),
                "test engine".to_string(),
                vec![
                    entry("duplicate.txt".to_string(), false, 1),
                    entry("duplicate.txt".to_string(), false, 2),
                ],
            )
            .unwrap_err();
        assert_eq!(error.code, "duplicate_entry");
    }
}
