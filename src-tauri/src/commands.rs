use crate::{
    archive::{self, ArchiveEntry, ArchiveError, ArchiveFormat, CompressionLevel},
    browser::{ArchiveDocument, ArchiveStore, EntryPage, SortKey},
    jobs::{self, JobManager, JobSnapshot},
    safe_paths::{self, CommitSummary, ConflictPolicy, StagingDirectory},
    settings::{LocalData, Settings, MAX_EXPANDED_BYTES, MIN_EXPANDED_BYTES},
    shell_requests::{ShellIntegrationStatus, ShellRequest, ShellRequestStore},
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use zeroize::Zeroizing;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtractResult {
    destination: String,
    files_extracted: usize,
    files_skipped: usize,
    renamed: usize,
    elapsed_ms: u64,
    warning_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestResult {
    path: String,
    elapsed_ms: u64,
    warning_count: usize,
}

#[tauri::command]
pub(crate) async fn open_archive(
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    password: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let password = password.map(Zeroizing::new);
        let archive_path = PathBuf::from(&path);
        let engine = data.engine_path();
        let entries = archive::list_archive(
            engine,
            &archive_path,
            password.as_ref().map(|value| value.as_str()),
        )?;
        archives.install(path, data.engine_version().to_string(), entries)
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command parameters are the IPC contract.
pub(crate) async fn start_extract(
    jobs: tauri::State<'_, JobManager>,
    data: tauri::State<'_, LocalData>,
    path: String,
    destination: String,
    conflict_policy: ConflictPolicy,
    entries: Option<Vec<String>>,
    password: Option<String>,
    max_expanded_bytes: u64,
    allow_unbounded: bool,
) -> Result<ExtractResult, ArchiveError> {
    let jobs = jobs.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("extract", 0)?;
        let result = (|| {
            let password = password.map(Zeroizing::new);
            let password = password.as_ref().map(|value| value.as_str());
            let archive_path = PathBuf::from(&path);
            let destination_path = PathBuf::from(&destination);
            let engine = data.engine_path();
            let archive_entries = archive::list_archive(engine, &archive_path, password)?;
            safe_paths::validate_archive_entries(&archive_entries)?;
            let selected = validate_selection(entries.unwrap_or_default(), &archive_entries)?;
            let total = validate_expansion(
                &selected,
                &archive_entries,
                max_expanded_bytes,
                allow_unbounded,
            )?;
            control.set_total_bytes(total);
            let staging = StagingDirectory::create()?;
            let outcome = jobs::run_extract(
                &control,
                engine,
                &archive_path,
                staging.path(),
                &selected,
                password,
            )?;
            let CommitSummary {
                files_extracted,
                files_skipped,
                renamed,
            } = safe_paths::commit_staging(
                staging.path(),
                &destination_path,
                &archive_path,
                conflict_policy,
            )?;
            Ok(ExtractResult {
                destination,
                files_extracted,
                files_skipped,
                renamed,
                elapsed_ms: outcome.elapsed_ms,
                warning_count: outcome.warning_count,
            })
        })();
        result
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_archive(
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    inputs: Vec<String>,
    output: String,
    format: ArchiveFormat,
    compression: CompressionLevel,
    password: Option<String>,
    password_confirmation: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("create", 0)?;
        let result = (|| {
            let password = password.map(Zeroizing::new);
            let confirmation = password_confirmation.map(Zeroizing::new);
            let password = confirmed_password(&password, &confirmation)?;
            let output_path = PathBuf::from(&output);
            format.validate_output(&output_path)?;
            let plan = archive::prepare_creation(
                &inputs.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                &output_path,
            )?;
            control.set_total_bytes(plan.total_bytes);
            let engine = data.engine_path();
            let working = tempfile::Builder::new()
                .prefix("archive-app-create-")
                .tempdir()
                .map_err(|error| {
                    ArchiveError::new(
                        "staging_failed",
                        format!("Could not create archive workspace: {error}"),
                    )
                })?;
            let temporary = working.path().join(match format {
                ArchiveFormat::Zip => "archive.zip",
                ArchiveFormat::SevenZip => "archive.7z",
            });
            jobs::run_create(
                &control,
                engine,
                &temporary,
                &plan,
                format,
                compression,
                password,
            )?;
            archive::test_archive(engine, &temporary, password)?;
            let entries = archive::list_archive(engine, &temporary, password)?;
            safe_paths::validate_archive_entries(&entries)?;
            safe_paths::install_created_archive(&temporary, &output_path)?;
            let mut document =
                archives.install(output, data.engine_version().to_string(), entries)?;
            document.skipped_links = plan.skipped_links;
            Ok(document)
        })();
        result
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) async fn test_archive(
    jobs: tauri::State<'_, JobManager>,
    data: tauri::State<'_, LocalData>,
    path: String,
    password: Option<String>,
) -> Result<TestResult, ArchiveError> {
    let jobs = jobs.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("test", 0)?;
        let result = (|| {
            let password = password.map(Zeroizing::new);
            let password = password.as_ref().map(|value| value.as_str());
            let archive_path = PathBuf::from(&path);
            let engine = data.engine_path();
            let entries = archive::list_archive(engine, &archive_path, password)?;
            control.set_total_bytes(entries.iter().filter_map(|entry| entry.size).sum());
            let outcome = jobs::run_test(&control, engine, &archive_path, password)?;
            Ok(TestResult {
                path,
                elapsed_ms: outcome.elapsed_ms,
                warning_count: outcome.warning_count,
            })
        })();
        result
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) fn job_status(
    jobs: tauri::State<'_, JobManager>,
) -> Result<Option<JobSnapshot>, ArchiveError> {
    jobs.status()
}

#[tauri::command]
pub(crate) fn cancel_job(jobs: tauri::State<'_, JobManager>) -> Result<bool, ArchiveError> {
    jobs.cancel()
}

#[tauri::command]
pub(crate) fn entry_icons(keys: Vec<String>) -> HashMap<String, String> {
    #[cfg(target_os = "macos")]
    return crate::macos_services::icons(keys);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = keys;
        HashMap::new()
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn entry_page(
    archives: tauri::State<'_, ArchiveStore>,
    path: String,
    folder: String,
    query: String,
    sort: SortKey,
    descending: bool,
    page: usize,
    page_size: usize,
    show_hidden: bool,
) -> Result<EntryPage, ArchiveError> {
    let archives = archives.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        archives.page(
            &path,
            &folder,
            &query,
            sort,
            descending,
            page,
            page_size,
            show_hidden,
        )
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) fn archive_changed(
    archives: tauri::State<'_, ArchiveStore>,
    path: String,
) -> Result<bool, ArchiveError> {
    archives.changed(&path)
}

#[tauri::command]
pub(crate) fn get_settings(data: tauri::State<'_, LocalData>) -> Result<Settings, ArchiveError> {
    data.load()
}

#[tauri::command]
pub(crate) fn save_settings(
    data: tauri::State<'_, LocalData>,
    settings: Settings,
) -> Result<Settings, ArchiveError> {
    data.save(settings)
}

#[tauri::command]
pub(crate) fn reset_settings(data: tauri::State<'_, LocalData>) -> Result<Settings, ArchiveError> {
    data.reset()
}

#[tauri::command]
pub(crate) fn record_diagnostic(
    data: tauri::State<'_, LocalData>,
    event: String,
    code: Option<String>,
) -> Result<(), ArchiveError> {
    data.record(&event, code.as_deref())
}

#[tauri::command]
pub(crate) fn clear_diagnostics(data: tauri::State<'_, LocalData>) -> Result<(), ArchiveError> {
    data.clear_diagnostics()
}

#[tauri::command]
pub(crate) fn export_diagnostics(
    data: tauri::State<'_, LocalData>,
    destination: String,
) -> Result<(), ArchiveError> {
    data.export_diagnostics(Path::new(&destination))
}

#[tauri::command]
pub(crate) fn open_destination(path: String) -> Result<(), ArchiveError> {
    let path = PathBuf::from(path).canonicalize().map_err(|error| {
        ArchiveError::new(
            "invalid_destination",
            format!("Could not resolve the extraction destination: {error}"),
        )
    })?;
    if !path.is_dir() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The extraction destination is not a folder",
        ));
    }
    platform_open(&path)
}

#[tauri::command]
pub(crate) fn take_shell_requests(
    requests: tauri::State<'_, ShellRequestStore>,
) -> Result<Vec<ShellRequest>, ArchiveError> {
    requests.take()
}

#[tauri::command]
pub(crate) fn shell_integration_status(
    requests: tauri::State<'_, ShellRequestStore>,
) -> ShellIntegrationStatus {
    requests.status()
}

#[tauri::command]
pub(crate) fn default_zip_output(inputs: Vec<String>) -> Result<String, ArchiveError> {
    let inputs = inputs.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    let first = inputs
        .first()
        .ok_or_else(|| ArchiveError::new("no_inputs", "Choose at least one item to compress"))?;
    if inputs
        .iter()
        .any(|path| !path.is_absolute() || !path.exists())
    {
        return Err(ArchiveError::new(
            "invalid_source",
            "Compression inputs must be absolute existing paths",
        ));
    }
    let parent = first
        .parent()
        .ok_or_else(|| ArchiveError::new("invalid_source", "Compression input has no folder"))?
        .canonicalize()
        .map_err(|error| ArchiveError::new("invalid_source", error.to_string()))?;
    if inputs.iter().any(|path| {
        path.parent()
            .and_then(|value| value.canonicalize().ok())
            .is_none_or(|value| value != parent)
    }) {
        return Err(ArchiveError::new(
            "invalid_source",
            "Finder selections must come from the same folder",
        ));
    }
    let base = if inputs.len() == 1 {
        let name = first
            .file_name()
            .ok_or_else(|| ArchiveError::new("invalid_source", "Input has no name"))?
            .to_string_lossy();
        if first
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            format!(
                "{} (archive).zip",
                first
                    .file_stem()
                    .ok_or_else(|| ArchiveError::new("invalid_source", "Input has no name"))?
                    .to_string_lossy()
            )
        } else {
            format!("{name}.zip")
        }
    } else {
        "Archive.zip".to_string()
    };
    let output = unique_output(&parent, &base)?;
    Ok(output.to_string_lossy().into_owned())
}

fn unique_output(parent: &Path, base: &str) -> Result<PathBuf, ArchiveError> {
    let candidate = parent.join(base);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let stem = base.strip_suffix(".zip").unwrap_or(base);
    for number in 2..=10_000 {
        let candidate = parent.join(format!("{stem} ({number}).zip"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ArchiveError::new(
        "output_exists",
        "Could not find an available ZIP filename",
    ))
}

fn confirmed_password<'a>(
    password: &'a Option<Zeroizing<String>>,
    confirmation: &Option<Zeroizing<String>>,
) -> Result<Option<&'a str>, ArchiveError> {
    match (password.as_ref(), confirmation.as_ref()) {
        (None, None) => Ok(None),
        (Some(password), Some(_confirmation)) if password.is_empty() => Err(ArchiveError::new(
            "empty_password",
            "Encrypted archives require a non-empty password",
        )),
        (Some(password), Some(confirmation)) if password.as_str() == confirmation.as_str() => {
            Ok(Some(password.as_str()))
        }
        _ => Err(ArchiveError::new(
            "password_mismatch",
            "Password and confirmation do not match",
        )),
    }
}

fn validate_selection(
    selected: Vec<String>,
    entries: &[ArchiveEntry],
) -> Result<Vec<String>, ArchiveError> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let mut unique = HashSet::new();
    for path in selected {
        let exact = entries.iter().find(|entry| entry.path == path);
        let prefix = format!("{path}/");
        let descendants = entries
            .iter()
            .filter(|entry| entry.path.starts_with(&prefix))
            .collect::<Vec<_>>();
        if exact.is_none() && descendants.is_empty() {
            return Err(ArchiveError::new(
                "entry_not_found",
                format!("Archive entry was not found: {path}"),
            ));
        }
        if exact.is_some_and(|entry| !entry.is_directory) || descendants.is_empty() {
            unique.insert(path);
        } else {
            unique.extend(descendants.into_iter().map(|entry| entry.path.clone()));
        }
    }
    let mut selected = unique.into_iter().collect::<Vec<_>>();
    selected.sort();
    Ok(selected)
}

