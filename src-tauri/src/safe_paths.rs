use crate::archive::{ArchiveEntry, ArchiveError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use unicode_normalization::UnicodeNormalization;

pub(crate) mod quarantine;

const STAGING_PREFIX: &str = "archive-app-extract-";
const STAGING_LOCK: &str = ".archive-app-staging.lock";
const EXTERNAL_STAGING_PREFIX: &str = ".archive-app-extract-";
const EXTERNAL_LOCATION: &str = "external-location";
const PREVIEW_PREFIX: &str = "preview-";
const PREVIEW_MARKER: &str = ".archi-preview";
const MAX_ARCHIVE_ENTRIES: usize = 1_000_000;
const MAX_ARCHIVE_DEPTH: usize = 100;

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
    _external: Option<tempfile::TempDir>,
    payload: PathBuf,
}

pub(crate) struct ArchiveRewrite {
    source: PathBuf,
    temporary: PathBuf,
    fingerprint: RewriteFingerprint,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RewriteFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl ArchiveRewrite {
    pub(crate) fn create(source: &Path) -> Result<Self, ArchiveError> {
        let metadata = fs::symlink_metadata(source).map_err(path_io_error)?;
        if !source.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ArchiveError::new(
                "archive_not_writable",
                "Only regular archive files can be modified",
            ));
        }
        let parent = source.parent().ok_or_else(|| {
            ArchiveError::new("archive_not_writable", "The archive has no parent folder")
        })?;
        let suffix = source
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let mut temporary = tempfile::Builder::new()
            .prefix(".archive-app-rewrite-")
            .suffix(&suffix)
            .tempfile_in(parent)
            .map_err(destination_error)?;
        let mut input = File::open(source).map_err(path_io_error)?;
        io::copy(&mut input, temporary.as_file_mut()).map_err(destination_error)?;
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(destination_error)?;
        temporary.as_file_mut().flush().map_err(destination_error)?;
        temporary.as_file().sync_all().map_err(destination_error)?;
        quarantine::copy(source, temporary.path()).map_err(destination_error)?;
        let (_, temporary) = temporary
            .keep()
            .map_err(|error| destination_error(error.error))?;
        Ok(Self {
            source: source.to_path_buf(),
            temporary,
            fingerprint: rewrite_fingerprint(&metadata),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.temporary
    }

    pub(crate) fn recovery_error(&self, error: ArchiveError) -> ArchiveError {
        // A cancelled rewrite is an unfinished copy with nothing worth recovering.
        if error.code == "cancelled" {
            let _ = fs::remove_file(&self.temporary);
            return error;
        }
        ArchiveError::new(
            &error.code,
            format!(
                "{} The recoverable rewritten archive is at {}",
                error.message,
                self.temporary.display()
            ),
        )
    }

    pub(crate) fn commit(self) -> Result<(), ArchiveError> {
        let current = fs::symlink_metadata(&self.source)
            .map_err(|error| self.recovery_error(path_io_error(error)))?;
        if rewrite_fingerprint(&current) != self.fingerprint {
            return Err(self.recovery_error(ArchiveError::new(
                "archive_changed",
                "The source archive changed before replacement.",
            )));
        }
        let parent = self.source.parent().ok_or_else(|| {
            self.recovery_error(ArchiveError::new(
                "archive_not_writable",
                "The archive has no parent folder.",
            ))
        })?;
        let backup = unique_internal_path(parent, ".archive-app-backup-")
            .map_err(|error| self.recovery_error(error))?;
        #[cfg(unix)]
        let source_moved = match fs::hard_link(&self.source, &backup) {
            Ok(()) => false,
            Err(_) => {
                fs::rename(&self.source, &backup).map_err(|error| {
                    self.recovery_error(ArchiveError::new(
                        "replacement_failed",
                        format!("Could not preserve the original archive: {error}."),
                    ))
                })?;
                true
            }
        };
        #[cfg(not(unix))]
        let source_moved = {
            fs::rename(&self.source, &backup).map_err(|error| {
                self.recovery_error(ArchiveError::new(
                    "replacement_failed",
                    format!("Could not preserve the original archive: {error}."),
                ))
            })?;
            true
        };
        if let Err(error) = fs::rename(&self.temporary, &self.source) {
            if source_moved {
                let _ = fs::rename(&backup, &self.source);
            } else {
                let _ = fs::remove_file(&backup);
            }
            return Err(self.recovery_error(ArchiveError::new(
                "replacement_failed",
                format!("Could not install the rewritten archive: {error}."),
            )));
        }
        if let Err(error) = File::open(&self.source)
            .and_then(|file| file.sync_all())
            .and_then(|()| sync_parent(parent))
        {
            let _ = fs::rename(&self.source, &self.temporary);
            let _ = fs::rename(&backup, &self.source);
            return Err(self.recovery_error(ArchiveError::new(
                "replacement_failed",
                format!("Could not make the archive replacement durable: {error}."),
            )));
        }
        let _ = fs::remove_file(backup);
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_: &Path) -> io::Result<()> {
    Ok(())
}

fn rewrite_fingerprint(metadata: &fs::Metadata) -> RewriteFingerprint {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    RewriteFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

impl StagingDirectory {
    pub(crate) fn create() -> Result<Self, ArchiveError> {
        Self::build(None)
    }

    /// Stages on the volume that will receive the result, so finished files can be moved
    /// into place instead of being written a second time. When `target` is on another
    /// volume than the system temporary folder, the work happens in a hidden folder beside
    /// it; a record in the temporary folder lets a later launch clean up after a crash.
    pub(crate) fn create_near(target: &Path) -> Result<Self, ArchiveError> {
        match other_volume_anchor(target) {
            Some(anchor) => Self::build(Some(&anchor)).or_else(|_| Self::build(None)),
            None => Self::build(None),
        }
    }

    fn build(external_parent: Option<&Path>) -> Result<Self, ArchiveError> {
        let staging_error = |error: io::Error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not prepare a private staging folder: {error}"),
            )
        };
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .map_err(staging_error)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join(STAGING_LOCK))
            .map_err(staging_error)?;
        lock.lock().map_err(staging_error)?;
        let external = match external_parent {
            Some(parent) => {
                let external = tempfile::Builder::new()
                    .prefix(EXTERNAL_STAGING_PREFIX)
                    .tempdir_in(parent)
                    .map_err(staging_error)?;
                let location = external.path().to_str().ok_or_else(|| {
                    staging_error(io::Error::other("the staging path is not valid Unicode"))
                })?;
                File::create(external.path().join(STAGING_LOCK)).map_err(staging_error)?;
                fs::write(directory.path().join(EXTERNAL_LOCATION), location)
                    .map_err(staging_error)?;
                Some(external)
            }
            None => None,
        };
        let payload = external
            .as_ref()
            .map_or(directory.path(), |external| external.path())
            .join("payload");
        fs::create_dir(&payload).map_err(staging_error)?;
        Ok(Self {
            _lock: lock,
            _directory: directory,
            _external: external,
            payload,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.payload
    }
}

/// The nearest existing folder at or above `target`, when it is on a different volume
/// than the system temporary folder.
#[cfg(unix)]
fn other_volume_anchor(target: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut anchor = target;
    while !anchor.is_dir() {
        anchor = anchor.parent()?;
    }
    let anchor = anchor.canonicalize().ok()?;
    let temporary = fs::metadata(std::env::temp_dir()).ok()?;
    (fs::metadata(&anchor).ok()?.dev() != temporary.dev()).then_some(anchor)
}

#[cfg(not(unix))]
fn other_volume_anchor(_target: &Path) -> Option<PathBuf> {
    None
}

/// Removes the hidden staging folder a crashed job left on another volume, after checking
/// that the recorded path really is one of Archi's.
fn remove_external_staging(record: &Path) {
    let Ok(location) = fs::read_to_string(record.join(EXTERNAL_LOCATION)) else {
        return;
    };
    let external = PathBuf::from(location);
    let owned = external.is_absolute()
        && external
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(EXTERNAL_STAGING_PREFIX))
        && fs::symlink_metadata(&external)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        && fs::symlink_metadata(external.join(STAGING_LOCK))
            .is_ok_and(|metadata| metadata.is_file());
    if owned {
        let _ = fs::remove_dir_all(external);
    }
}

/// Moves a finished file into place without copying it. Returns false when the two paths
/// cannot be joined by a rename (another volume, or a file system without the needed
/// support), so the caller can copy instead. An existing `target` is never replaced.
fn move_without_copy(source: &Path, target: &Path) -> io::Result<bool> {
    match tempfile::TempPath::try_from_path(source)?.persist_noclobber(target) {
        Ok(()) => Ok(true),
        Err(error) => {
            // The wrapper would otherwise delete the source when dropped.
            let _ = error.path.keep();
            match error.error.kind() {
                io::ErrorKind::AlreadyExists => Err(error.error),
                _ if lacks_exclusive_rename(&error.error) && is_vacant(target) => {
                    Ok(fs::rename(source, target).is_ok())
                }
                _ => Ok(false),
            }
        }
    }
}

/// True when the file system cannot rename without replacing (exFAT and FAT, for example).
fn lacks_exclusive_rename(error: &io::Error) -> bool {
    // ENOTSUP on macOS and EOPNOTSUPP on Linux; std does not map either to a kind.
    const NOT_SUPPORTED: i32 = if cfg!(target_os = "macos") { 45 } else { 95 };
    error.kind() == io::ErrorKind::Unsupported || error.raw_os_error() == Some(NOT_SUPPORTED)
}

fn is_vacant(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}

/// Gives a finished temporary file its final name without replacing anything. exFAT and
/// FAT volumes offer no atomic way to do that, so there the name is checked and then taken.
fn persist_new(temporary: tempfile::NamedTempFile, target: &Path) -> io::Result<()> {
    match temporary.persist_noclobber(target) {
        Ok(_) => Ok(()),
        Err(error) if lacks_exclusive_rename(&error.error) && is_vacant(target) => error
            .file
            .persist(target)
            .map(|_| ())
            .map_err(|error| error.error),
        Err(error) => Err(error.error),
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
                remove_external_staging(&path);
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

pub fn cleanup_stale_previews(root: &Path, older_than: Duration) -> Result<usize, ArchiveError> {
    prepare_preview_root(root)?;
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in fs::read_dir(root).map_err(preview_io_error)? {
        let entry = entry.map_err(preview_io_error)?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(PREVIEW_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(preview_io_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let marker = path.join(PREVIEW_MARKER);
        let marker_metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(preview_io_error(error)),
        };
        let age = now
            .duration_since(marker_metadata.modified().map_err(preview_io_error)?)
            .unwrap_or_default();
        if age < older_than {
            continue;
        }
        match fs::remove_dir_all(path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(preview_io_error(error)),
        }
    }
    Ok(removed)
}

/// Why one entry is being copied out of an archive into the private cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryCopy {
    /// Opened or shown in Quick Look: capped by the preview limit and never executable.
    Preview,
    /// Dragged to Finder: an ordinary extraction of one file, so scripts and large files
    /// are allowed. The copy keeps its quarantine marker and is never launched by Archi.
    DragOut,
}

pub(crate) fn validate_preview_entry(
    entry: &ArchiveEntry,
    max_bytes: u64,
    purpose: EntryCopy,
) -> Result<(), ArchiveError> {
    safe_components(&entry.path)?;
    if entry.is_directory {
        return Err(ArchiveError::new(
            "preview_is_directory",
            "Open folders by navigating inside the archive",
        ));
    }
    if entry.is_link || entry.link_target.is_some() {
        return Err(ArchiveError::new(
            "unsafe_link",
            "Archive links cannot be opened or previewed",
        ));
    }
    if purpose == EntryCopy::DragOut {
        // An undeclared size is still bounded while the entry is extracted.
        return match entry.size {
            Some(size) if size > max_bytes => Err(ArchiveError::new(
                "expansion_limit_exceeded",
                format!(
                    "This entry is {size} bytes, above the configured {max_bytes}-byte extraction limit. Use Extract to continue anyway."
                ),
            )),
            _ => Ok(()),
        };
    }
    let size = entry.size.ok_or_else(|| {
        ArchiveError::new(
            "preview_size_unknown",
            "This entry does not declare a size and cannot be opened safely",
        )
    })?;
    if size > max_bytes {
        return Err(ArchiveError::new(
            "preview_size_exceeded",
            format!("This entry is {size} bytes, above the {max_bytes}-byte preview limit"),
        ));
    }
    if has_executable_extension(&entry.path) {
        return Err(executable_preview_error());
    }
    Ok(())
}

pub(crate) fn persist_preview_file(
    staging: &Path,
    selected_path: &str,
    preview_root: &Path,
    max_bytes: u64,
    purpose: EntryCopy,
) -> Result<PathBuf, ArchiveError> {
    validate_staging(staging)?;
    if count_files(staging)? != 1 {
        return Err(ArchiveError::new(
            "preview_output_invalid",
            "The archive engine did not produce exactly one preview file",
        ));
    }
    let components = safe_components(selected_path)?;
    let source = components
        .iter()
        .fold(staging.to_path_buf(), |path, component| {
            path.join(component)
        });
    let metadata = fs::symlink_metadata(&source).map_err(path_io_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_hard_link(&metadata) {
        return Err(ArchiveError::new(
            "unsafe_file_type",
            "Only regular archive files can be opened or previewed",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(ArchiveError::new(
            "preview_size_exceeded",
            format!(
                "The extracted entry is {} bytes, above the {max_bytes}-byte limit",
                metadata.len()
            ),
        ));
    }
    if purpose == EntryCopy::Preview && is_executable_payload(&source, &metadata)? {
        return Err(executable_preview_error());
    }

    prepare_preview_root(preview_root)?;
    let directory = tempfile::Builder::new()
        .prefix(PREVIEW_PREFIX)
        .tempdir_in(preview_root)
        .map_err(preview_io_error)?;
    restrict_preview_directory(directory.path())?;
    let marker = directory.path().join(PREVIEW_MARKER);
    File::create(&marker).map_err(preview_io_error)?;
    restrict_preview_file(&marker)?;
    let file_name = source.file_name().ok_or_else(|| {
        ArchiveError::new("preview_output_invalid", "The preview file has no name")
    })?;
    let destination = directory.path().join(file_name);
    let mut input = File::open(&source).map_err(preview_io_error)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&destination)
        .map_err(preview_io_error)?;
    io::copy(&mut input, &mut output).map_err(preview_io_error)?;
    output.flush().map_err(preview_io_error)?;
    output.sync_all().map_err(preview_io_error)?;
    match purpose {
        EntryCopy::Preview => restrict_preview_file(&destination)?,
        // The dragged copy is the user's extracted file, so it keeps the archived mode.
        EntryCopy::DragOut => {
            fs::set_permissions(&destination, metadata.permissions()).map_err(preview_io_error)?
        }
    }
    quarantine::copy(staging, &destination).map_err(preview_io_error)?;
    let kept = directory.keep();
    Ok(kept.join(file_name))
}

pub fn validate_archive_entries(entries: &[ArchiveEntry]) -> Result<(), ArchiveError> {
    // Links are never written to disk: extraction leaves them out, and anything that
    // still appears as a link in staging is rejected by `validate_staging`.
    let logical = entries
        .iter()
        .filter(|entry| !is_archive_link(entry))
        .map(|entry| LogicalEntry {
            path: entry.path.clone(),
            is_directory: entry.is_directory,
            is_link: false,
        })
        .collect::<Vec<_>>();
    validate_logical_entries(&logical)
}

pub(crate) fn is_archive_link(entry: &ArchiveEntry) -> bool {
    entry.is_link || entry.link_target.is_some()
}

/// Gives each hard link its own copy of the file it names. Both paths are archive entry
/// paths resolved inside `staging`; the engine is never asked to create a link. `extras`
/// are files extracted only to be copied from, and are removed afterwards.
pub(crate) fn materialize_hard_links(
    staging: &Path,
    links: &[(String, String)],
    extras: &[String],
) -> Result<(), ArchiveError> {
    let staged = |path: &str| -> Result<PathBuf, ArchiveError> {
        Ok(safe_components(path)?
            .iter()
            .fold(staging.to_path_buf(), |joined, part| joined.join(part)))
    };
    for (link, target) in links {
        let source = staged(target)?;
        let destination = staged(link)?;
        let metadata = fs::symlink_metadata(&source).map_err(path_io_error)?;
        if !metadata.is_file() || is_hard_link(&metadata) {
            return Err(ArchiveError::new(
                "unsafe_link",
                format!("The file a hard link names was not extracted as a regular file: {target}"),
            ));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(path_io_error)?;
        }
        let mut input = File::open(&source).map_err(path_io_error)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(path_io_error)?;
        io::copy(&mut input, &mut output).map_err(path_io_error)?;
        output
            .set_permissions(metadata.permissions())
            .map_err(path_io_error)?;
        output
            .set_times(FileTimes::new().set_modified(metadata.modified().map_err(path_io_error)?))
            .map_err(path_io_error)?;
    }
    for extra in extras {
        let path = staged(extra)?;
        fs::remove_file(&path).map_err(path_io_error)?;
        // Folders that existed only to hold it would otherwise be extracted empty.
        let mut folder = path.parent();
        while let Some(current) = folder.filter(|current| *current != staging) {
            if fs::remove_dir(current).is_err() {
                break;
            }
            folder = current.parent();
        }
    }
    Ok(())
}

pub fn validate_staging(root: &Path) -> Result<(), ArchiveError> {
    let metadata = fs::symlink_metadata(root).map_err(path_io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ArchiveError::new(
            "unsafe_staging",
            "The extraction staging path is not a real folder",
        ));
    }
    let (logical, _) = inspect_staging(root, None, true)?;
    validate_logical_entries(&logical)
}

/// Checks the staged output against the limits and returns the bytes written so far.
pub(crate) fn enforce_staging_limits(
    root: &Path,
    max_bytes: Option<u64>,
) -> Result<u64, ArchiveError> {
    inspect_staging(root, max_bytes, false).map(|(_, bytes)| bytes)
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
        commit_entry(&source, &target, policy, &mut summary, &mut index, staging)?;
    }
    Ok(summary)
}

pub fn install_created_archive(source: &Path, destination: &Path) -> Result<(), ArchiveError> {
    install_created_archives(&[source.to_path_buf()], source, destination, false, false).map(|_| ())
}

pub(crate) fn install_created_archives(
    sources: &[PathBuf],
    source_base: &Path,
    destination_base: &Path,
    replace_existing: bool,
    replace_volumes: bool,
) -> Result<Vec<PathBuf>, ArchiveError> {
    if sources.is_empty() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The archive engine produced no output files",
        ));
    }
    let parent = destination_base.parent().ok_or_else(|| {
        ArchiveError::new(
            "invalid_destination",
            "Archive output path had no parent folder",
        )
    })?;
    if !parent.is_dir() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The archive destination folder is unavailable",
        ));
    }
    let source_name = source_base
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArchiveError::new(
                "invalid_destination",
                "Archive output name is not valid Unicode",
            )
        })?;
    let destination_name = destination_base
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArchiveError::new(
                "invalid_destination",
                "Archive output name is not valid Unicode",
            )
        })?;
    let mut targets = Vec::with_capacity(sources.len());
    let mut split = false;
    for source in sources {
        if !source.is_file() {
            return Err(ArchiveError::new(
                "invalid_destination",
                "An archive output volume is unavailable",
            ));
        }
        let name = source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                ArchiveError::new(
                    "invalid_destination",
                    "Archive volume name is not valid Unicode",
                )
            })?;
        let suffix = name.strip_prefix(source_name).ok_or_else(|| {
            ArchiveError::new(
                "invalid_destination",
                "Archive volume names are inconsistent",
            )
        })?;
        let target = parent.join(format!("{destination_name}{suffix}"));
        // Only the exact file the user confirmed may be replaced, never a differently
        // spelled equivalent. An existing volume set is moved aside as a whole below.
        let replace = replace_existing && suffix.is_empty();
        split |= !suffix.is_empty();
        if !suffix.is_empty() && replace_volumes {
            targets.push((target, false));
            continue;
        }
        if let Some(existing) = existing_equivalent(&target)? {
            let replaceable = replace
                && existing == target
                && fs::symlink_metadata(&existing).is_ok_and(|metadata| metadata.is_file());
            if !replaceable {
                return Err(ArchiveError::new(
                    "output_exists",
                    format!("An item already exists at {}", target.display()),
                ));
            }
        }
        targets.push((target, replace));
    }

    // Replacing a split archive retires every volume of the old set, including ones the
    // new set is too short to overwrite. They are kept until the new set is in place.
    let mut retired = Vec::new();
    if split && replace_volumes {
        for volume in existing_volumes(destination_base)? {
            let backup = unique_internal_path(parent, ".archive-app-backup-")?;
            if let Err(error) = fs::rename(&volume, &backup) {
                restore_retired(&retired);
                return Err(destination_error(error));
            }
            retired.push((volume, backup));
        }
    }
    let mut installed = Vec::with_capacity(sources.len());
    for (source, (target, replace)) in sources.iter().zip(&targets) {
        match install_archive_file(source, target, *replace, parent) {
            Ok(()) => installed.push(target.to_path_buf()),
            Err(error) => {
                for path in &installed {
                    let _ = fs::remove_file(path);
                }
                restore_retired(&retired);
                return Err(error);
            }
        }
    }
    for (_, backup) in retired {
        let _ = fs::remove_file(backup);
    }
    // The save panel confirmed replacing this single file; a split set takes its place.
    if split
        && replace_existing
        && fs::symlink_metadata(destination_base).is_ok_and(|m| m.is_file())
    {
        let _ = fs::remove_file(destination_base);
    }
    Ok(installed)
}

