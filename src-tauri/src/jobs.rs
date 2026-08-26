use crate::archive::{self, ArchiveError, ArchiveFormat, CompressionLevel, CreationPlan};
use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Deref,
    path::Path,
    process::{Child, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const COMMITTING: u8 = 2;
const DONE: u8 = 3;

#[derive(Clone)]
pub(crate) struct JobManager {
    current: Arc<Mutex<Option<Arc<JobControl>>>>,
    next_id: Arc<AtomicU64>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

pub(crate) struct JobControl {
    id: u64,
    operation: &'static str,
    child: Mutex<Option<Child>>,
    cancel_requested: AtomicBool,
    phase: AtomicU8,
    progress: Mutex<Progress>,
    started: Instant,
}

pub(crate) struct ActiveJob {
    manager: JobManager,
    control: Arc<JobControl>,
}

impl Deref for ActiveJob {
    type Target = Arc<JobControl>;

    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

impl Drop for ActiveJob {
    fn drop(&mut self) {
        self.manager.finish(&self.control);
    }
}

impl Drop for JobControl {
    fn drop(&mut self) {
        if let Ok(slot) = self.child.get_mut() {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[derive(Default)]
struct Progress {
    percent: u8,
    total_bytes: u64,
    current_entry: Option<String>,
    warning_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobSnapshot {
    pub id: u64,
    pub operation: String,
    pub phase: String,
    pub percent: u8,
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_ms: u64,
    pub bytes_per_second: u64,
    pub current_entry: Option<String>,
    pub warning_count: usize,
    pub cancellable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JobOutcome {
    pub elapsed_ms: u64,
    pub warning_count: usize,
}

impl JobManager {
    pub(crate) fn start(
        &self,
        operation: &'static str,
        total_bytes: u64,
    ) -> Result<ActiveJob, ArchiveError> {
        Ok(ActiveJob {
            manager: self.clone(),
            control: self.begin(operation, total_bytes)?,
        })
    }

    pub(crate) fn begin(
        &self,
        operation: &'static str,
        total_bytes: u64,
    ) -> Result<Arc<JobControl>, ArchiveError> {
        let mut current = self.current.lock().map_err(lock_error)?;
        if current.is_some() {
            return Err(ArchiveError::new(
                "job_busy",
                "Another archive operation is already running",
            ));
        }
        let control = Arc::new(JobControl {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            operation,
            child: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
            phase: AtomicU8::new(STARTING),
            progress: Mutex::new(Progress {
                total_bytes,
                ..Progress::default()
            }),
            started: Instant::now(),
        });
        *current = Some(control.clone());
        Ok(control)
    }

    pub(crate) fn finish(&self, control: &Arc<JobControl>) {
        control.phase.store(DONE, Ordering::SeqCst);
        if let Ok(mut current) = self.current.lock() {
            if current
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, control))
            {
                *current = None;
            }
        }
    }

    pub(crate) fn status(&self) -> Result<Option<JobSnapshot>, ArchiveError> {
        self.current
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .map(|control| control.snapshot())
            .transpose()
    }

    pub(crate) fn cancel(&self) -> Result<bool, ArchiveError> {
        let current = self.current.lock().map_err(lock_error)?.clone();
        let Some(control) = current else {
            return Ok(false);
        };
        if control.phase.load(Ordering::SeqCst) >= COMMITTING {
            return Ok(false);
        }
        control.cancel_requested.store(true, Ordering::SeqCst);
        if let Some(child) = control.child.lock().map_err(lock_error)?.as_mut() {
            let _ = child.kill();
        }
        Ok(true)
    }
}

impl JobControl {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    pub(crate) fn set_total_bytes(&self, total_bytes: u64) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.total_bytes = total_bytes;
        }
    }

    fn install(&self, mut child: Child) -> Result<(), ArchiveError> {
        let mut slot = self.child.lock().map_err(lock_error)?;
        if self.cancelled() {
            let _ = child.kill();
        }
        *slot = Some(child);
        self.phase.store(RUNNING, Ordering::SeqCst);
        Ok(())
    }

    fn wait(&self, stdout: &mut File, stderr: &mut File) -> Result<ExitStatus, ArchiveError> {
        loop {
            self.read_progress(stdout, stderr)?;
            let mut slot = self.child.lock().map_err(lock_error)?;
            let child = slot.as_mut().ok_or_else(|| {
                ArchiveError::new("internal_error", "Archive worker was not started")
            })?;
            if let Some(status) = child.try_wait().map_err(|error| {
                ArchiveError::new(
                    "engine_unavailable",
                    format!("Could not monitor 7-Zip: {error}"),
                )
            })? {
                slot.take();
                self.phase.store(COMMITTING, Ordering::SeqCst);
                self.read_progress(stdout, stderr)?;
                return Ok(status);
            }
            drop(slot);
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn read_progress(&self, stdout: &mut File, stderr: &mut File) -> Result<(), ArchiveError> {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map_err(log_error)?;
        stderr.read_to_end(&mut bytes).map_err(log_error)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let chunk = String::from_utf8_lossy(&bytes);
        let mut progress = self.progress.lock().map_err(lock_error)?;
        if let Some(percent) = parse_percent(&chunk) {
            progress.percent = progress.percent.max(percent);
        }
        if let Some(entry) = parse_current_entry(&chunk) {
            progress.current_entry = Some(entry);
        }
        progress.warning_count += warning_count(&chunk);
        Ok(())
    }

    fn complete_output(&self, stdout: &str, stderr: &str) -> Result<JobOutcome, ArchiveError> {
        let text = format!("{stdout}\n{stderr}");
        let mut progress = self.progress.lock().map_err(lock_error)?;
        progress.percent = 100;
        progress.warning_count = warning_count(&text);
        if let Some(entry) = parse_current_entry(&text) {
            progress.current_entry = Some(entry);
        }
        Ok(JobOutcome {
            elapsed_ms: elapsed_ms(self.started),
            warning_count: progress.warning_count,
        })
    }

    fn snapshot(&self) -> Result<JobSnapshot, ArchiveError> {
        let progress = self.progress.lock().map_err(lock_error)?;
        let elapsed_ms = elapsed_ms(self.started);
        let processed_bytes = progress.total_bytes.saturating_mul(progress.percent as u64) / 100;
        let phase = self.phase.load(Ordering::SeqCst);
        Ok(JobSnapshot {
            id: self.id,
            operation: self.operation.to_string(),
            phase: if self.cancelled() && phase < COMMITTING {
                "cancelling"
            } else {
                match phase {
                    STARTING => "preparing",
                    RUNNING => "running",
                    COMMITTING => "finishing",
                    _ => "done",
                }
            }
            .to_string(),
            percent: progress.percent,
            processed_bytes,
            total_bytes: progress.total_bytes,
            elapsed_ms,
            bytes_per_second: processed_bytes.saturating_mul(1_000) / elapsed_ms.max(1),
            current_entry: progress.current_entry.clone(),
            warning_count: progress.warning_count,
            cancellable: phase < COMMITTING && !self.cancelled(),
        })
    }
}

pub(crate) fn run_extract(
    control: &Arc<JobControl>,
    binary: &Path,
    archive_path: &Path,
    staging: &Path,
    entries: &[String],
    password: Option<&str>,
) -> Result<JobOutcome, ArchiveError> {
    run_worker(control, |stdout, stderr| {
        archive::spawn_extract_entries(
            binary,
            archive_path,
            staging,
            entries,
            password,
            stdout,
            stderr,
        )
    })
}

pub(crate) fn run_create(
    control: &Arc<JobControl>,
    binary: &Path,
    archive_path: &Path,
    plan: &CreationPlan,
    format: ArchiveFormat,
    compression: CompressionLevel,
    password: Option<&str>,
) -> Result<JobOutcome, ArchiveError> {
    run_worker(control, |stdout, stderr| {
        archive::spawn_create(
            binary,
            archive_path,
            plan,
            format,
            compression,
            password,
            (stdout, stderr),
        )
    })
}

pub(crate) fn run_test(
    control: &Arc<JobControl>,
    binary: &Path,
    archive_path: &Path,
    password: Option<&str>,
) -> Result<JobOutcome, ArchiveError> {
    run_worker(control, |stdout, stderr| {
        archive::spawn_test(binary, archive_path, password, stdout, stderr)
    })
}

fn run_worker(
    control: &Arc<JobControl>,
    spawn: impl FnOnce(Stdio, Stdio) -> Result<Child, ArchiveError>,
) -> Result<JobOutcome, ArchiveError> {
    if control.cancelled() {
        return Err(cancelled_error());
    }
    let mut stdout = tempfile::NamedTempFile::new().map_err(log_error)?;
    let mut stderr = tempfile::NamedTempFile::new().map_err(log_error)?;
    let child = spawn(
        Stdio::from(stdout.reopen().map_err(log_error)?),
        Stdio::from(stderr.reopen().map_err(log_error)?),
    )?;
    control.install(child)?;
    let status = control.wait(stdout.as_file_mut(), stderr.as_file_mut())?;
    let stdout = read_log(stdout.as_file_mut())?;
    let stderr = read_log(stderr.as_file_mut())?;
    if control.cancelled() {
        Err(cancelled_error())
    } else if status.success() {
        control.complete_output(&stdout, &stderr)
    } else {
        Err(archive::engine_failure(&status, &stdout, &stderr))
    }
}

fn read_log(file: &mut File) -> Result<String, ArchiveError> {
    file.seek(SeekFrom::Start(0)).map_err(log_error)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(log_error)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_percent(text: &str) -> Option<u8> {
    let mut latest = None;
    for line in text.lines() {
        let bytes = line.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'%' {
                continue;
            }
            let mut start = index;
            while start > 0 && bytes[start - 1].is_ascii_digit() {
                start -= 1;
            }
            if start < index
                && line[..start]
                    .chars()
                    .all(|character| character.is_whitespace() || character == '\u{8}')
            {
                latest = std::str::from_utf8(&bytes[start..index])
                    .ok()
                    .and_then(|value| value.parse::<u8>().ok())
                    .filter(|value| *value <= 100)
                    .or(latest);
            }
        }
    }
    latest
}

fn parse_current_entry(text: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        ["+ ", "T ", "- "].into_iter().find_map(|marker| {
            let value = line.rsplit_once(marker)?.1;
            let value = value
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>();
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    })
}

fn warning_count(text: &str) -> usize {
    text.to_ascii_lowercase().matches("warning").count()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn cancelled_error() -> ArchiveError {
    ArchiveError::new("cancelled", "Archive operation was cancelled")
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> ArchiveError {
    ArchiveError::new("internal_error", "Archive job state was unavailable")
}

fn log_error(error: std::io::Error) -> ArchiveError {
    ArchiveError::new(
        "engine_unavailable",
        format!("Could not capture 7-Zip output: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, process::Command};

    #[test]
    fn parses_engine_progress_and_current_entry() {
        let output = "  0%\u{8}\u{8}\u{8}\u{8}    \u{8}\u{8}\u{8}\u{8}T report 99%.txt\n 51% 2";
        assert_eq!(parse_percent(output), Some(51));
        assert_eq!(
            parse_current_entry(output).as_deref(),
            Some("report 99%.txt")
        );
    }

    #[test]
    fn frontend_job_contract_uses_camel_case_fields() {
        let snapshot = JobSnapshot {
            id: 1,
            operation: "extract".to_string(),
            phase: "running".to_string(),
            percent: 50,
            processed_bytes: 1,
            total_bytes: 2,
            elapsed_ms: 3,
            bytes_per_second: 4,
            current_entry: None,
            warning_count: 0,
            cancellable: true,
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            serde_json::json!({
                "id": 1,
                "operation": "extract",
                "phase": "running",
                "percent": 50,
                "processedBytes": 1,
                "totalBytes": 2,
                "elapsedMs": 3,
                "bytesPerSecond": 4,
                "currentEntry": null,
                "warningCount": 0,
                "cancellable": true
            })
        );
    }

    #[test]
    fn progress_tolerates_invalid_and_partial_utf8() {
        let manager = JobManager::default();
        let control = manager.begin("extract", 100).unwrap();
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        stdout.write_all(b" 42%\nT bad-\xff-name\n").unwrap();
        stdout.rewind().unwrap();
        control.read_progress(&mut stdout, &mut stderr).unwrap();
        assert_eq!(control.snapshot().unwrap().percent, 42);
        manager.finish(&control);
    }

    #[test]
    fn active_job_releases_the_slot_during_unwind() {
        let manager = JobManager::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let manager = manager.clone();
            move || {
                let _active = manager.start("extract", 0).unwrap();
                panic!("simulated worker panic");
            }
        }));
        assert!(result.is_err());
        assert!(manager.begin("test", 0).is_ok());
    }

    #[test]
    fn one_job_at_a_time_and_cancel_preserves_existing_files() {
        let manager = JobManager::default();
        let control = manager.begin("extract", 100).unwrap();
        assert_eq!(manager.begin("test", 0).err().unwrap().code, "job_busy");

        let destination = tempfile::tempdir().unwrap();
        let existing = destination.path().join("existing.txt");
        fs::write(&existing, "keep me").unwrap();
        let child = Command::new(archive::bundled_engine().unwrap())
            .args(["b", "-bsp1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        control.install(child).unwrap();
        let started = Instant::now();
        assert!(manager.cancel().unwrap());
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        assert!(!control.wait(&mut stdout, &mut stderr).unwrap().success());
        assert!(control.cancelled());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(fs::read_to_string(existing).unwrap(), "keep me");
        manager.finish(&control);
        assert!(manager.begin("test", 0).is_ok());
    }
}