fn selected_total(selected: &[String], entries: &[ArchiveEntry]) -> u64 {
    entries
        .iter()
        .filter(|entry| is_selected(entry, selected))
        .filter_map(|entry| entry.size)
        .fold(0, u64::saturating_add)
}

fn is_selected(entry: &ArchiveEntry, selected: &[String]) -> bool {
    selected.is_empty()
        || selected.iter().any(|path| {
            entry.path == *path
                || entry
                    .path
                    .strip_prefix(path)
                    .is_some_and(|suffix| suffix.starts_with(['/', '\\']))
        })
}

fn validate_expansion(
    selected: &[String],
    entries: &[ArchiveEntry],
    limit: u64,
    allow_unbounded: bool,
) -> Result<u64, ArchiveError> {
    if !(MIN_EXPANDED_BYTES..=MAX_EXPANDED_BYTES).contains(&limit) {
        return Err(ArchiveError::new(
            "invalid_expansion_limit",
            "The extraction size limit is invalid",
        ));
    }
    let total = selected_total(selected, entries);
    let unknown = entries
        .iter()
        .filter(|entry| !entry.is_directory && is_selected(entry, selected) && entry.size.is_none())
        .count();
    if !allow_unbounded && unknown > 0 {
        return Err(ArchiveError::new(
            "expansion_size_unknown",
            format!(
                "{unknown} selected archive entries do not declare an expanded size. The configured limit cannot fully protect this extraction."
            ),
        ));
    }
    if !allow_unbounded && total > limit {
        return Err(ArchiveError::new(
            "expansion_limit_exceeded",
            format!(
                "The archive declares {total} expanded bytes, above the configured {limit}-byte limit."
            ),
        ));
    }
    Ok(total)
}