fn restore_retired(retired: &[(PathBuf, PathBuf)]) {
    for (volume, backup) in retired {
        let _ = fs::rename(backup, volume);
    }
}

/// The numbered files (`name.001`, `name.002`, ...) of a split archive already at `base`.
pub(crate) fn existing_volumes(base: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    let (Some(parent), Some(name)) = (base.parent(), base.file_name().and_then(|n| n.to_str()))
    else {
        return Ok(Vec::new());
    };
    if !parent.is_dir() {
        return Ok(Vec::new());
    }
    let prefix = format!("{name}.");
    let mut volumes = Vec::new();
    for entry in fs::read_dir(parent).map_err(destination_error)? {
        let entry = entry.map_err(destination_error)?;
        let is_volume = entry.file_name().to_str().is_some_and(|candidate| {
            candidate.strip_prefix(&prefix).is_some_and(|number| {
                number.len() == 3 && number.bytes().all(|byte| byte.is_ascii_digit())
            })
        });
        if is_volume && entry.file_type().map_err(destination_error)?.is_file() {
            volumes.push(entry.path());
        }
    }
    volumes.sort();
    Ok(volumes)
}

/// Puts one finished archive file at its destination, moving it when the workspace shares
/// the destination's volume and copying it otherwise.
fn install_archive_file(
    source: &Path,
    target: &Path,
    replace: bool,
    parent: &Path,
) -> Result<(), ArchiveError> {
    File::open(source)
        .and_then(|file| file.sync_all())
        .map_err(path_io_error)?;
    let moved = if replace {
        fs::rename(source, target).is_ok()
    } else {
        move_without_copy(source, target).map_err(destination_error)?
    };
    if moved {
        return Ok(());
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".archive-app-create-")
        .tempfile_in(parent)
        .map_err(destination_error)?;
    let mut input = File::open(source).map_err(path_io_error)?;
    io::copy(&mut input, temporary.as_file_mut()).map_err(destination_error)?;
    temporary.as_file_mut().flush().map_err(destination_error)?;
    temporary.as_file().sync_all().map_err(destination_error)?;
    let persisted = if replace {
        temporary
            .persist(target)
            .map(|_| ())
            .map_err(|error| error.error)
    } else {
        persist_new(temporary, target)
    };
    persisted.map_err(destination_error)
}

