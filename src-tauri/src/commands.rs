use crate::{
    archive::{self, ArchiveEntry, ArchiveError, ArchiveFormat, CompressionLevel},
    browser::{ArchiveDocument, ArchiveFolder, ArchiveStore, EntryPage, SortKey},
    jobs::{self, JobManager, JobSnapshot},
    safe_paths::{self, CommitSummary, ConflictPolicy, EntryCopy, StagingDirectory},
    settings::{LocalData, Settings, MAX_EXPANDED_BYTES, MIN_EXPANDED_BYTES},
    shell_requests::{ShellIntegrationStatus, ShellRequest, ShellRequestStore},
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tauri::Manager;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtractResult {
    destination: String,
    files_extracted: usize,
    files_skipped: usize,
    renamed: usize,
    links_skipped: usize,
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
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    password: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Listing a large archive or tarball can take a while, so it runs as a job
        // the user can see and cancel.
        let control = jobs.start("open", 0)?;
        let password = password.map(Zeroizing::new);
        let archive_path = PathBuf::from(&path);
        let engine = data.engine_path();
        let entries = jobs::run_inspect(
            &control,
            engine,
            &archive_path,
            password.as_ref().map(|value| value.as_str()),
        )?
        .entries;
        let mut document = archives.install(path, data.engine_version().to_string(), entries)?;
        document.comment = archive::read_zip_comment(&archive_path).ok().flatten();
        let _ = data.record_recent(&archive_path);
        Ok(document)
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
    enclose: bool,
) -> Result<ExtractResult, ArchiveError> {
    let jobs = jobs.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("extract", 0)?;
        let result = (|| {
            let password = password.map(Zeroizing::new);
            let password = password.as_ref().map(|value| value.as_str());
            let archive_path = PathBuf::from(&path);
            let mut destination_path = PathBuf::from(&destination);
            let engine = data.engine_path();
            let listing = jobs::run_inspect(&control, engine, &archive_path, password)?;
            let archive_entries = listing.entries;
            safe_paths::validate_archive_entries(&archive_entries)?;
            let selected = validate_selection(entries.unwrap_or_default(), &archive_entries)?;
            let LinkPlan {
                paths: selected,
                links_skipped,
                hard_links,
                extras,
            } = without_links(selected, &archive_entries)?;
            let plan = archive::prepare_extraction(&selected, listing.layout)?;
            let total = validate_expansion(
                &selected,
                &archive_entries,
                max_expanded_bytes,
                allow_unbounded,
            )?;
            control.set_total_bytes(total);
            let staging = StagingDirectory::create_near(&destination_path)?;
            let outcome = jobs::run_extract(
                &control,
                engine,
                &archive_path,
                staging.path(),
                &plan,
                password,
                (!allow_unbounded).then_some(max_expanded_bytes),
            )?;
            if !hard_links.is_empty() {
                safe_paths::materialize_hard_links(staging.path(), &hard_links, &extras)?;
                safe_paths::enforce_staging_limits(
                    staging.path(),
                    (!allow_unbounded).then_some(max_expanded_bytes),
                )?;
            }
            let CommitSummary {
                files_extracted,
                files_skipped,
                renamed,
            } = {
                if enclose {
                    destination_path =
                        enclosing_destination(staging.path(), &destination_path, &archive_path)?;
                }
                safe_paths::commit_staging(
                    staging.path(),
                    &destination_path,
                    &archive_path,
                    conflict_policy,
                )?
            };
            Ok(ExtractResult {
                destination: destination_path.to_string_lossy().into_owned(),
                files_extracted,
                files_skipped,
                renamed,
                links_skipped,
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
pub(crate) async fn open_archive_entry(
    app: tauri::AppHandle,
    jobs: tauri::State<'_, JobManager>,
    data: tauri::State<'_, LocalData>,
    path: String,
    entry: String,
    password: Option<String>,
    quick_look: bool,
) -> Result<(), ArchiveError> {
    let jobs = jobs.inner().clone();
    let data = data.inner().clone();
    let preview_root = preview_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let preview = prepare_preview_entry(
            jobs,
            data,
            path,
            entry,
            password,
            preview_root,
            EntryCopy::Preview,
        )?;
        let launched = if quick_look {
            #[cfg(target_os = "macos")]
            {
                crate::macos_services::quick_look(&preview)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(ArchiveError::new(
                    "preview_unsupported",
                    "Native Quick Look is available only on macOS",
                ))
            }
        } else {
            platform_open(&preview)
        };
        if let Err(error) = launched {
            if let Some(directory) = preview.parent() {
                let _ = fs::remove_dir_all(directory);
            }
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) async fn prepare_archive_entry(
    app: tauri::AppHandle,
    jobs: tauri::State<'_, JobManager>,
    data: tauri::State<'_, LocalData>,
    path: String,
    entry: String,
    password: Option<String>,
) -> Result<String, ArchiveError> {
    let jobs = jobs.inner().clone();
    let data = data.inner().clone();
    let preview_root = preview_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        prepare_preview_entry(
            jobs,
            data,
            path,
            entry,
            password,
            preview_root,
            EntryCopy::DragOut,
        )
        .map(|preview| preview.to_string_lossy().into_owned())
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
    volume_size: Option<u64>,
    password: Option<String>,
    password_confirmation: Option<String>,
    replace_existing: bool,
    open_result: bool,
    replace_volumes: bool,
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
            archive::validate_volume_size(volume_size)?;
            // Found before compressing anything, so the user can decide up front.
            if volume_size.is_some() && !replace_volumes {
                let existing = safe_paths::existing_volumes(&output_path)?;
                if let Some(first) = existing.first() {
                    return Err(ArchiveError::new(
                        "volumes_exist",
                        format!(
                            "A split archive with this name is already in the folder: {} {}, starting with {}.",
                            existing.len(),
                            if existing.len() == 1 { "file" } else { "files" },
                            first.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    ));
                }
            }
            let plan = archive::prepare_creation_with(
                &inputs.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                &output_path,
                replace_existing,
            )?;
            format.validate_creation_options(&plan, volume_size, password)?;
            control.set_total_bytes(plan.total_bytes);
            let engine = data.engine_path();
            // Built beside the output so the finished archive is moved, not copied, into place.
            let working = StagingDirectory::create_near(&output_path)?;
            let temporary = working.path().join(output_path.file_name().ok_or_else(|| {
                ArchiveError::new("invalid_destination", "Archive output has no file name")
            })?);
            match format {
                ArchiveFormat::TarGzip | ArchiveFormat::TarXz | ArchiveFormat::TarZstd => {
                    let tar = working.path().join("payload.tar");
                    jobs::run_create_tar(&control, engine, &tar, &plan, compression)?;
                    control.set_total_bytes(
                        fs::metadata(&tar)
                            .map_err(|error| {
                                ArchiveError::new(
                                    "creation_failed",
                                    format!("Could not inspect staged TAR: {error}"),
                                )
                            })?
                            .len(),
                    );
                    if format == ArchiveFormat::TarZstd {
                        jobs::run_zstd(&control, &tar, &temporary, compression)?;
                    } else {
                        jobs::run_compress_stream(
                            &control,
                            engine,
                            &temporary,
                            &tar,
                            format,
                            compression,
                        )?;
                    }
                }
                ArchiveFormat::Zstd => {
                    let source = plan.single_file().ok_or_else(|| {
                        ArchiveError::new(
                            "stream_requires_file",
                            "Zstandard streams require exactly one regular file",
                        )
                    })?;
                    jobs::run_zstd(&control, &source, &temporary, compression)?;
                }
                _ => {
                    jobs::run_create(
                        &control,
                        engine,
                        &temporary,
                        &plan,
                        format,
                        compression,
                        volume_size,
                        password,
                    )?;
                }
            }
            let generated = archive::generated_archive_paths(&temporary, volume_size)?;
            let first_volume = generated[0].clone();
            archive::test_archive(engine, &first_volume, password)?;
            let entries = jobs::run_inspect(&control, engine, &first_volume, password)?.entries;
            safe_paths::validate_archive_entries(&entries)?;
            let installed = safe_paths::install_created_archives(
                &generated,
                &temporary,
                &output_path,
                replace_existing,
                replace_volumes,
            )?;
            let installed_path = installed[0].to_string_lossy().into_owned();
            // A quick compress from Finder must not replace the archive being browsed.
            let mut document = if open_result {
                archives.install(installed_path, data.engine_version().to_string(), entries)?
            } else {
                ArchiveStore::describe(&installed_path, data.engine_version(), &entries)
            };
            document.skipped_links = plan.skipped_links;
            document.comment = archive::read_zip_comment(Path::new(&document.path))
                .ok()
                .flatten();
            let _ = data.record_recent(Path::new(&document.path));
            Ok(document)
        })();
        result
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command parameters are the IPC contract.
pub(crate) async fn add_to_archive(
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    inputs: Vec<String>,
    compression: CompressionLevel,
    password: Option<String>,
    folder: String,
    replace_existing: bool,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("modify", 0)?;
        let password = password.map(Zeroizing::new);
        let password = password.as_ref().map(|value| value.as_str());
        let archive_path = PathBuf::from(&path);
        let format = require_writable_format(&archive_path)?;
        let original_entries = archive::list_archive(data.engine_path(), &archive_path, password)?;
        safe_paths::validate_archive_entries(&original_entries)?;
        let plan = archive::prepare_addition(
            &inputs.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
            &archive_path,
            folder.trim_matches('/'),
        )?;
        check_addition(&plan, &original_entries, replace_existing)?;
        control.set_total_bytes(plan.total_bytes);
        let rewrite = safe_paths::ArchiveRewrite::create(&archive_path)?;
        let candidate = (|| {
            jobs::run_create(
                &control,
                data.engine_path(),
                rewrite.path(),
                &plan,
                format,
                compression,
                None,
                password,
            )?;
            archive::test_archive(data.engine_path(), rewrite.path(), password)?;
            let entries = archive::list_archive(data.engine_path(), rewrite.path(), password)?;
            safe_paths::validate_archive_entries(&entries)?;
            let stored = entries
                .iter()
                .map(|entry| normalized(&entry.path))
                .collect::<HashSet<_>>();
            if plan
                .names
                .iter()
                .any(|name| !stored.contains(&normalized(name)))
            {
                return Err(ArchiveError::new(
                    "add_failed",
                    "The archive engine did not store every added item where it was expected.",
                ));
            }
            Ok(entries)
        })()
        .map_err(|error| rewrite.recovery_error(error))?;
        rewrite.commit()?;
        install_modified_document(&archives, &data, path, candidate)
    })
    .await
    .map_err(task_error)?
}

// File systems and archives may spell the same name with different Unicode composition.
fn normalized(path: &str) -> String {
    path.nfc().collect()
}

#[tauri::command]
pub(crate) async fn delete_archive_entries(
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    entries: Vec<String>,
    password: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("modify", 0)?;
        let password = password.map(Zeroizing::new);
        let password = password.as_ref().map(|value| value.as_str());
        let archive_path = PathBuf::from(&path);
        require_writable_format(&archive_path)?;
        let original = archive::list_archive(data.engine_path(), &archive_path, password)?;
        safe_paths::validate_archive_entries(&original)?;
        let selected = modification_selection(&entries, &original)?;
        control.set_total_bytes(selected_total(&selected, &original));
        let plan = archive::prepare_deletion(&selected)?;
        let rewrite = safe_paths::ArchiveRewrite::create(&archive_path)?;
        let candidate = (|| {
            jobs::run_delete(
                &control,
                data.engine_path(),
                rewrite.path(),
                &plan,
                password,
            )?;
            archive::test_archive(data.engine_path(), rewrite.path(), password)?;
            let entries = archive::list_archive(data.engine_path(), rewrite.path(), password)?;
            safe_paths::validate_archive_entries(&entries)?;
            Ok(entries)
        })()
        .map_err(|error| rewrite.recovery_error(error))?;
        rewrite.commit()?;
        install_modified_document(&archives, &data, path, candidate)
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) async fn rename_archive_entry(
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    entry: String,
    new_name: String,
    password: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = jobs.start("modify", 0)?;
        let password = password.map(Zeroizing::new);
        let password = password.as_ref().map(|value| value.as_str());
        let archive_path = PathBuf::from(&path);
        require_writable_format(&archive_path)?;
        let original = archive::list_archive(data.engine_path(), &archive_path, password)?;
        safe_paths::validate_archive_entries(&original)?;
        let (plan, expected) = rename_plan(&original, &entry, &new_name)?;
        control.set_total_bytes(selected_total(std::slice::from_ref(&entry), &original));
        let rewrite = safe_paths::ArchiveRewrite::create(&archive_path)?;
        let candidate = (|| {
            jobs::run_rename(
                &control,
                data.engine_path(),
                rewrite.path(),
                &plan,
                password,
            )?;
            archive::test_archive(data.engine_path(), rewrite.path(), password)?;
            let entries = archive::list_archive(data.engine_path(), rewrite.path(), password)?;
            safe_paths::validate_archive_entries(&entries)?;
            if entry_paths(&entries) != entry_paths(&expected) {
                return Err(ArchiveError::new(
                    "rename_failed",
                    "The archive engine did not produce the expected renamed entries",
                ));
            }
            Ok(entries)
        })()
        .map_err(|error| rewrite.recovery_error(error))?;
        rewrite.commit()?;
        install_modified_document(&archives, &data, path, candidate)
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) async fn set_archive_comment(
    jobs: tauri::State<'_, JobManager>,
    archives: tauri::State<'_, ArchiveStore>,
    data: tauri::State<'_, LocalData>,
    path: String,
    comment: String,
    password: Option<String>,
) -> Result<ArchiveDocument, ArchiveError> {
    let jobs = jobs.inner().clone();
    let archives = archives.inner().clone();
    let data = data.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _control = jobs.start("modify", 0)?;
        let password = password.map(Zeroizing::new);
        let password = password.as_ref().map(|value| value.as_str());
        let archive_path = PathBuf::from(&path);
        if require_writable_format(&archive_path)? != ArchiveFormat::Zip {
            return Err(ArchiveError::new(
                "comment_unsupported",
                "Comments can only be edited in single-volume ZIP archives",
            ));
        }
        let rewrite = safe_paths::ArchiveRewrite::create(&archive_path)?;
        let candidate = (|| {
            archive::set_zip_comment(rewrite.path(), &comment)?;
            archive::test_archive(data.engine_path(), rewrite.path(), password)?;
            let entries = archive::list_archive(data.engine_path(), rewrite.path(), password)?;
            safe_paths::validate_archive_entries(&entries)?;
            Ok(entries)
        })()
        .map_err(|error| rewrite.recovery_error(error))?;
        rewrite.commit()?;
        install_modified_document(&archives, &data, path, candidate)
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
            let entries = jobs::run_inspect(&control, engine, &archive_path, password)?.entries;
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
    file_type: String,
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
            &file_type,
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
pub(crate) async fn view_entry_paths(
    archives: tauri::State<'_, ArchiveStore>,
    path: String,
    folder: String,
    query: String,
    file_type: String,
    show_hidden: bool,
) -> Result<Vec<String>, ArchiveError> {
    let archives = archives.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        archives.paths(&path, &folder, &query, &file_type, show_hidden)
    })
    .await
    .map_err(task_error)?
}

#[tauri::command]
pub(crate) async fn archive_folders(
    archives: tauri::State<'_, ArchiveStore>,
    path: String,
    folder: String,
    show_hidden: bool,
) -> Result<Vec<ArchiveFolder>, ArchiveError> {
    let archives = archives.inner().clone();
    tauri::async_runtime::spawn_blocking(move || archives.folders(&path, &folder, show_hidden))
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
pub(crate) fn recent_archives(
    data: tauri::State<'_, LocalData>,
) -> Result<Vec<String>, ArchiveError> {
    data.recent_archives()
}

#[tauri::command]
pub(crate) fn clear_recent_archives(data: tauri::State<'_, LocalData>) -> Result<(), ArchiveError> {
    data.clear_recent_archives()
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
pub(crate) fn set_menu_enabled(
    menu: tauri::State<'_, crate::app_menu::AppMenu>,
    items: HashMap<String, bool>,
) -> Result<(), ArchiveError> {
    menu.set_enabled(&items)
        .map_err(|error| ArchiveError::new("menu_unavailable", error.to_string()))
}

#[tauri::command]
pub(crate) fn set_recent_menu(
    app: tauri::AppHandle,
    menu: tauri::State<'_, crate::app_menu::AppMenu>,
    paths: Vec<String>,
) -> Result<(), ArchiveError> {
    menu.show_recents(&app, paths.into_iter().take(10).collect())
        .map_err(|error| ArchiveError::new("menu_unavailable", error.to_string()))
}

#[tauri::command]
pub(crate) fn take_shell_requests(
    requests: tauri::State<'_, ShellRequestStore>,
) -> Result<Vec<ShellRequest>, ArchiveError> {
    requests.take()
}

#[tauri::command]
pub(crate) fn take_shell_rejections(requests: tauri::State<'_, ShellRequestStore>) -> Vec<String> {
    requests.take_rejections()
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

/// Where a one-step extraction (Finder's Extract Here, or a dropped batch) should land.
/// A single top-level item goes straight into `destination`; several items get a new
/// folder named after the archive, so they never spill among the folder's other files.
fn enclosing_destination(
    staging: &Path,
    destination: &Path,
    archive: &Path,
) -> Result<PathBuf, ArchiveError> {
    let staged = fs::read_dir(staging)
        .map_err(|error| ArchiveError::new("staging_failed", error.to_string()))?
        .count();
    if staged <= 1 {
        return Ok(destination.to_path_buf());
    }
    let name = archive
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_lowercase();
    let stem = [
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz2", ".tbz", ".txz", ".tzst",
        ".zip.001", ".7z.001",
    ]
    .iter()
    .find(|suffix| lower.ends_with(*suffix))
    .map(|suffix| name[..name.len() - suffix.len()].to_string())
    .or_else(|| {
        archive
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
    })
    .filter(|stem| !stem.is_empty())
    .unwrap_or_else(|| "Extracted".to_string());
    let mut folder = destination.join(&stem);
    let mut number = 2;
    while fs::symlink_metadata(&folder).is_ok() {
        folder = destination.join(format!("{stem} {number}"));
        number += 1;
    }
    Ok(folder)
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

fn require_writable_format(path: &Path) -> Result<ArchiveFormat, ArchiveError> {
    archive::writable_format(path).ok_or_else(|| {
        ArchiveError::new(
            "archive_not_writable",
            "Only single-volume ZIP and 7z archives can be modified",
        )
    })
}

fn install_modified_document(
    archives: &ArchiveStore,
    data: &LocalData,
    path: String,
    entries: Vec<ArchiveEntry>,
) -> Result<ArchiveDocument, ArchiveError> {
    let mut document = archives.install(path, data.engine_version().to_string(), entries)?;
    document.comment = archive::read_zip_comment(Path::new(&document.path))
        .ok()
        .flatten();
    let _ = data.record_recent(Path::new(&document.path));
    Ok(document)
}

fn modification_selection(
    requested: &[String],
    entries: &[ArchiveEntry],
) -> Result<Vec<String>, ArchiveError> {
    if requested.is_empty() {
        return Err(ArchiveError::new(
            "no_entries",
            "Select at least one archive entry",
        ));
    }
    let index = EntryIndex::new(entries);
    let mut selected = HashSet::new();
    for path in requested {
        let before = selected.len();
        let mut found = index.by_path.contains_key(path.as_str());
        if found {
            selected.insert(path.clone());
        }
        for descendant in index.descendants(path.trim_end_matches('/')) {
            found = true;
            selected.insert(descendant.to_string());
        }
        if !found && selected.len() == before {
            return Err(ArchiveError::new(
                "entry_not_found",
                format!("The archive entry is no longer available: {path}"),
            ));
        }
    }
    let mut selected = selected.into_iter().collect::<Vec<_>>();
    selected.sort();
    Ok(selected)
}

/// Entry paths indexed once, so a large selection is not resolved by rescanning every entry.
struct EntryIndex<'a> {
    by_path: HashMap<&'a str, &'a ArchiveEntry>,
    sorted: Vec<&'a str>,
}

impl<'a> EntryIndex<'a> {
    fn new(entries: &'a [ArchiveEntry]) -> Self {
        let by_path = entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect::<HashMap<_, _>>();
        let mut sorted = by_path.keys().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        Self { by_path, sorted }
    }

    /// Everything stored beneath `folder`; paths sharing the `folder/` prefix sort together.
    fn descendants(&self, folder: &str) -> impl Iterator<Item = &'a str> + '_ {
        let prefix = format!("{folder}/");
        let start = self.sorted.partition_point(|path| *path < prefix.as_str());
        self.sorted[start..]
            .iter()
            .copied()
            .take_while(move |path| path.starts_with(&prefix))
    }
}

/// The paths chosen for an operation; an empty selection means every entry.
struct Selection<'a>(HashSet<&'a str>);

impl<'a> Selection<'a> {
    fn new(selected: &'a [String]) -> Self {
        Self(selected.iter().map(String::as_str).collect())
    }

    /// True for a selected entry and for anything inside a selected folder.
    fn contains(&self, entry: &ArchiveEntry) -> bool {
        if self.0.is_empty() {
            return true;
        }
        let mut path = entry.path.as_str();
        loop {
            if self.0.contains(path) {
                return true;
            }
            match path.rfind(['/', '\\']) {
                Some(separator) => path = &path[..separator],
                None => return false,
            }
        }
    }
}

/// Refuses additions that would turn a file into a folder or the reverse, and requires
/// `replace_existing` before an existing file is overwritten. Adding into a folder that
/// already exists merges with it.
fn check_addition(
    plan: &archive::CreationPlan,
    existing: &[ArchiveEntry],
    replace_existing: bool,
) -> Result<(), ArchiveError> {
    let by_path = existing
        .iter()
        .map(|entry| (normalized(&entry.path), entry))
        .collect::<HashMap<_, _>>();
    let mut folders = HashSet::new();
    for path in by_path.keys() {
        let mut path = path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if !folders.insert(parent) {
                break;
            }
            path = parent;
        }
    }
    let type_conflict = |name: &str, kind: &str| {
        ArchiveError::new(
            "entry_type_conflict",
            format!("{name} cannot be added because the archive already has a {kind} with that name. Rename or delete it first."),
        )
    };
    let mut replaced = Vec::new();
    for original in &plan.names {
        let name = &normalized(original);
        let mut ancestor = name.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if by_path.get(parent).is_some_and(|entry| !entry.is_directory) {
                return Err(type_conflict(parent, "file"));
            }
            ancestor = parent;
        }
        let existing_file = by_path
            .get(name.as_str())
            .is_some_and(|entry| !entry.is_directory);
        let existing_folder = folders.contains(name.as_str())
            || by_path
                .get(name.as_str())
                .is_some_and(|entry| entry.is_directory);
        if plan.root.join(original).is_dir() {
            if existing_file {
                return Err(type_conflict(name, "file"));
            }
        } else if existing_folder {
            return Err(type_conflict(name, "folder"));
        } else if existing_file {
            replaced.push(original.as_str());
        }
    }
    if replace_existing || replaced.is_empty() {
        return Ok(());
    }
    Err(ArchiveError::new(
        "entry_exists",
        if replaced.len() == 1 {
            format!("{} already exists in this archive.", replaced[0])
        } else {
            format!(
                "{} items already exist in this archive, including {}.",
                replaced.len(),
                replaced[0]
            )
        },
    ))
}

fn rename_plan(
    entries: &[ArchiveEntry],
    from: &str,
    new_name: &str,
) -> Result<(archive::RenamePlan, Vec<ArchiveEntry>), ArchiveError> {
    if new_name.is_empty()
        || new_name.contains(['/', '\\', '\n', '\r'])
        || new_name != new_name.trim()
    {
        return Err(ArchiveError::new(
            "invalid_name",
            "Enter one non-empty file or folder name without slashes or surrounding spaces",
        ));
    }
    let prefix = format!("{}/", from.trim_end_matches('/'));
    let matches = entries
        .iter()
        .filter(|entry| entry.path == from || entry.path.starts_with(&prefix))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(ArchiveError::new(
            "entry_not_found",
            "The archive entry is no longer available",
        ));
    }
    let parent = from.rsplit_once('/').map(|(parent, _)| parent);
    let to = parent
        .map(|parent| format!("{parent}/{new_name}"))
        .unwrap_or_else(|| new_name.to_string());
    let mut expected = entries.to_vec();
    for entry in &mut expected {
        if entry.path == from {
            entry.path.clone_from(&to);
        } else if let Some(suffix) = entry.path.strip_prefix(&prefix) {
            entry.path = format!("{to}/{suffix}");
        }
    }
    safe_paths::validate_archive_entries(&expected)?;

    let pairs = if matches.iter().any(|entry| entry.path == from) {
        vec![(from.to_string(), to)]
    } else {
        let mut roots = Vec::<&ArchiveEntry>::new();
        for entry in matches {
            if roots
                .iter()
                .any(|root| root.is_directory && entry.path.starts_with(&format!("{}/", root.path)))
            {
                continue;
            }
            roots.push(entry);
        }
        roots
            .into_iter()
            .map(|entry| {
                let suffix = entry.path.strip_prefix(&prefix).unwrap();
                (entry.path.clone(), format!("{to}/{suffix}"))
            })
            .collect()
    };
    Ok((archive::prepare_rename(pairs)?, expected))
}

fn entry_paths(entries: &[ArchiveEntry]) -> Vec<String> {
    let mut paths = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths
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
    let index = EntryIndex::new(entries);
    let mut unique = HashSet::new();
    for path in selected {
        let exact = index.by_path.get(path.as_str());
        let mut descendants = index.descendants(&path).peekable();
        if exact.is_none() && descendants.peek().is_none() {
            return Err(ArchiveError::new(
                "entry_not_found",
                format!("Archive entry was not found: {path}"),
            ));
        }
        if exact.is_some_and(|entry| !entry.is_directory) || descendants.peek().is_none() {
            drop(descendants);
            unique.insert(path);
        } else {
            unique.extend(descendants.map(str::to_string));
        }
    }
    let mut selected = unique.into_iter().collect::<Vec<_>>();
    selected.sort();
    Ok(selected)
}

/// What to ask the engine for once links are taken out of a selection.
#[derive(Debug)]
struct LinkPlan {
    /// Paths to extract; empty still means "everything".
    paths: Vec<String>,
    /// Links left out because they cannot be recreated safely.
    links_skipped: usize,
    /// Hard links to write as copies afterwards: (link path, path of the file it names).
    hard_links: Vec<(String, String)>,
    /// Files extracted only so a hard link can be copied from them.
    extras: Vec<String>,
}

/// Follows a hard link to the regular file it names, which must be in the same archive.
fn hard_link_target<'a>(
    entry: &ArchiveEntry,
    by_path: &HashMap<&str, &'a ArchiveEntry>,
) -> Option<&'a ArchiveEntry> {
    let mut current = entry;
    for _ in 0..16 {
        let target = by_path.get(current.link_target.as_deref()?)?;
        if target.hard_link {
            current = target;
        } else if target.is_directory || safe_paths::is_archive_link(target) {
            return None;
        } else {
            return Some(target);
        }
    }
    None
}

/// Takes links out of an extraction. Symbolic links are skipped; a hard link is written
/// afterwards as its own copy of the file it names, so the engine never creates a link.
fn without_links(
    selected: Vec<String>,
    entries: &[ArchiveEntry],
) -> Result<LinkPlan, ArchiveError> {
    let selection = Selection::new(&selected);
    let links = entries
        .iter()
        .filter(|entry| safe_paths::is_archive_link(entry) && selection.contains(entry))
        .collect::<Vec<_>>();
    if links.is_empty() {
        drop(selection);
        return Ok(LinkPlan {
            paths: selected,
            links_skipped: 0,
            hard_links: Vec::new(),
            extras: Vec::new(),
        });
    }
    let by_path = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut links_skipped = 0;
    let mut hard_links = Vec::new();
    let mut extras = Vec::new();
    for link in links {
        match hard_link_target(link, &by_path).filter(|_| link.hard_link) {
            Some(target) => {
                if !selection.contains(target) && !extras.contains(&target.path) {
                    extras.push(target.path.clone());
                }
                hard_links.push((link.path.clone(), target.path.clone()));
            }
            None => links_skipped += 1,
        }
    }
    // Naming a folder would bring its links back, so name every remaining item itself.
    // Folders are created by their contents; only empty ones need their own line.
    let mut parents = HashSet::new();
    for entry in entries {
        let mut path = entry.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if !parents.insert(parent) {
                break;
            }
            path = parent;
        }
    }
    let mut paths = entries
        .iter()
        .filter(|entry| !safe_paths::is_archive_link(entry) && selection.contains(entry))
        .filter(|entry| !entry.is_directory || !parents.contains(entry.path.as_str()))
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    paths.extend(extras.iter().cloned());
    if paths.is_empty() {
        return Err(ArchiveError::new(
            "only_links",
            "The selection contains only links, which Archi does not extract",
        ));
    }
    Ok(LinkPlan {
        paths,
        links_skipped,
        hard_links,
        extras,
    })
}