fn platform_open(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ArchiveError::new(
                "open_destination_failed",
                format!("Could not open the extraction destination: {error}"),
            )
        })?;
    Ok(())
}

fn task_error(error: tauri::Error) -> ArchiveError {
    ArchiveError::new("internal_error", format!("Archive task failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_confirmation_selected_totals_and_expansion_limits_are_enforced() {
        let password = Some(Zeroizing::new("secret".to_string()));
        let wrong = Some(Zeroizing::new("different".to_string()));
        assert_eq!(
            confirmed_password(&password, &wrong).unwrap_err().code,
            "password_mismatch"
        );
        let entries = vec![
            ArchiveEntry {
                path: "folder".to_string(),
                is_directory: true,
                size: Some(0),
                packed_size: None,
                modified: None,
                encrypted: false,
                method: None,
                is_link: false,
                link_target: None,
            },
            ArchiveEntry {
                path: "folder/file.txt".to_string(),
                is_directory: false,
                size: Some(42),
                packed_size: None,
                modified: None,
                encrypted: false,
                method: None,
                is_link: false,
                link_target: None,
            },
        ];
        assert_eq!(selected_total(&["folder".to_string()], &entries), 42);
        assert_eq!(
            validate_selection(vec!["folder".to_string()], &entries).unwrap(),
            ["folder/file.txt"]
        );
        assert_eq!(
            validate_selection(vec!["missing".to_string()], &entries)
                .unwrap_err()
                .code,
            "entry_not_found"
        );
        assert_eq!(
            validate_expansion(&[], &entries, MIN_EXPANDED_BYTES, false).unwrap(),
            42
        );
        let mut huge = entries[1].clone();
        huge.size = Some(MIN_EXPANDED_BYTES + 1);
        assert_eq!(
            validate_expansion(&[], &[huge], MIN_EXPANDED_BYTES, false)
                .unwrap_err()
                .code,
            "expansion_limit_exceeded"
        );
        let mut unknown = entries[1].clone();
        unknown.size = None;
        assert_eq!(
            validate_expansion(&[], &[unknown.clone()], MIN_EXPANDED_BYTES, false)
                .unwrap_err()
                .code,
            "expansion_size_unknown"
        );
        validate_expansion(&[], &[unknown], MIN_EXPANDED_BYTES, true).unwrap();
    }

    #[test]
    fn finder_zip_output_is_sensible_and_does_not_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("one.txt");
        let second = root.path().join("two.txt");
        std::fs::write(&first, "one").unwrap();
        std::fs::write(&second, "two").unwrap();
        let inputs = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        let canonical = root.path().canonicalize().unwrap();
        assert_eq!(
            default_zip_output(inputs.clone()).unwrap(),
            canonical.join("Archive.zip").to_string_lossy()
        );
        std::fs::write(root.path().join("Archive.zip"), "existing").unwrap();
        assert_eq!(
            default_zip_output(inputs).unwrap(),
            canonical.join("Archive (2).zip").to_string_lossy()
        );

        let archive = root.path().join("existing.zip");
        std::fs::write(&archive, "archive").unwrap();
        assert_eq!(
            default_zip_output(vec![archive.to_string_lossy().into_owned()]).unwrap(),
            canonical.join("existing (archive).zip").to_string_lossy()
        );
    }
}
