use crate::archive::{self, ArchiveError, ArchiveFormat, CompressionLevel};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const SETTINGS_VERSION: u32 = 1;
const MAX_LOG_BYTES: u64 = 256 * 1024;
pub(crate) const MIN_EXPANDED_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const DEFAULT_EXPANDED_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const MIN_PREVIEW_BYTES: u64 = 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_PREVIEW_BYTES: u64 = 100 * 1024 * 1024;
const MAX_RECENT_ARCHIVES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExtractionDestination {
    Ask,
    Sibling,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    version: u32,
    default_format: ArchiveFormat,
    default_compression: CompressionLevel,
    #[serde(default = "default_compression")]
    zip_compression: CompressionLevel,
    #[serde(default = "default_compression")]
    seven_zip_compression: CompressionLevel,
    extraction_destination: ExtractionDestination,
    custom_destination: Option<String>,
    reveal_on_completion: bool,
    notifications: bool,
    show_hidden_entries: bool,
    #[serde(default = "default_history_enabled")]
    history_enabled: bool,
    #[serde(default = "default_expanded_bytes")]
    max_expanded_bytes: u64,
    #[serde(default = "default_preview_bytes")]
    max_preview_bytes: u64,
    max_concurrent_jobs: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            default_format: ArchiveFormat::Zip,
            default_compression: CompressionLevel::Normal,
            zip_compression: CompressionLevel::Normal,
            seven_zip_compression: CompressionLevel::Normal,
            extraction_destination: ExtractionDestination::Ask,
            custom_destination: None,
            reveal_on_completion: true,
            notifications: false,
            show_hidden_entries: false,
            history_enabled: true,
            max_expanded_bytes: default_expanded_bytes(),
            max_preview_bytes: default_preview_bytes(),
            max_concurrent_jobs: 1,
        }
    }
}

impl Settings {
    pub(crate) fn preview_limit(&self) -> u64 {
        self.max_preview_bytes.min(self.max_expanded_bytes)
    }
}

const fn default_expanded_bytes() -> u64 {
    DEFAULT_EXPANDED_BYTES
}

const fn default_preview_bytes() -> u64 {
    DEFAULT_PREVIEW_BYTES
}

const fn default_compression() -> CompressionLevel {
    CompressionLevel::Normal
}

const fn default_history_enabled() -> bool {
    true
}

#[derive(Clone)]
pub(crate) struct LocalData(Arc<LocalDataInner>);

struct LocalDataInner {
    directory: PathBuf,
    engine_path: PathBuf,
    engine_version: String,
    lock: Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRecord<'a> {
    timestamp: u64,
    app_version: &'static str,
    os: &'static str,
    architecture: &'static str,
    engine_version: &'a str,
    event: &'a str,
    code: Option<&'a str>,
}

impl LocalData {
    pub(crate) fn initialize(app: &AppHandle) -> Result<Self, ArchiveError> {
        let directory = app.path().app_config_dir().map_err(|error| {
            ArchiveError::new(
                "settings_unavailable",
                format!("Could not locate the settings folder: {error}"),
            )
        })?;
        fs::create_dir_all(&directory).map_err(|error| {
            ArchiveError::new(
                "settings_unavailable",
                format!("Could not create the settings folder: {error}"),
            )
        })?;
        restrict_directory(&directory)?;
        let engine_path = archive::bundled_engine()?;
        let engine_version = archive::engine_version(&engine_path)?;
        let data = Self(Arc::new(LocalDataInner {
            directory,
            engine_path,
            engine_version,
            lock: Mutex::new(()),
        }));
        data.record("startup", None)?;
        Ok(data)
    }

