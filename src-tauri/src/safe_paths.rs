use crate::archive::{ArchiveEntry, ArchiveError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

const STAGING_PREFIX: &str = "archive-app-extract-";
const STAGING_LOCK: &str = ".archive-app-staging.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    Ask,
    Replace,
    Skip,
    KeepBoth,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub files_extracted: usize,
    pub files_skipped: usize,
    pub renamed: usize,
}

pub(crate) struct StagingDirectory {
    _lock: File,
    _directory: tempfile::TempDir,
    payload: PathBuf,
}

impl StagingDirectory {
    pub(crate) fn create() -> Result<Self, ArchiveError> {
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .map_err(|error| {
                ArchiveError::new(
                    "staging_failed",
                    format!("Could not create a private staging folder: {error}"),
                )
            })?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join(STAGING_LOCK))
            .map_err(|error| {
                ArchiveError::new(
                    "staging_failed",
                    format!("Could not mark the private staging folder: {error}"),
                )
            })?;
        lock.lock().map_err(|error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not lock the private staging folder: {error}"),
            )
        })?;
        let payload = directory.path().join("payload");
        fs::create_dir(&payload).map_err(|error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not prepare the private staging folder: {error}"),
            )
        })?;
        Ok(Self {
            _lock: lock,
            _directory: directory,
            payload,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.payload
    }
}

pub fn cleanup_stale_staging() -> Result<usize, ArchiveError> {
    let mut removed = 0;
    for entry in fs::read_dir(std::env::temp_dir()).map_err(cleanup_error)? {
        let entry = entry.map_err(cleanup_error)?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(cleanup_error(error)),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join(STAGING_LOCK))
        else {
            continue;
        };
        match lock.try_lock() {
            Ok(()) => {
                drop(lock);
                match fs::remove_dir_all(path) {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(cleanup_error(error)),
                }
            }
            Err(error) => {
                let error: io::Error = error.into();
                if error.kind() != io::ErrorKind::WouldBlock {
                    return Err(cleanup_error(error));
                }
            }
        }
    }
    Ok(removed)
}

pub fn validate_archive_entries(entries: &[ArchiveEntry]) -> Result<(), ArchiveError> {
    let logical = entries
        .iter()
        .map(|entry| LogicalEntry {
            path: entry.path.clone(),
            is_directory: entry.is_directory,
            is_link: entry.is_link || entry.link_target.is_some(),
        })
        .collect::<Vec<_>>();
    validate_logical_entries(&logical)
}

pub fn validate_staging(root: &Path) -> Result<(), ArchiveError> {
    let metadata = fs::symlink_metadata(root).map_err(path_io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ArchiveError::new(
            "unsafe_staging",
            "The extraction staging path is not a real folder",
        ));
    }
    let mut logical = Vec::new();
    collect_staged_entries(root, root, &mut logical)?;
    validate_logical_entries(&logical)
}

pub fn commit_staging(
    staging: &Path,
    destination: &Path,
    protected_source: &Path,
    policy: ConflictPolicy,
) -> Result<CommitSummary, ArchiveError> {
    validate_staging(staging)?;
    fs::create_dir_all(destination).map_err(destination_error)?;
    let destination = destination.canonicalize().map_err(destination_error)?;
    let protected_source = protected_source.canonicalize().map_err(|error| {
        ArchiveError::new(
            "archive_not_found",
            format!("Could not resolve the source archive: {error}"),
        )
    })?;

    let mut index = DestinationIndex::default();
    let mut conflicts = Vec::new();
    for source in sorted_children(staging)? {
        let target = destination.join(source.file_name().ok_or_else(|| {
            ArchiveError::new("unsafe_path", "An extracted entry had no file name")
        })?);
        preflight_commit(
            &source,
            &target,
            &destination,
            &protected_source,
            policy,
            &mut conflicts,
            &mut index,
        )?;
    }
    if !conflicts.is_empty() {
        let preview = conflicts.into_iter().take(5).collect::<Vec<_>>().join(", ");
        return Err(ArchiveError::new(
            "conflict",
            format!("Files already exist: {preview}. Choose Replace, Skip, or Keep Both."),
        ));
    }

    let mut summary = CommitSummary::default();
    for source in sorted_children(staging)? {
        let target = destination.join(source.file_name().ok_or_else(|| {
            ArchiveError::new("unsafe_path", "An extracted entry had no file name")
        })?);
        commit_entry(&source, &target, policy, &mut summary, &mut index)?;
    }
    Ok(summary)
}