#[derive(Debug)]
struct LogicalEntry {
    path: String,
    is_directory: bool,
    is_link: bool,
}

fn validate_logical_entries(entries: &[LogicalEntry]) -> Result<(), ArchiveError> {
    if entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ArchiveError::new(
            "entry_limit_exceeded",
            format!("This archive contains more than {MAX_ARCHIVE_ENTRIES} entries"),
        ));
    }
    let mut normalized = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.is_link {
            return Err(ArchiveError::new(
                "unsafe_link",
                format!("Archive links are disabled: {}", entry.path),
            ));
        }
        let components = safe_components(&entry.path)?;
        if components.len() > MAX_ARCHIVE_DEPTH {
            return Err(ArchiveError::new(
                "path_depth_exceeded",
                format!(
                    "Archive paths cannot be deeper than {MAX_ARCHIVE_DEPTH} folders: {}",
                    entry.path
                ),
            ));
        }
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
            || (cfg!(windows) && is_windows_unsafe_name(component))
            || component.starts_with(STAGING_PREFIX)
            || component.starts_with(EXTERNAL_STAGING_PREFIX)
            || *component == STAGING_LOCK
        {
            return Err(unsafe_path(path));
        }
    }
    Ok(components)
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (cfg!(windows) || matches!(bytes.get(2), None | Some(b'/' | b'\\')))
}