    pub(crate) fn load(&self) -> Result<Settings, ArchiveError> {
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        let path = self.0.directory.join("settings.json");
        if !path.exists() {
            return Ok(Settings::default());
        }
        let settings: Settings =
            serde_json::from_reader(fs::File::open(&path).map_err(|error| {
                ArchiveError::new(
                    "settings_unavailable",
                    format!("Could not open settings: {error}"),
                )
            })?)
            .map_err(|error| {
                ArchiveError::new(
                    "settings_invalid",
                    format!("Settings are not valid JSON: {error}"),
                )
            })?;
        validate(&settings)?;
        Ok(settings)
    }

    pub(crate) fn engine_path(&self) -> &Path {
        &self.0.engine_path
    }

    pub(crate) fn engine_version(&self) -> &str {
        &self.0.engine_version
    }

    pub(crate) fn save(&self, settings: Settings) -> Result<Settings, ArchiveError> {
        validate(&settings)?;
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        write_json(&self.0.directory, "settings.json", &settings)?;
        if !settings.history_enabled {
            remove_if_exists(&self.0.directory.join("recent-archives.json"))?;
        }
        Ok(settings)
    }

    pub(crate) fn reset(&self) -> Result<Settings, ArchiveError> {
        self.save(Settings::default())
    }

    pub(crate) fn recent_archives(&self) -> Result<Vec<String>, ArchiveError> {
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        read_recents(&self.0.directory)
    }

    pub(crate) fn record_recent(&self, path: &Path) -> Result<(), ArchiveError> {
        if !self.load()?.history_enabled {
            return Ok(());
        }
        let path = path.canonicalize().map_err(settings_error)?;
        if !path.is_file() {
            return Err(ArchiveError::new(
                "settings_invalid",
                "Only existing archive files can be added to recent history",
            ));
        }
        let path = path.to_string_lossy().into_owned();
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        let mut recents = read_recents(&self.0.directory)?;
        recents.retain(|value| value != &path);
        recents.insert(0, path);
        recents.truncate(MAX_RECENT_ARCHIVES);
        write_json(&self.0.directory, "recent-archives.json", &recents)
    }

    pub(crate) fn clear_recent_archives(&self) -> Result<(), ArchiveError> {
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        remove_if_exists(&self.0.directory.join("recent-archives.json"))
    }

    pub(crate) fn record(&self, event: &str, code: Option<&str>) -> Result<(), ArchiveError> {
        if !matches!(
            event,
            "startup" | "open" | "create" | "extract" | "test" | "modify" | "error"
        ) || code.is_some_and(|value| {
            value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }) {
            return Err(ArchiveError::new(
                "invalid_diagnostic",
                "Diagnostic event data was rejected",
            ));
        }
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        rotate_log(&self.0.directory)?;
        let record = DiagnosticRecord {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            app_version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            engine_version: &self.0.engine_version,
            event,
            code,
        };
        let path = self.0.directory.join("diagnostics.log");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(log_error)?;
        serde_json::to_writer(&mut file, &record).map_err(log_error)?;
        file.write_all(b"\n").map_err(log_error)?;
        restrict_file(&path)?;
        Ok(())
    }

    pub(crate) fn clear_diagnostics(&self) -> Result<(), ArchiveError> {
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        for name in ["diagnostics.log", "diagnostics.1.log"] {
            let path = self.0.directory.join(name);
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(log_error(error)),
            }
        }
        Ok(())
    }

    pub(crate) fn export_diagnostics(&self, destination: &Path) -> Result<(), ArchiveError> {
        if !destination.is_absolute() || destination.file_name().is_none() {
            return Err(ArchiveError::new(
                "invalid_destination",
                "The diagnostics export path must be an absolute file path",
            ));
        }
        let parent = destination.parent().ok_or_else(|| {
            ArchiveError::new("invalid_destination", "Diagnostics export has no folder")
        })?;
        if !parent.is_dir() {
            return Err(ArchiveError::new(
                "invalid_destination",
                "The diagnostics export folder does not exist",
            ));
        }
        let _guard = self.0.lock.lock().map_err(lock_error)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(log_error)?;
        for name in ["diagnostics.1.log", "diagnostics.log"] {
            if let Ok(mut input) = fs::File::open(self.0.directory.join(name)) {
                let mut buffer = Vec::new();
                input.read_to_end(&mut buffer).map_err(log_error)?;
                output.write_all(&buffer).map_err(log_error)?;
            }
        }
        restrict_file(destination)?;
        Ok(())
    }
}