pub fn install_created_archive(source: &Path, destination: &Path) -> Result<(), ArchiveError> {
    let parent = destination.parent().ok_or_else(|| {
        ArchiveError::new(
            "invalid_destination",
            "Archive output path had no parent folder",
        )
    })?;
    if !source.is_file() || !parent.is_dir() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "Archive output or destination folder is unavailable",
        ));
    }
    if existing_equivalent(destination)?.is_some() {
        return Err(ArchiveError::new(
            "output_exists",
            "An item already exists at the archive output path",
        ));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".archive-app-create-")
        .tempfile_in(parent)
        .map_err(destination_error)?;
    let mut input = File::open(source).map_err(path_io_error)?;
    io::copy(&mut input, temporary.as_file_mut()).map_err(destination_error)?;
    temporary.as_file_mut().flush().map_err(destination_error)?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| destination_error(error.error))?;
    Ok(())
}

#[derive(Debug)]
struct LogicalEntry {
    path: String,
    is_directory: bool,
    is_link: bool,
}

fn validate_logical_entries(entries: &[LogicalEntry]) -> Result<(), ArchiveError> {
    let mut normalized = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.is_link {
            return Err(ArchiveError::new(
                "unsafe_link",
                format!("Archive links are disabled: {}", entry.path),
            ));
        }
        let components = safe_components(&entry.path)?;
        let key = components
            .iter()
            .map(|component| portable_key(component))
            .collect::<Vec<_>>();
        normalized.push((key, entry.is_directory, entry.path.as_str()));
    }
    normalized.sort_by(|left, right| {
        left.0
            .len()
            .cmp(&right.0.len())
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut seen = HashMap::new();
    for (components, is_directory, display) in normalized {
        let key = components.join("/");
        if seen.insert(key.clone(), is_directory).is_some() {
            return Err(ArchiveError::new(
                "normalization_collision",
                format!("Archive entries collide after normalization: {display}"),
            ));
        }
        for length in 1..components.len() {
            if seen.get(&components[..length].join("/")) == Some(&false) {
                return Err(ArchiveError::new(
                    "path_collision",
                    format!("A file is also used as a parent folder: {display}"),
                ));
            }
        }
    }
    Ok(())
}

fn safe_components(path: &str) -> Result<Vec<&str>, ArchiveError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || is_windows_drive_path(path)
    {
        return Err(unsafe_path(path));
    }
    let components = path.split(['/', '\\']).collect::<Vec<_>>();
    if components.iter().any(|component| component.is_empty()) {
        return Err(unsafe_path(path));
    }
    for component in &components {
        if matches!(*component, "." | "..")
            || component.chars().any(char::is_control)
            || component.contains(':')
            || component.ends_with([' ', '.'])
            || is_reserved_name(component)
            || component.starts_with(STAGING_PREFIX)
            || *component == STAGING_LOCK
        {
            return Err(unsafe_path(path));
        }
    }
    Ok(components)
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_reserved_name(component: &str) -> bool {
    let base = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn portable_key(component: &str) -> String {
    component.nfc().flat_map(char::to_lowercase).collect()
}

fn collect_staged_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<LogicalEntry>,
) -> Result<(), ArchiveError> {
    for path in sorted_children(current)? {
        let metadata = fs::symlink_metadata(&path).map_err(path_io_error)?;
        let relative = path.strip_prefix(root).map_err(|_| {
            ArchiveError::new("destination_escape", "An extracted path escaped staging")
        })?;
        let display = relative.to_str().ok_or_else(|| {
            ArchiveError::new(
                "unsafe_path",
                "An extracted file name could not be represented as Unicode",
            )
        })?;
        let is_link = metadata.file_type().is_symlink() || is_hard_link(&metadata);
        if !metadata.is_dir() && !metadata.is_file() && !is_link {
            return Err(ArchiveError::new(
                "unsafe_file_type",
                format!("Special files are disabled: {display}"),
            ));
        }
        entries.push(LogicalEntry {
            path: display.to_string(),
            is_directory: metadata.is_dir(),
            is_link,
        });
        if metadata.is_dir() && !is_link {
            collect_staged_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_hard_link(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn is_hard_link(_metadata: &fs::Metadata) -> bool {
    false
}

fn preflight_commit(
    source: &Path,
    target: &Path,
    destination: &Path,
    protected_source: &Path,
    policy: ConflictPolicy,
    conflicts: &mut Vec<String>,
    index: &mut DestinationIndex,
) -> Result<(), ArchiveError> {
    let source_metadata = fs::symlink_metadata(source).map_err(path_io_error)?;
    let existing = index.existing_equivalent(target)?;
    let Some(existing) = existing else {
        return Ok(());
    };
    if existing.canonicalize().ok().as_deref() == Some(protected_source) {
        return Err(ArchiveError::new(
            "source_overlap",
            "Extraction would overwrite the source archive",
        ));
    }
    let existing_metadata = fs::symlink_metadata(&existing).map_err(destination_error)?;
    if source_metadata.is_dir()
        && existing_metadata.is_dir()
        && !existing_metadata.file_type().is_symlink()
    {
        for child in sorted_children(source)? {
            preflight_commit(
                &child,
                &existing.join(child.file_name().ok_or_else(|| {
                    ArchiveError::new("unsafe_path", "An extracted entry had no file name")
                })?),
                destination,
                protected_source,
                policy,
                conflicts,
                index,
            )?;
        }
        return Ok(());
    }
    if policy == ConflictPolicy::Replace
        && (source_metadata.is_dir() != existing_metadata.is_dir()
            || existing_metadata.file_type().is_symlink())
    {
        return Err(ArchiveError::new(
            "conflict_type_mismatch",
            format!(
                "Replace cannot change a file into a folder or link: {}",
                existing.display()
            ),
        ));
    }
    if policy == ConflictPolicy::Ask {
        conflicts.push(
            existing
                .strip_prefix(destination)
                .unwrap_or(&existing)
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(())
}

fn commit_entry(
    source: &Path,
    target: &Path,
    policy: ConflictPolicy,
    summary: &mut CommitSummary,
    index: &mut DestinationIndex,
) -> Result<(), ArchiveError> {
    let source_metadata = fs::symlink_metadata(source).map_err(path_io_error)?;
    let existing = index.existing_equivalent(target)?;

    if source_metadata.is_dir() {
        let final_target = match existing {
            None => {
                fs::create_dir(target).map_err(destination_error)?;
                index.record(target)?;
                target.to_path_buf()
            }
            Some(path)
                if fs::symlink_metadata(&path)
                    .map_err(destination_error)?
                    .is_dir() =>
            {
                path
            }
            Some(_) if policy == ConflictPolicy::Skip => {
                summary.files_skipped += count_files(source)?;
                return Ok(());
            }
            Some(_) if policy == ConflictPolicy::KeepBoth => {
                summary.renamed += 1;
                let unique = unique_target(target, true, index)?;
                fs::create_dir(&unique).map_err(destination_error)?;
                index.record(&unique)?;
                unique
            }
            Some(_) => {
                return Err(ArchiveError::new(
                    "conflict_type_mismatch",
                    "Replace cannot change a file into a folder or link",
                ));
            }
        };
        for child in sorted_children(source)? {
            commit_entry(
                &child,
                &final_target.join(child.file_name().ok_or_else(|| {
                    ArchiveError::new("unsafe_path", "An extracted entry had no file name")
                })?),
                policy,
                summary,
                index,
            )?;
        }
        return Ok(());
    }

    let (final_target, replace) = match existing {
        None => (target.to_path_buf(), None),
        Some(_) if policy == ConflictPolicy::Skip => {
            summary.files_skipped += 1;
            return Ok(());
        }
        Some(path) if policy == ConflictPolicy::Replace => (path.clone(), Some(path)),
        Some(_) if policy == ConflictPolicy::KeepBoth => {
            summary.renamed += 1;
            (unique_target(target, false, index)?, None)
        }
        Some(_) => {
            return Err(ArchiveError::new(
                "conflict",
                "A destination file already exists",
            ));
        }
    };
    install_file(source, &final_target, replace.as_deref())?;
    if let Some(existing) = replace {
        index.remove(&existing)?;
    }
    index.record(&final_target)?;
    summary.files_extracted += 1;
    Ok(())
}

fn install_file(source: &Path, target: &Path, existing: Option<&Path>) -> Result<(), ArchiveError> {
    let parent = target.parent().ok_or_else(|| {
        ArchiveError::new(
            "invalid_destination",
            "Destination file had no parent folder",
        )
    })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".archive-app-part-")
        .tempfile_in(parent)
        .map_err(destination_error)?;
    let source_metadata = fs::metadata(source).map_err(path_io_error)?;
    let mut input = File::open(source).map_err(path_io_error)?;
    io::copy(&mut input, temporary.as_file_mut()).map_err(destination_error)?;
    temporary
        .as_file()
        .set_permissions(source_metadata.permissions())
        .map_err(destination_error)?;
    temporary
        .as_file()
        .set_times(
            FileTimes::new().set_modified(source_metadata.modified().map_err(path_io_error)?),
        )
        .map_err(destination_error)?;
    temporary.as_file_mut().flush().map_err(destination_error)?;

    let backup = if let Some(existing) = existing {
        let backup = unique_internal_path(parent, ".archive-app-backup-")?;
        fs::rename(existing, &backup).map_err(destination_error)?;
        Some(backup)
    } else {
        None
    };
    let persisted = if existing.is_some() {
        temporary.persist(target)
    } else {
        temporary.persist_noclobber(target)
    };
    match persisted {
        Ok(_) => {
            if let Some(backup) = backup {
                fs::remove_file(backup).map_err(destination_error)?;
            }
            fs::remove_file(source).map_err(path_io_error)?;
            Ok(())
        }
        Err(error) => {
            if let Some(backup) = backup {
                let _ = fs::rename(backup, target);
            }
            Err(destination_error(error.error))
        }
    }
}

#[derive(Default)]
struct DestinationIndex {
    directories: HashMap<PathBuf, HashMap<String, Vec<PathBuf>>>,
    #[cfg(test)]
    directory_reads: usize,
}

impl DestinationIndex {
    fn existing_equivalent(&mut self, target: &Path) -> Result<Option<PathBuf>, ArchiveError> {
        if fs::symlink_metadata(target).is_ok() {
            return Ok(Some(target.to_path_buf()));
        }
        let Some(parent) = target.parent() else {
            return Ok(None);
        };
        if !parent.is_dir() {
            return Ok(None);
        }
        let name = unicode_name(target)?;
        if !self.directories.contains_key(parent) {
            let mut names = HashMap::<String, Vec<PathBuf>>::new();
            for entry in fs::read_dir(parent).map_err(destination_error)? {
                let entry = entry.map_err(destination_error)?;
                if let Some(candidate) = entry.file_name().to_str() {
                    names
                        .entry(portable_key(candidate))
                        .or_default()
                        .push(entry.path());
                }
            }
            self.directories.insert(parent.to_path_buf(), names);
            #[cfg(test)]
            {
                self.directory_reads += 1;
            }
        }
        let matches = self
            .directories
            .get(parent)
            .and_then(|names| names.get(&portable_key(name)));
        if matches.is_some_and(|paths| paths.len() > 1) {
            return Err(ArchiveError::new(
                "destination_collision",
                format!("Destination contains names that collide after normalization: {name}"),
            ));
        }
        Ok(matches.and_then(|paths| paths.first().cloned()))
    }

    fn record(&mut self, path: &Path) -> Result<(), ArchiveError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let name = unicode_name(path)?;
        if let Some(names) = self.directories.get_mut(parent) {
            let paths = names.entry(portable_key(name)).or_default();
            if !paths.iter().any(|candidate| candidate == path) {
                paths.push(path.to_path_buf());
            }
        }
        Ok(())
    }

    fn remove(&mut self, path: &Path) -> Result<(), ArchiveError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let name = unicode_name(path)?;
        if let Some(paths) = self
            .directories
            .get_mut(parent)
            .and_then(|names| names.get_mut(&portable_key(name)))
        {
            paths.retain(|candidate| candidate != path);
        }
        Ok(())
    }
}

fn unicode_name(path: &Path) -> Result<&str, ArchiveError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArchiveError::new(
                "invalid_destination",
                "Destination file name could not be represented as Unicode",
            )
        })
}

fn existing_equivalent(target: &Path) -> Result<Option<PathBuf>, ArchiveError> {
    if fs::symlink_metadata(target).is_ok() {
        return Ok(Some(target.to_path_buf()));
    }
    let Some(parent) = target.parent() else {
        return Ok(None);
    };
    if !parent.is_dir() {
        return Ok(None);
    }
    let Some(name) = target.file_name().and_then(|value| value.to_str()) else {
        return Err(ArchiveError::new(
            "invalid_destination",
            "Destination file name could not be represented as Unicode",
        ));
    };
    let key = portable_key(name);
    let mut matches = Vec::new();
    for entry in fs::read_dir(parent).map_err(destination_error)? {
        let entry = entry.map_err(destination_error)?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|candidate| portable_key(candidate) == key)
        {
            matches.push(entry.path());
        }
    }
    if matches.len() > 1 {
        return Err(ArchiveError::new(
            "destination_collision",
            format!("Destination contains names that collide after normalization: {name}"),
        ));
    }
    Ok(matches.pop())
}

