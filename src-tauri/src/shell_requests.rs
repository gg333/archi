use crate::archive::ArchiveError;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const REQUEST_VERSION: u8 = 1;
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_REQUEST_AGE_SECONDS: u64 = 60;
const MAX_PATHS: usize = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShellAction {
    Open,
    ExtractHere,
    ExtractToFolder,
    TestArchive,
    CompressZip,
    CompressOptions,
}

impl ShellAction {
    fn parse(value: &str) -> Result<Self, ArchiveError> {
        serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|_| {
            ArchiveError::new(
                "invalid_shell_request",
                format!("Unknown shell action: {value}"),
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellRequest {
    version: u8,
    pub(crate) action: ShellAction,
    pub(crate) paths: Vec<String>,
    created_at: u64,
    nonce: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellIntegrationStatus {
    available: bool,
    provider_registered: bool,
    document_extensions: usize,
    service_actions: usize,
}

#[derive(Clone)]
pub(crate) struct ShellRequestStore(Arc<ShellRequestStoreInner>);

struct ShellRequestStoreInner {
    directory: PathBuf,
    queue: Mutex<VecDeque<ShellRequest>>,
    io_lock: Mutex<()>,
    provider_registered: AtomicBool,
}

impl ShellRequestStore {
    pub(crate) fn initialize(app: &AppHandle) -> Result<Self, ArchiveError> {
        let directory = app
            .path()
            .app_config_dir()
            .map_err(shell_error)?
            .join("shell-requests");
        Self::in_directory(directory)
    }

    fn in_directory(directory: PathBuf) -> Result<Self, ArchiveError> {
        fs::create_dir_all(&directory).map_err(shell_error)?;
        restrict_directory(&directory)?;
        let store = Self(Arc::new(ShellRequestStoreInner {
            directory,
            queue: Mutex::new(VecDeque::new()),
            io_lock: Mutex::new(()),
            provider_registered: AtomicBool::new(false),
        }));
        store.remove_stale_files()?;
        Ok(store)
    }

    pub(crate) fn submit(&self, action: &str, paths: Vec<String>) -> Result<(), ArchiveError> {
        let request = ShellRequest {
            version: REQUEST_VERSION,
            action: ShellAction::parse(action)?,
            paths,
            created_at: now_seconds(),
            nonce: secure_nonce()?,
        };
        validate_request(&request)?;
        let path = self.0.directory.join(format!("{}.json", request.nonce));
        let bytes = serde_json::to_vec(&request).map_err(shell_error)?;
        if bytes.len() as u64 > MAX_REQUEST_BYTES {
            return Err(invalid_request("Shell request is too large"));
        }
        let mut file = create_private_file(&path)?;
        file.write_all(&bytes).map_err(shell_error)?;
        file.sync_all().map_err(shell_error)?;
        drop(file);
        let request = self.consume_file(&path)?;
        self.0.queue.lock().map_err(lock_error)?.push_back(request);
        Ok(())
    }

    pub(crate) fn take(&self) -> Result<Vec<ShellRequest>, ArchiveError> {
        Ok(self
            .0
            .queue
            .lock()
            .map_err(lock_error)?
            .pop_front()
            .into_iter()
            .collect())
    }

    pub(crate) fn set_provider_registered(&self, registered: bool) {
        self.0
            .provider_registered
            .store(registered, Ordering::Relaxed);
    }

    pub(crate) fn status(&self) -> ShellIntegrationStatus {
        ShellIntegrationStatus {
            available: cfg!(target_os = "macos"),
            provider_registered: self.0.provider_registered.load(Ordering::Relaxed),
            document_extensions: if cfg!(target_os = "macos") { 17 } else { 0 },
            service_actions: if cfg!(target_os = "macos") { 5 } else { 0 },
        }
    }

    fn consume_file(&self, path: &Path) -> Result<ShellRequest, ArchiveError> {
        let _guard = self.0.io_lock.lock().map_err(lock_error)?;
        if path.parent().and_then(|parent| parent.canonicalize().ok())
            != Some(self.0.directory.canonicalize().map_err(shell_error)?)
        {
            return Err(invalid_request(
                "Shell request is outside the private request folder",
            ));
        }
        let metadata = fs::symlink_metadata(path).map_err(shell_error)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_REQUEST_BYTES {
            let _ = fs::remove_file(path);
            return Err(invalid_request("Shell request is not a small regular file"));
        }
        validate_owner_and_permissions(&self.0.directory, &metadata).inspect_err(|_| {
            let _ = fs::remove_file(path);
        })?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(shell_error)?
            .take(MAX_REQUEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(shell_error)?;
        let request: ShellRequest = serde_json::from_slice(&bytes).map_err(|_| {
            let _ = fs::remove_file(path);
            invalid_request("Shell request JSON is invalid")
        })?;
        let expected_nonce = path.file_stem().and_then(|value| value.to_str());
        let result = validate_request(&request).and_then(|()| {
            if expected_nonce == Some(request.nonce.as_str()) {
                Ok(request)
            } else {
                Err(invalid_request(
                    "Shell request nonce does not match its filename",
                ))
            }
        });
        fs::remove_file(path).map_err(shell_error)?;
        result
    }

    fn remove_stale_files(&self) -> Result<(), ArchiveError> {
        for entry in fs::read_dir(&self.0.directory).map_err(shell_error)? {
            let entry = entry.map_err(shell_error)?;
            if entry.file_type().map_err(shell_error)?.is_file() {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

fn validate_request(request: &ShellRequest) -> Result<(), ArchiveError> {
    if request.version != REQUEST_VERSION {
        return Err(invalid_request("Unsupported shell request version"));
    }
    if request.paths.is_empty() || request.paths.len() > MAX_PATHS {
        return Err(invalid_request(
            "Shell request has an invalid selection size",
        ));
    }
    if request.nonce.len() != 32 || !request.nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_request("Shell request nonce is invalid"));
    }
    let age = now_seconds().saturating_sub(request.created_at);
    if request.created_at > now_seconds().saturating_add(5) || age > MAX_REQUEST_AGE_SECONDS {
        return Err(invalid_request("Shell request is stale"));
    }
    if request
        .paths
        .iter()
        .any(|path| !Path::new(path).is_absolute() || !Path::new(path).exists())
    {
        return Err(invalid_request(
            "Shell request paths must be absolute existing items",
        ));
    }
    Ok(())
}

fn secure_nonce() -> Result<String, ArchiveError> {
    #[cfg(unix)]
    {
        let mut bytes = [0_u8; 16];
        fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(shell_error)?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(not(unix))]
    {
        Err(ArchiveError::new(
            "shell_integration_unavailable",
            "Secure shell requests are not implemented on this platform",
        ))
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<fs::File, ArchiveError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(shell_error)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> Result<fs::File, ArchiveError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(shell_error)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(shell_error)
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

#[cfg(unix)]
fn validate_owner_and_permissions(
    directory: &Path,
    metadata: &fs::Metadata,
) -> Result<(), ArchiveError> {
    use std::os::unix::fs::MetadataExt;
    let directory = fs::metadata(directory).map_err(shell_error)?;
    if metadata.uid() != directory.uid() || metadata.mode() & 0o077 != 0 {
        return Err(invalid_request(
            "Shell request owner or permissions are invalid",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_and_permissions(
    _directory: &Path,
    _metadata: &fs::Metadata,
) -> Result<(), ArchiveError> {
    Ok(())
}

fn invalid_request(message: &str) -> ArchiveError {
    ArchiveError::new("invalid_shell_request", message)
}

fn shell_error(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::new(
        "shell_request_unavailable",
        format!("Could not process the shell request: {error}"),
    )
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> ArchiveError {
    ArchiveError::new(
        "shell_request_unavailable",
        "Shell requests are temporarily unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_request_round_trip_is_validated_and_consumed_once() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected.txt");
        fs::write(&selected, "test").unwrap();
        let store = ShellRequestStore::in_directory(root.path().join("requests")).unwrap();

        store
            .submit(
                "compress_zip",
                vec![selected.to_string_lossy().into_owned()],
            )
            .unwrap();
        let requests = store.take().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, ShellAction::CompressZip);
        assert!(store.take().unwrap().is_empty());
        assert!(fs::read_dir(root.path().join("requests"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn rejects_unknown_stale_relative_and_outside_requests() {
        let root = tempfile::tempdir().unwrap();
        let store = ShellRequestStore::in_directory(root.path().join("requests")).unwrap();
        assert_eq!(
            store
                .submit("delete_everything", vec!["/tmp".into()])
                .unwrap_err()
                .code,
            "invalid_shell_request"
        );
        assert_eq!(
            store
                .submit("open", vec!["relative.zip".into()])
                .unwrap_err()
                .code,
            "invalid_shell_request"
        );

        let outside = root.path().join("outside.json");
        fs::write(&outside, "{}").unwrap();
        assert_eq!(
            store.consume_file(&outside).unwrap_err().code,
            "invalid_shell_request"
        );

        let selected = root.path().join("selected.zip");
        fs::write(&selected, "test").unwrap();
        let stale = ShellRequest {
            version: 1,
            action: ShellAction::Open,
            paths: vec![selected.to_string_lossy().into_owned()],
            created_at: 0,
            nonce: "0123456789abcdef0123456789abcdef".into(),
        };
        let path = store.0.directory.join(format!("{}.json", stale.nonce));
        let mut file = create_private_file(&path).unwrap();
        serde_json::to_writer(&mut file, &stale).unwrap();
        drop(file);
        assert_eq!(
            store.consume_file(&path).unwrap_err().code,
            "invalid_shell_request"
        );
        assert!(!path.exists());
    }
}