fn validate(settings: &Settings) -> Result<(), ArchiveError> {
    if settings.version != SETTINGS_VERSION {
        return Err(ArchiveError::new(
            "settings_version",
            "This settings version is not supported",
        ));
    }
    if settings.max_concurrent_jobs != 1 {
        return Err(ArchiveError::new(
            "settings_invalid",
            "This release supports one concurrent archive job",
        ));
    }
    if !(MIN_EXPANDED_BYTES..=MAX_EXPANDED_BYTES).contains(&settings.max_expanded_bytes) {
        return Err(ArchiveError::new(
            "settings_invalid",
            "The extraction size limit must be between 1 MiB and 1 TiB",
        ));
    }
    if !(MIN_PREVIEW_BYTES..=MAX_PREVIEW_BYTES).contains(&settings.max_preview_bytes) {
        return Err(ArchiveError::new(
            "settings_invalid",
            "The preview size limit must be between 1 MiB and 1 GiB",
        ));
    }
    if settings.extraction_destination == ExtractionDestination::Custom {
        let path = settings.custom_destination.as_deref().ok_or_else(|| {
            ArchiveError::new("settings_invalid", "Choose a custom extraction destination")
        })?;
        if !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
            return Err(ArchiveError::new(
                "settings_invalid",
                "The custom extraction destination must be an existing folder",
            ));
        }
    }
    Ok(())
}

fn read_recents(directory: &Path) -> Result<Vec<String>, ArchiveError> {
    let path = directory.join("recent-archives.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    if fs::metadata(&path).map_err(settings_error)?.len() > 64 * 1024 {
        return Err(ArchiveError::new(
            "settings_invalid",
            "Recent archive history is unexpectedly large",
        ));
    }
    let recents: Vec<String> =
        serde_json::from_reader(fs::File::open(&path).map_err(settings_error)?)
            .map_err(settings_error)?;
    if recents.len() > MAX_RECENT_ARCHIVES
        || recents.iter().any(|value| !Path::new(value).is_absolute())
    {
        return Err(ArchiveError::new(
            "settings_invalid",
            "Recent archive history is invalid",
        ));
    }
    Ok(recents
        .into_iter()
        .filter(|value| Path::new(value).is_file())
        .collect())
}

fn remove_if_exists(path: &Path) -> Result<(), ArchiveError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(settings_error(error)),
    }
}

fn write_json<T: Serialize>(directory: &Path, name: &str, value: &T) -> Result<(), ArchiveError> {
    let temporary = directory.join(format!(".{name}.tmp"));
    let target = directory.join(name);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(settings_error)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(settings_error)?;
    file.write_all(b"\n").map_err(settings_error)?;
    file.sync_all().map_err(settings_error)?;
    restrict_file(&temporary)?;
    #[cfg(windows)]
    if target.exists() {
        fs::remove_file(&target).map_err(settings_error)?;
    }
    fs::rename(&temporary, &target).map_err(settings_error)?;
    Ok(())
}