// Names Windows cannot create; they are ordinary file names everywhere else.
fn is_windows_unsafe_name(component: &str) -> bool {
    component.contains(':') || component.ends_with([' ', '.']) || is_reserved_name(component)
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

fn inspect_staging(
    root: &Path,
    max_bytes: Option<u64>,
    collect_entries: bool,
) -> Result<(Vec<LogicalEntry>, u64), ArchiveError> {
    let mut entries = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    let mut count = 0_usize;
    let mut total_bytes = 0_u64;
    while let Some((current, depth)) = stack.pop() {
        for path in sorted_children(&current)? {
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
            count += 1;
            if count > MAX_ARCHIVE_ENTRIES {
                return Err(ArchiveError::new(
                    "entry_limit_exceeded",
                    format!("Extraction produced more than {MAX_ARCHIVE_ENTRIES} entries"),
                ));
            }
            let entry_depth = depth + 1;
            if entry_depth > MAX_ARCHIVE_DEPTH {
                return Err(ArchiveError::new(
                    "path_depth_exceeded",
                    format!("Extraction produced a path deeper than {MAX_ARCHIVE_DEPTH} folders"),
                ));
            }
            if metadata.is_file() {
                total_bytes = total_bytes.saturating_add(metadata.len());
                if let Some(limit) = max_bytes.filter(|limit| total_bytes > *limit) {
                    return Err(ArchiveError::new(
                    "expansion_limit_exceeded",
                    format!(
                        "Extraction produced {total_bytes} bytes, above the configured {limit}-byte limit"
                    ),
                ));
                }
            }
            if collect_entries {
                entries.push(LogicalEntry {
                    path: display.to_string(),
                    is_directory: metadata.is_dir(),
                    is_link,
                });
            }
            if metadata.is_dir() && !is_link {
                stack.push((path, entry_depth));
            }
        }
    }
    Ok((entries, total_bytes))
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
    quarantine_source: &Path,
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
                quarantine_source,
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
    install_file(source, &final_target, replace.as_deref(), quarantine_source)?;
    if let Some(existing) = replace {
        index.remove(&existing)?;
    }
    index.record(&final_target)?;
    summary.files_extracted += 1;
    Ok(())
}

fn install_file(
    source: &Path,
    target: &Path,
    existing: Option<&Path>,
    quarantine_source: &Path,
) -> Result<(), ArchiveError> {
    let parent = target.parent().ok_or_else(|| {
        ArchiveError::new(
            "invalid_destination",
            "Destination file had no parent folder",
        )
    })?;
    // Staging normally shares a volume with the destination, so the finished file is
    // moved into place. The copy below remains for when a move is not possible.
    quarantine::copy(quarantine_source, source).map_err(destination_error)?;
    match existing {
        None => {
            if move_without_copy(source, target).map_err(destination_error)? {
                return Ok(());
            }
        }
        Some(existing) => {
            let backup = unique_internal_path(parent, ".archive-app-backup-")?;
            fs::rename(existing, &backup).map_err(destination_error)?;
            if fs::rename(source, target).is_ok() {
                fs::remove_file(backup).map_err(destination_error)?;
                return Ok(());
            }
            fs::rename(&backup, existing).map_err(destination_error)?;
        }
    }
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
    quarantine::copy(quarantine_source, temporary.path()).map_err(destination_error)?;

    let backup = if let Some(existing) = existing {
        let backup = unique_internal_path(parent, ".archive-app-backup-")?;
        fs::rename(existing, &backup).map_err(destination_error)?;
        Some(backup)
    } else {
        None
    };
    let persisted = if existing.is_some() {
        temporary
            .persist(target)
            .map(|_| ())
            .map_err(|error| error.error)
    } else {
        persist_new(temporary, target)
    };
    match persisted {
        Ok(()) => {
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
            Err(destination_error(error))
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

fn prepare_preview_root(root: &Path) -> Result<(), ArchiveError> {
    if !root.is_absolute() {
        return Err(ArchiveError::new(
            "preview_unavailable",
            "The preview cache path is not absolute",
        ));
    }
    fs::create_dir_all(root).map_err(preview_io_error)?;
    let metadata = fs::symlink_metadata(root).map_err(preview_io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ArchiveError::new(
            "preview_unavailable",
            "The preview cache is not a real folder",
        ));
    }
    restrict_preview_directory(root)
}

fn has_executable_extension(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = name
        .rsplit_once('.')
        .map(|(_, value)| value.to_ascii_lowercase());
    extension.is_some_and(|extension| {
        matches!(
            extension.as_str(),
            "app"
                | "applescript"
                | "bat"
                | "bash"
                | "bin"
                | "bundle"
                | "cmd"
                | "com"
                | "command"
                | "csh"
                | "desktop"
                | "dmg"
                | "exe"
                | "fish"
                | "inetloc"
                | "jar"
                | "js"
                | "jse"
                | "ksh"
                | "mobileconfig"
                | "msi"
                | "msp"
                | "mpkg"
                | "php"
                | "pkg"
                | "pl"
                | "plugin"
                | "ps1"
                | "py"
                | "pyw"
                | "rb"
                | "run"
                | "scpt"
                | "scr"
                | "sh"
                | "url"
                | "vbs"
                | "webloc"
                | "workflow"
                | "wsf"
                | "zsh"
        )
    })
}

fn is_executable_payload(path: &Path, metadata: &fs::Metadata) -> Result<bool, ArchiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return Ok(true);
        }
    }
    let mut file = File::open(path).map_err(preview_io_error)?;
    let mut prefix = [0_u8; 8];
    let read = file.read(&mut prefix).map_err(preview_io_error)?;
    let prefix = &prefix[..read];
    Ok(prefix.starts_with(b"#!")
        || prefix.starts_with(b"MZ")
        || prefix.starts_with(b"\x7fELF")
        || matches!(
            prefix.get(..4),
            Some(
                [0xfe, 0xed, 0xfa, 0xce]
                    | [0xfe, 0xed, 0xfa, 0xcf]
                    | [0xce, 0xfa, 0xed, 0xfe]
                    | [0xcf, 0xfa, 0xed, 0xfe]
                    | [0xca, 0xfe, 0xba, 0xbe]
                    | [0xbe, 0xba, 0xfe, 0xca]
            )
        ))
}