fn unique_target(
    target: &Path,
    is_directory: bool,
    index: &mut DestinationIndex,
) -> Result<PathBuf, ArchiveError> {
    let parent = target.parent().ok_or_else(|| {
        ArchiveError::new("invalid_destination", "Destination had no parent folder")
    })?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArchiveError::new(
                "invalid_destination",
                "Destination file name could not be represented as Unicode",
            )
        })?;
    let path = Path::new(name);
    let stem = if is_directory {
        name
    } else {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(name)
    };
    let extension = if is_directory {
        None
    } else {
        path.extension().and_then(|value| value.to_str())
    };
    for number in 2..10_000 {
        let candidate = match extension {
            Some(extension) => parent.join(format!("{stem} ({number}).{extension}")),
            None => parent.join(format!("{stem} ({number})")),
        };
        if index.existing_equivalent(&candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Err(ArchiveError::new(
        "destination_full",
        format!("Could not find a non-conflicting name for {name}"),
    ))
}

fn unique_internal_path(parent: &Path, prefix: &str) -> Result<PathBuf, ArchiveError> {
    for number in 0..10_000 {
        let candidate = parent.join(format!("{prefix}{}-{number}", std::process::id()));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(ArchiveError::new(
        "destination_full",
        "Could not create a temporary replacement path",
    ))
}

fn sorted_children(path: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    let mut children = fs::read_dir(path)
        .map_err(path_io_error)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(path_io_error))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    Ok(children)
}

fn count_files(path: &Path) -> Result<usize, ArchiveError> {
    let metadata = fs::symlink_metadata(path).map_err(path_io_error)?;
    if metadata.is_file() {
        return Ok(1);
    }
    let mut count = 0;
    for child in sorted_children(path)? {
        count += count_files(&child)?;
    }
    Ok(count)
}

fn unsafe_path(path: &str) -> ArchiveError {
    ArchiveError::new(
        "unsafe_path",
        format!("Archive entry uses an unsafe path: {path}"),
    )
}

fn path_io_error(error: io::Error) -> ArchiveError {
    ArchiveError::new(
        "staging_failed",
        format!("Could not inspect extracted files: {error}"),
    )
}

fn destination_error(error: io::Error) -> ArchiveError {
    ArchiveError::new(
        "destination_failed",
        format!("Could not update the destination: {error}"),
    )
}

fn cleanup_error(error: io::Error) -> ArchiveError {
    ArchiveError::new(
        "cleanup_failed",
        format!("Could not clean a stale extraction: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static STAGING_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn active_staging_is_locked_and_removed_on_drop() {
        let _guard = STAGING_TEST_LOCK.lock().unwrap();
        let staging = StagingDirectory::create().unwrap();
        let root = staging.path().parent().unwrap().to_path_buf();
        cleanup_stale_staging().unwrap();
        assert!(root.exists());
        drop(staging);
        assert!(!root.exists());
    }

    #[test]
    fn stale_owned_staging_is_removed_on_cleanup() {
        let _guard = STAGING_TEST_LOCK.lock().unwrap();
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .unwrap();
        File::create(directory.path().join(STAGING_LOCK)).unwrap();
        let path = directory.keep();
        cleanup_stale_staging().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_ignores_unmarked_and_symlinked_matching_paths() {
        let _guard = STAGING_TEST_LOCK.lock().unwrap();
        let unmarked = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .unwrap();
        cleanup_stale_staging().unwrap();
        assert!(unmarked.path().exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = tempfile::tempdir().unwrap();
            let link = std::env::temp_dir()
                .join(format!("{STAGING_PREFIX}symlink-{}", std::process::id()));
            symlink(target.path(), &link).unwrap();
            cleanup_stale_staging().unwrap();
            assert!(link.is_symlink());
            fs::remove_file(link).unwrap();
        }
    }

    #[test]
    fn destination_index_reads_each_directory_once() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("existing.txt"), "existing").unwrap();
        let mut index = DestinationIndex::default();
        for number in 0..100 {
            assert!(index
                .existing_equivalent(&directory.path().join(format!("new-{number}.txt")))
                .unwrap()
                .is_none());
        }
        assert_eq!(index.directory_reads, 1);
    }
}