fn rotate_log(directory: &Path) -> Result<(), ArchiveError> {
    let current = directory.join("diagnostics.log");
    if fs::metadata(&current).map(|value| value.len()).unwrap_or(0) < MAX_LOG_BYTES {
        return Ok(());
    }
    let previous = directory.join("diagnostics.1.log");
    if previous.exists() {
        fs::remove_file(&previous).map_err(log_error)?;
    }
    fs::rename(current, previous).map_err(log_error)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(settings_error)
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(settings_error)
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> ArchiveError {
    ArchiveError::new(
        "settings_unavailable",
        "Settings are temporarily unavailable",
    )
}

fn settings_error(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::new(
        "settings_unavailable",
        format!("Could not save settings: {error}"),
    )
}

fn log_error(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::new(
        "diagnostics_unavailable",
        format!("Could not update diagnostics: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(directory: PathBuf) -> LocalData {
        LocalData(Arc::new(LocalDataInner {
            directory,
            engine_path: PathBuf::from("7zz-test"),
            engine_version: "7-Zip test".to_string(),
            lock: Mutex::new(()),
        }))
    }

    #[test]
    fn settings_round_trip_reset_and_sanitized_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let data = data(directory.path().to_path_buf());
        let settings = Settings {
            show_hidden_entries: true,
            ..Settings::default()
        };
        assert!(data.save(settings.clone()).unwrap().show_hidden_entries);
        assert_eq!(data.load().unwrap(), settings);
        assert!(!data.reset().unwrap().show_hidden_entries);
        data.record("open", Some("wrong_password")).unwrap();
        assert!(data.record("open", Some("secret value")).is_err());
        let export = directory.path().join("export.log");
        data.export_diagnostics(&export).unwrap();
        let text = fs::read_to_string(export).unwrap();
        assert!(text.contains("wrong_password"));
        let existing = directory.path().join("existing.log");
        fs::write(&existing, "keep me").unwrap();
        assert!(data.export_diagnostics(&existing).is_err());
        assert_eq!(fs::read_to_string(existing).unwrap(), "keep me");
        data.clear_diagnostics().unwrap();
        assert!(!directory.path().join("diagnostics.log").exists());
    }

    #[test]
    fn legacy_settings_receive_the_default_expansion_limit() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("settings.json"),
            r#"{"version":1,"defaultFormat":"zip","defaultCompression":"normal","extractionDestination":"ask","customDestination":null,"revealOnCompletion":true,"notifications":false,"showHiddenEntries":false,"maxConcurrentJobs":1}"#,
        )
        .unwrap();
        assert_eq!(
            data(directory.path().to_path_buf())
                .load()
                .unwrap()
                .max_expanded_bytes,
            DEFAULT_EXPANDED_BYTES
        );
        assert_eq!(
            data(directory.path().to_path_buf())
                .load()
                .unwrap()
                .max_preview_bytes,
            DEFAULT_PREVIEW_BYTES
        );
    }

    #[test]
    fn recent_archive_history_is_private_bounded_and_clearable() {
        let directory = tempfile::tempdir().unwrap();
        let data = data(directory.path().to_path_buf());
        for index in 0..12 {
            let archive = directory.path().join(format!("{index}.zip"));
            fs::write(&archive, "fixture").unwrap();
            data.record_recent(&archive).unwrap();
        }
        let recents = data.recent_archives().unwrap();
        assert_eq!(recents.len(), MAX_RECENT_ARCHIVES);
        assert!(recents[0].ends_with("11.zip"));
        data.clear_recent_archives().unwrap();
        assert!(data.recent_archives().unwrap().is_empty());

        let disabled = Settings {
            history_enabled: false,
            ..Settings::default()
        };
        data.save(disabled).unwrap();
        data.record_recent(&directory.path().join("11.zip"))
            .unwrap();
        assert!(data.recent_archives().unwrap().is_empty());
    }

    #[test]
    fn frontend_settings_contract_uses_camel_case_fields() {
        assert_eq!(
            serde_json::to_value(Settings::default()).unwrap(),
            serde_json::json!({
                "version": 1,
                "defaultFormat": "zip",
                "defaultCompression": "normal",
                "zipCompression": "normal",
                "sevenZipCompression": "normal",
                "extractionDestination": "ask",
                "customDestination": null,
                "revealOnCompletion": true,
                "notifications": false,
                "showHiddenEntries": false,
                "historyEnabled": true,
                "maxExpandedBytes": 10_737_418_240_u64,
                "maxPreviewBytes": 104_857_600_u64,
                "maxConcurrentJobs": 1
            })
        );
    }
}