fn selected_total(selected: &[String], entries: &[ArchiveEntry]) -> u64 {
    let selection = Selection::new(selected);
    entries
        .iter()
        .filter(|entry| selection.contains(entry))
        .filter_map(|entry| entry.size)
        .fold(0, u64::saturating_add)
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
    let selection = Selection::new(selected);
    let unknown = entries
        .iter()
        .filter(|entry| !entry.is_directory && !safe_paths::is_archive_link(entry))
        .filter(|entry| entry.size.is_none() && selection.contains(entry))
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

fn preview_root(app: &tauri::AppHandle) -> Result<PathBuf, ArchiveError> {
    Ok(app
        .path()
        .app_cache_dir()
        .map_err(|error| {
            ArchiveError::new(
                "preview_unavailable",
                format!("Could not locate the preview cache: {error}"),
            )
        })?
        .join("Previews"))
}

fn prepare_preview_entry(
    jobs: JobManager,
    data: LocalData,
    path: String,
    entry: String,
    password: Option<String>,
    preview_root: PathBuf,
    purpose: EntryCopy,
) -> Result<PathBuf, ArchiveError> {
    let control = jobs.start("extract", 0)?;
    let password = password.map(Zeroizing::new);
    let password = password.as_ref().map(|value| value.as_str());
    let archive_path = PathBuf::from(path);
    let listing = jobs::run_inspect(&control, data.engine_path(), &archive_path, password)?;
    let archive_entries = listing.entries;
    safe_paths::validate_archive_entries(&archive_entries)?;
    let selected_entry = archive_entries
        .iter()
        .find(|candidate| candidate.path == entry)
        .ok_or_else(|| {
            ArchiveError::new(
                "entry_not_found",
                "The archive entry is no longer available",
            )
        })?;
    let settings = data.load()?;
    let limit = match purpose {
        EntryCopy::Preview => settings.preview_limit(),
        EntryCopy::DragOut => settings.extraction_limit(),
    };
    // A hard link is opened as the file it names, under the link's own name.
    let named = if selected_entry.hard_link {
        let by_path = archive_entries
            .iter()
            .map(|candidate| (candidate.path.as_str(), candidate))
            .collect::<HashMap<_, _>>();
        Some(hard_link_target(selected_entry, &by_path).ok_or_else(|| {
            ArchiveError::new(
                "unsafe_link",
                "This link does not name a regular file in the archive",
            )
        })?)
    } else {
        None
    };
    let source_path = named.map_or(&entry, |target| &target.path).clone();
    match named {
        Some(target) => safe_paths::validate_preview_entry(
            &ArchiveEntry {
                path: entry.clone(),
                ..target.clone()
            },
            limit,
            purpose,
        )?,
        None => safe_paths::validate_preview_entry(selected_entry, limit, purpose)?,
    }
    let selected = validate_selection(vec![source_path.clone()], &archive_entries)?;
    // A dragged entry's declared size was checked above; its real size is capped below.
    let total = validate_expansion(
        &selected,
        &archive_entries,
        limit,
        purpose == EntryCopy::DragOut,
    )?;
    control.set_total_bytes(total);
    let plan = archive::prepare_extraction(&selected, listing.layout)?;
    let staging = StagingDirectory::create()?;
    jobs::run_extract(
        &control,
        data.engine_path(),
        &archive_path,
        staging.path(),
        &plan,
        password,
        Some(limit),
    )?;
    if named.is_some() {
        safe_paths::materialize_hard_links(
            staging.path(),
            &[(entry.clone(), source_path.clone())],
            std::slice::from_ref(&source_path),
        )?;
    }
    safe_paths::persist_preview_file(staging.path(), &entry, &preview_root, limit, purpose)
}

pub(crate) fn platform_open(path: &Path) -> Result<(), ArchiveError> {
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
                "open_failed",
                format!("Could not open the requested item: {error}"),
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
                hard_link: false,
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
                hard_link: false,
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

        let implicit_folder = vec![
            ArchiveEntry {
                path: "folder/one.txt".to_string(),
                ..entries[1].clone()
            },
            ArchiveEntry {
                path: "folder/nested/two.txt".to_string(),
                ..entries[1].clone()
            },
        ];
        let (_, renamed) = rename_plan(&implicit_folder, "folder", "renamed").unwrap();
        assert_eq!(
            entry_paths(&renamed),
            ["renamed/nested/two.txt", "renamed/one.txt"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn additions_land_in_the_chosen_folder_and_replace_only_when_confirmed() {
        let engine = archive::bundled_engine().unwrap();
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("base");
        fs::create_dir_all(base.join("Documents")).unwrap();
        fs::write(base.join("Documents/a.txt"), "old").unwrap();
        fs::write(base.join("Documents/keep.txt"), "keep").unwrap();
        fs::write(base.join("a.txt"), "root").unwrap();
        let zip = root.path().join("target.zip");
        let plan =
            archive::prepare_creation(&[base.join("Documents"), base.join("a.txt")], &zip).unwrap();
        archive::create_archive(
            &engine,
            &zip,
            &plan,
            ArchiveFormat::Zip,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();

        let incoming = root.path().join("incoming");
        fs::create_dir_all(incoming.join("photos/deep")).unwrap();
        fs::create_dir_all(incoming.join("r\u{e9}sum\u{e9}s")).unwrap();
        fs::write(incoming.join("a.txt"), "new").unwrap();
        fs::write(incoming.join("photos/deep/y.jpg"), "y").unwrap();
        fs::write(incoming.join("r\u{e9}sum\u{e9}s/cv.txt"), "cv").unwrap();
        let inputs = [
            incoming.join("a.txt"),
            incoming.join("photos"),
            incoming.join("r\u{e9}sum\u{e9}s"),
        ];

        let existing = archive::list_archive(&engine, &zip, None).unwrap();
        let plan = archive::prepare_addition(&inputs, &zip, "Documents").unwrap();
        assert!(plan
            .names
            .contains(&"Documents/photos/deep/y.jpg".to_string()));
        let refused = check_addition(&plan, &existing, false).unwrap_err();
        assert_eq!(refused.code, "entry_exists");
        assert!(refused.message.contains("Documents/a.txt"));
        check_addition(&plan, &existing, true).unwrap();
        archive::create_archive(
            &engine,
            &zip,
            &plan,
            ArchiveFormat::Zip,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();

        let entries = archive::list_archive(&engine, &zip, None).unwrap();
        assert!(entries.iter().all(|entry| !entry.is_link));
        let stored = entries
            .iter()
            .map(|entry| normalized(&entry.path))
            .collect::<HashSet<_>>();
        assert!(plan
            .names
            .iter()
            .all(|name| stored.contains(&normalized(name))));
        let out = root.path().join("out");
        archive::extract_archive(&engine, &zip, &out, None).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("Documents/a.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(out.join("Documents/keep.txt")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "root");
        assert_eq!(
            fs::read_to_string(out.join("Documents/photos/deep/y.jpg")).unwrap(),
            "y"
        );

        // A file cannot take a folder's name, nor be stored beneath a file.
        let clash = root.path().join("clash");
        fs::create_dir_all(&clash).unwrap();
        fs::write(clash.join("Documents"), "file").unwrap();
        let plan = archive::prepare_addition(&[clash.join("Documents")], &zip, "").unwrap();
        assert_eq!(
            check_addition(&plan, &entries, true).unwrap_err().code,
            "entry_type_conflict"
        );
        let plan = archive::prepare_addition(&[incoming.join("a.txt")], &zip, "a.txt").unwrap();
        assert_eq!(
            check_addition(&plan, &entries, true).unwrap_err().code,
            "entry_type_conflict"
        );
        assert_eq!(
            archive::prepare_addition(&[incoming.join("a.txt")], &zip, "../up")
                .unwrap_err()
                .code,
            "invalid_folder"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tarballs_are_browsed_and_extracted_without_their_links() {
        use crate::archive::ArchiveLayout;
        use std::os::unix::fs::symlink;

        let engine = archive::bundled_engine().unwrap();
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("proj");
        fs::create_dir_all(project.join("sub")).unwrap();
        fs::create_dir_all(project.join("empty")).unwrap();
        fs::create_dir_all(project.join("only-links")).unwrap();
        fs::write(project.join("a.txt"), "hello").unwrap();
        fs::write(project.join("sub/b.txt"), "world").unwrap();
        fs::write(project.join("st*r.txt"), "star").unwrap();
        fs::write(project.join("sub/trailing "), "space").unwrap();
        symlink("a.txt", project.join("link.txt")).unwrap();
        symlink("/etc", project.join("only-links/escape")).unwrap();

        for (name, flag, layout) in [
            ("p.tar", "-cf", ArchiveLayout::Plain),
            ("p.tar.gz", "-czf", ArchiveLayout::CompressedTar),
            ("p.txz", "-cJf", ArchiveLayout::CompressedTar),
        ] {
            let tarball = root.path().join(name);
            assert!(Command::new("/usr/bin/bsdtar")
                .env("COPYFILE_DISABLE", "1")
                .current_dir(root.path())
                .arg(flag)
                .arg(&tarball)
                .arg("proj")
                .status()
                .unwrap()
                .success());

            let listing = archive::inspect_archive(&engine, &tarball, None).unwrap();
            assert_eq!(listing.layout, layout, "{name}");
            // Opening through a cancellable job sees exactly the same archive.
            let manager = JobManager::default();
            let job = manager.start("open", 0).unwrap();
            let through_job = jobs::run_inspect(&job, &engine, &tarball, None).unwrap();
            assert_eq!(through_job.layout, layout, "{name}");
            assert_eq!(through_job.entries, listing.entries, "{name}");
            assert!(manager.cancel().unwrap() || !job.cancelled());
            drop(job);
            let job = manager.start("open", 0).unwrap();
            assert!(manager.cancel().unwrap());
            assert_eq!(
                jobs::run_inspect(&job, &engine, &tarball, None)
                    .unwrap_err()
                    .code,
                "cancelled"
            );
            drop(job);
            assert!(listing
                .entries
                .iter()
                .any(|entry| entry.path == "proj/sub/b.txt"));
            safe_paths::validate_archive_entries(&listing.entries).unwrap();

            for (requested, expected_links) in [(vec![], 2), (vec!["proj/sub".to_string()], 0)] {
                let everything = requested.is_empty();
                let selected = validate_selection(requested, &listing.entries).unwrap();
                let LinkPlan {
                    paths: selected,
                    links_skipped: links,
                    ..
                } = without_links(selected, &listing.entries).unwrap();
                assert_eq!(links, expected_links, "{name}");
                let plan = archive::prepare_extraction(&selected, listing.layout).unwrap();
                let staging = StagingDirectory::create().unwrap();
                let status = archive::spawn_extract_entries(
                    &engine,
                    &tarball,
                    staging.path(),
                    &plan,
                    None,
                    Stdio::null(),
                    Stdio::null(),
                )
                .unwrap()
                .wait()
                .unwrap();
                assert!(status.success(), "{name}");
                safe_paths::validate_staging(staging.path()).unwrap();
                let extracted = staging.path().join("proj");
                assert_eq!(
                    fs::read_to_string(extracted.join("sub/b.txt")).unwrap(),
                    "world"
                );
                assert_eq!(
                    fs::read_to_string(extracted.join("sub/trailing ")).unwrap(),
                    "space"
                );
                assert_eq!(extracted.join("a.txt").exists(), everything, "{name}");
                assert_eq!(extracted.join("st*r.txt").exists(), everything, "{name}");
                assert_eq!(extracted.join("empty").is_dir(), everything, "{name}");
                assert!(fs::symlink_metadata(extracted.join("link.txt")).is_err());
                assert!(fs::symlink_metadata(extracted.join("only-links/escape")).is_err());
            }
            assert_eq!(
                without_links(vec!["proj/link.txt".to_string()], &listing.entries)
                    .unwrap_err()
                    .code,
                "only_links"
            );
        }

        // A compressed stream that is not a TAR keeps its single-file listing.
        let fake = root.path().join("fake.tar.gz");
        let plan = archive::prepare_creation(&[project.join("a.txt")], &fake).unwrap();
        archive::create_archive(
            &engine,
            &root.path().join("fake.gz"),
            &plan,
            ArchiveFormat::Gzip,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();
        fs::rename(root.path().join("fake.gz"), &fake).unwrap();
        let listing = archive::inspect_archive(&engine, &fake, None).unwrap();
        assert_eq!(listing.layout, ArchiveLayout::Plain);
        assert_eq!(listing.entries.len(), 1);

        // A truncated stream is reported as damaged rather than shown as an empty tarball.
        let damaged = root.path().join("damaged.tar.gz");
        let bytes = fs::read(root.path().join("p.tar.gz")).unwrap();
        fs::write(&damaged, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(
            archive::inspect_archive(&engine, &damaged, None)
                .unwrap_err()
                .code,
            "damaged_archive"
        );
        let manager = JobManager::default();
        let job = manager.start("open", 0).unwrap();
        assert_eq!(
            jobs::run_inspect(&job, &engine, &damaged, None)
                .unwrap_err()
                .code,
            "damaged_archive"
        );
        assert_eq!(
            jobs::run_inspect(&job, &engine, &fake, None)
                .unwrap()
                .layout,
            ArchiveLayout::Plain
        );
    }

    #[test]
    fn large_selections_are_resolved_without_rescanning_every_entry() {
        let file = |path: String| ArchiveEntry {
            path,
            is_directory: false,
            size: Some(1),
            packed_size: None,
            modified: None,
            encrypted: false,
            method: None,
            is_link: false,
            link_target: None,
            hard_link: false,
        };
        let mut entries = (0..100_000)
            .map(|index| file(format!("bulk/group-{:03}/file-{index:06}.txt", index % 500)))
            .collect::<Vec<_>>();
        entries.push(ArchiveEntry {
            is_link: true,
            ..file("bulk/link".to_string())
        });
        let everything = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();

        let started = std::time::Instant::now();
        let selected = validate_selection(everything.clone(), &entries).unwrap();
        assert_eq!(selected.len(), 100_001);
        assert_eq!(
            validate_expansion(&selected, &entries, MAX_EXPANDED_BYTES, false).unwrap(),
            100_001
        );
        let plan = without_links(selected, &entries).unwrap();
        assert_eq!((plan.paths.len(), plan.links_skipped), (100_000, 1));
        assert_eq!(
            modification_selection(&everything, &entries).unwrap().len(),
            100_001
        );

        let folder = validate_selection(vec!["bulk/group-007".to_string()], &entries).unwrap();
        assert_eq!(folder.len(), 200);
        assert_eq!(selected_total(&["bulk".to_string()], &entries), 100_001);
        assert_eq!(selected_total(&["bulk/group-00".to_string()], &entries), 0);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "selection handling took {:?}",
            started.elapsed()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn hard_links_are_extracted_as_their_own_copies() {
        use std::os::unix::fs::MetadataExt;

        let engine = archive::bundled_engine().unwrap();
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("proj");
        fs::create_dir_all(project.join("bin")).unwrap();
        fs::write(project.join("bin/tool"), "binary").unwrap();
        fs::hard_link(project.join("bin/tool"), project.join("alias")).unwrap();
        fs::hard_link(project.join("bin/tool"), project.join("bin/second")).unwrap();
        fs::write(project.join("other.txt"), "other").unwrap();
        let tarball = root.path().join("links.tar.gz");
        assert!(Command::new("/usr/bin/bsdtar")
            .env("COPYFILE_DISABLE", "1")
            .current_dir(root.path())
            .arg("-czf")
            .arg(&tarball)
            .arg("proj")
            .status()
            .unwrap()
            .success());

        let listing = archive::inspect_archive(&engine, &tarball, None).unwrap();
        let links = listing
            .entries
            .iter()
            .filter(|entry| entry.hard_link)
            .collect::<Vec<_>>();
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|entry| entry.size == Some(6)));
        let real = listing
            .entries
            .iter()
            .find(|entry| !entry.is_link && entry.size == Some(6))
            .unwrap()
            .path
            .clone();
        let a_link = links[0].path.clone();

        let extract = |requested: Vec<String>| {
            let selected = validate_selection(requested, &listing.entries).unwrap();
            let plan = without_links(selected, &listing.entries).unwrap();
            assert_eq!(plan.links_skipped, 0);
            let staging = StagingDirectory::create().unwrap();
            let extraction = archive::prepare_extraction(&plan.paths, listing.layout).unwrap();
            assert!(archive::spawn_extract_entries(
                &engine,
                &tarball,
                staging.path(),
                &extraction,
                None,
                Stdio::null(),
                Stdio::null(),
            )
            .unwrap()
            .wait()
            .unwrap()
            .success());
            safe_paths::materialize_hard_links(staging.path(), &plan.hard_links, &plan.extras)
                .unwrap();
            safe_paths::validate_staging(staging.path()).unwrap();
            (staging, plan)
        };

        // Everything: all three names exist as separate, identical files.
        let (staging, plan) = extract(Vec::new());
        assert_eq!(plan.hard_links.len(), 2);
        assert!(plan.extras.is_empty());
        for name in ["proj/bin/tool", "proj/alias", "proj/bin/second"] {
            let path = staging.path().join(name);
            assert_eq!(fs::read_to_string(&path).unwrap(), "binary", "{name}");
            assert_eq!(fs::metadata(&path).unwrap().nlink(), 1, "{name}");
        }
        assert!(staging.path().join("proj/other.txt").exists());

        // Only a link: the file it names is fetched to copy from, then removed again.
        let (staging, plan) = extract(vec![a_link.clone()]);
        assert_eq!(plan.extras, std::slice::from_ref(&real));
        assert_eq!(
            fs::read_to_string(staging.path().join(&a_link)).unwrap(),
            "binary"
        );
        let extracted = walk_files(staging.path());
        assert_eq!(extracted, [staging.path().join(&a_link)]);

        // A hard link naming something outside the archive is skipped, not followed.
        let mut forged = listing.entries.clone();
        for entry in forged.iter_mut().filter(|entry| entry.hard_link) {
            entry.link_target = Some("/etc/hosts".to_string());
        }
        let plan = without_links(Vec::new(), &forged).unwrap();
        assert_eq!((plan.links_skipped, plan.hard_links.len()), (2, 0));
        assert_eq!(
            safe_paths::materialize_hard_links(
                staging.path(),
                &[("../escape".to_string(), a_link.clone())],
                &[]
            )
            .unwrap_err()
            .code,
            "unsafe_path"
        );
    }

    #[cfg(test)]
    fn walk_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn one_step_extraction_encloses_loose_items_in_a_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let destination = root.path().join("Downloads");
        fs::create_dir_all(staging.join("only")).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let archive = destination.join("Photos.tar.gz");
        fs::write(&archive, "archive").unwrap();

        assert_eq!(
            enclosing_destination(&staging, &destination, &archive).unwrap(),
            destination
        );
        fs::write(staging.join("loose.txt"), "loose").unwrap();
        assert_eq!(
            enclosing_destination(&staging, &destination, &archive).unwrap(),
            destination.join("Photos")
        );
        fs::create_dir(destination.join("Photos")).unwrap();
        assert_eq!(
            enclosing_destination(&staging, &destination, &archive).unwrap(),
            destination.join("Photos 2")
        );
        // An archive without an extension would otherwise collide with its own folder.
        let bare = destination.join("backup");
        fs::write(&bare, "archive").unwrap();
        assert_eq!(
            enclosing_destination(&staging, &destination, &bare).unwrap(),
            destination.join("backup 2")
        );
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