fn executable_preview_error() -> ArchiveError {
    ArchiveError::new(
        "preview_executable_blocked",
        "Scripts, applications, and executable files cannot be opened from an archive",
    )
}

#[cfg(unix)]
fn restrict_preview_directory(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(preview_io_error)
}

#[cfg(not(unix))]
fn restrict_preview_directory(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_preview_file(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(preview_io_error)
}

#[cfg(not(unix))]
fn restrict_preview_file(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
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

fn preview_io_error(error: io::Error) -> ArchiveError {
    ArchiveError::new(
        "preview_unavailable",
        format!("Could not prepare the temporary preview: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static STAGING_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn preview_entry(path: &str, size: Option<u64>) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_string(),
            is_directory: false,
            size,
            packed_size: None,
            modified: None,
            encrypted: false,
            method: None,
            is_link: false,
            link_target: None,
            hard_link: false,
        }
    }

    #[test]
    fn preview_copy_is_private_scoped_and_executables_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let nested = staging.join("folder");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("note.txt"), "preview me").unwrap();
        let previews = root.path().join("Previews");
        let preview = persist_preview_file(
            &staging,
            "folder/note.txt",
            &previews,
            100 * 1024 * 1024,
            EntryCopy::Preview,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&preview).unwrap(), "preview me");
        assert!(preview.starts_with(&previews));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&preview).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        assert_eq!(
            validate_preview_entry(
                &preview_entry("run.command", Some(1)),
                100,
                EntryCopy::Preview
            )
            .unwrap_err()
            .code,
            "preview_executable_blocked"
        );
        assert_eq!(
            validate_preview_entry(
                &preview_entry("installer.pkg", Some(1)),
                100,
                EntryCopy::Preview
            )
            .unwrap_err()
            .code,
            "preview_executable_blocked"
        );
        fs::remove_dir_all(&staging).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("payload"), b"\xcf\xfa\xed\xfecontents").unwrap();
        assert_eq!(
            persist_preview_file(&staging, "payload", &previews, 100, EntryCopy::Preview)
                .unwrap_err()
                .code,
            "preview_executable_blocked"
        );

        let unrelated = previews.join("preview-unmarked");
        fs::create_dir(&unrelated).unwrap();
        assert_eq!(
            cleanup_stale_previews(&previews, Duration::ZERO).unwrap(),
            1
        );
        assert!(unrelated.exists());
        assert!(!preview.exists());
    }

    #[test]
    fn created_archive_replaces_only_a_confirmed_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("staged.zip");
        let output = root.path().join("output.zip");
        fs::write(&source, "new").unwrap();
        fs::write(&output, "old").unwrap();
        assert_eq!(
            install_created_archives(
                std::slice::from_ref(&source),
                &source,
                &output,
                false,
                false
            )
            .unwrap_err()
            .code,
            "output_exists"
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "old");
        install_created_archives(std::slice::from_ref(&source), &source, &output, true, false)
            .unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "new");
        // The finished archive was moved into place, not copied.
        assert!(!source.exists());

        fs::write(&source, "newer").unwrap();
        let folder = root.path().join("folder.zip");
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            install_created_archives(std::slice::from_ref(&source), &source, &folder, true, false)
                .unwrap_err()
                .code,
            "output_exists"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dragged_entries_follow_extraction_rules_not_preview_rules() {
        use std::os::unix::fs::PermissionsExt;

        let script = preview_entry("tools/run.sh", Some(5_000));
        assert_eq!(
            validate_preview_entry(&script, 100, EntryCopy::Preview)
                .unwrap_err()
                .code,
            "preview_size_exceeded"
        );
        validate_preview_entry(&script, 10_000, EntryCopy::DragOut).unwrap();
        validate_preview_entry(&preview_entry("stream", None), 100, EntryCopy::DragOut).unwrap();
        assert_eq!(
            validate_preview_entry(&script, 100, EntryCopy::DragOut)
                .unwrap_err()
                .code,
            "expansion_limit_exceeded"
        );
        let mut link = preview_entry("link", Some(1));
        link.is_link = true;
        assert_eq!(
            validate_preview_entry(&link, 100, EntryCopy::DragOut)
                .unwrap_err()
                .code,
            "unsafe_link"
        );

        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let previews = root.path().join("previews");
        fs::create_dir_all(staging.join("tools")).unwrap();
        let source = staging.join("tools/run.sh");
        fs::write(&source, "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            persist_preview_file(
                &staging,
                "tools/run.sh",
                &previews,
                1024,
                EntryCopy::Preview
            )
            .unwrap_err()
            .code,
            "preview_executable_blocked"
        );
        let dragged = persist_preview_file(
            &staging,
            "tools/run.sh",
            &previews,
            1024,
            EntryCopy::DragOut,
        )
        .unwrap();
        assert_eq!(dragged.file_name().unwrap(), "run.sh");
        assert_eq!(
            fs::metadata(&dragged).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            persist_preview_file(&staging, "tools/run.sh", &previews, 4, EntryCopy::DragOut)
                .unwrap_err()
                .code,
            "preview_size_exceeded"
        );
    }

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

    #[cfg(unix)]
    #[test]
    fn staged_files_are_moved_into_place_and_staging_beside_a_target_is_cleaned_up() {
        use std::os::unix::fs::MetadataExt;
        let _guard = STAGING_TEST_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let archive = root.path().join("source.zip");
        fs::write(&archive, "archive").unwrap();

        // Staging placed beside the destination, as it is for another volume.
        let staging = StagingDirectory::build(Some(&destination)).unwrap();
        let hidden = staging.path().parent().unwrap().to_path_buf();
        assert_eq!(hidden.parent().unwrap(), destination);
        fs::create_dir(staging.path().join("folder")).unwrap();
        let staged = staging.path().join("folder/file.txt");
        fs::write(&staged, "payload").unwrap();
        fs::write(staging.path().join("existing.txt"), "new").unwrap();
        fs::write(destination.join("existing.txt"), "old").unwrap();
        let inode = fs::metadata(&staged).unwrap().ino();

        let summary = commit_staging(
            staging.path(),
            &destination,
            &archive,
            ConflictPolicy::Replace,
        )
        .unwrap();
        assert_eq!(summary.files_extracted, 2);
        let installed = destination.join("folder/file.txt");
        assert_eq!(fs::metadata(&installed).unwrap().ino(), inode);
        assert!(!staged.exists());
        assert_eq!(
            fs::read_to_string(destination.join("existing.txt")).unwrap(),
            "new"
        );
        drop(staging);
        assert!(!hidden.exists());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 2);

        // After a crash the hidden folder is found through its record and removed.
        let crashed = StagingDirectory::build(Some(&destination)).unwrap();
        let hidden = crashed.path().parent().unwrap().to_path_buf();
        fs::write(crashed.path().join("partial.bin"), "partial").unwrap();
        let StagingDirectory {
            _lock,
            _directory,
            _external,
            ..
        } = crashed;
        let record = _directory.keep();
        let _ = _external.unwrap().keep();
        drop(_lock);
        cleanup_stale_staging().unwrap();
        assert!(!hidden.exists());
        assert!(!record.exists());

        // A record cannot be used to delete a folder that is not Archi's.
        let innocent = destination.join("folder");
        let forged = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .unwrap();
        File::create(forged.path().join(STAGING_LOCK)).unwrap();
        fs::write(
            forged.path().join(EXTERNAL_LOCATION),
            innocent.to_str().unwrap(),
        )
        .unwrap();
        let _ = forged.keep();
        cleanup_stale_staging().unwrap();
        assert!(innocent.join("file.txt").exists());

        assert!(validate_archive_entries(&[preview_entry(
            ".archive-app-extract-forged/file.txt",
            Some(1)
        )])
        .is_err());
    }

    #[test]
    fn replacing_a_split_archive_retires_the_whole_old_set() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let out = root.path().join("out");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&out).unwrap();
        let base = work.join("set.zip");
        let output = out.join("set.zip");
        let build = |parts: usize, tag: &str| {
            (1..=parts)
                .map(|number| {
                    let path = work.join(format!("set.zip.{number:03}"));
                    fs::write(&path, format!("{tag}-{number}")).unwrap();
                    path
                })
                .collect::<Vec<_>>()
        };
        // An old five-part set, an unrelated neighbour, and the single file the save
        // panel offered to replace.
        for number in 1..=5 {
            fs::write(out.join(format!("set.zip.{number:03}")), "old").unwrap();
        }
        fs::write(out.join("set.zip.notes"), "keep").unwrap();
        fs::write(&output, "old single").unwrap();
        assert_eq!(existing_volumes(&output).unwrap().len(), 5);

        let volumes = build(2, "new");
        assert_eq!(
            install_created_archives(&volumes, &base, &output, true, false)
                .unwrap_err()
                .code,
            "output_exists"
        );
        assert_eq!(fs::read_to_string(out.join("set.zip.001")).unwrap(), "old");

        let installed = install_created_archives(&volumes, &base, &output, true, true).unwrap();
        assert_eq!(installed.len(), 2);
        assert_eq!(
            fs::read_to_string(out.join("set.zip.002")).unwrap(),
            "new-2"
        );
        let mut names = fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["set.zip.001", "set.zip.002", "set.zip.notes"]);

        // If the new set cannot be installed, the old one is put back.
        let mut broken = build(2, "newer");
        broken.push(work.join("set.zip.003"));
        assert!(install_created_archives(&broken, &base, &output, false, true).is_err());
        assert_eq!(
            fs::read_to_string(out.join("set.zip.001")).unwrap(),
            "new-1"
        );
        assert_eq!(
            fs::read_to_string(out.join("set.zip.002")).unwrap(),
            "new-2"
        );
        assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
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

    #[test]
    fn archive_rewrite_replaces_atomically_and_detects_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.zip");
        fs::write(&source, "original").unwrap();
        let rewrite = ArchiveRewrite::create(&source).unwrap();
        fs::write(rewrite.path(), "replacement").unwrap();
        rewrite.commit().unwrap();
        assert_eq!(fs::read_to_string(&source).unwrap(), "replacement");

        let rewrite = ArchiveRewrite::create(&source).unwrap();
        let recovery = rewrite.path().to_path_buf();
        fs::write(rewrite.path(), "candidate").unwrap();
        fs::write(&source, "external!!!").unwrap();
        let error = rewrite.commit().unwrap_err();
        assert_eq!(error.code, "archive_changed");
        assert_eq!(fs::read_to_string(&source).unwrap(), "external!!!");
        assert_eq!(fs::read_to_string(recovery).unwrap(), "candidate");
    }

    #[test]
    fn extraction_limits_use_actual_files_and_reject_deep_paths() {
        let staging = tempfile::tempdir().unwrap();
        fs::write(staging.path().join("payload.bin"), [0_u8; 8]).unwrap();
        assert_eq!(
            enforce_staging_limits(staging.path(), Some(7))
                .unwrap_err()
                .code,
            "expansion_limit_exceeded"
        );

        let deep = format!(
            "{}/payload.txt",
            vec!["folder"; MAX_ARCHIVE_DEPTH].join("/")
        );
        assert_eq!(
            validate_archive_entries(&[preview_entry(&deep, Some(1))])
                .unwrap_err()
                .code,
            "path_depth_exceeded"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_survives_rewrite_preview_and_staged_commit() {
        let marker = b"0081;66d00000;Archi;00000000-0000-0000-0000-000000000000";
        let root = tempfile::tempdir().unwrap();

        let archive = root.path().join("source.zip");
        fs::write(&archive, "original").unwrap();
        quarantine::write(&archive, marker).unwrap();
        let rewrite = ArchiveRewrite::create(&archive).unwrap();
        assert_eq!(
            quarantine::read(rewrite.path()).unwrap().as_deref(),
            Some(marker.as_slice())
        );
        fs::write(rewrite.path(), "replacement").unwrap();
        rewrite.commit().unwrap();
        assert_eq!(
            quarantine::read(&archive).unwrap().as_deref(),
            Some(marker.as_slice())
        );

        let staging = root.path().join("preview-staging");
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("note.txt"), "preview").unwrap();
        quarantine::write(&staging, marker).unwrap();
        let preview_root = root.path().join("previews");
        let preview = persist_preview_file(
            &staging,
            "note.txt",
            &preview_root,
            1024,
            EntryCopy::Preview,
        )
        .unwrap();
        assert_eq!(
            quarantine::read(&preview).unwrap().as_deref(),
            Some(marker.as_slice())
        );

        let staging = root.path().join("commit-staging");
        let destination = root.path().join("destination");
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(staging.join("payload.txt"), "payload").unwrap();
        quarantine::write(&staging, marker).unwrap();
        commit_staging(&staging, &destination, &archive, ConflictPolicy::KeepBoth).unwrap();
        assert_eq!(
            quarantine::read(&destination.join("payload.txt"))
                .unwrap()
                .as_deref(),
            Some(marker.as_slice())
        );
    }
}
