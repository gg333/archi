use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
};

pub const PINNED_ENGINE_VERSION: &str = "26.02";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveFormat {
    Zip,
    SevenZip,
    TarGzip,
    TarXz,
    TarZstd,
    Gzip,
    Xz,
    Zstd,
}

impl ArchiveFormat {
    fn switch(self) -> Option<&'static str> {
        match self {
            Self::Zip => Some("-tzip"),
            Self::SevenZip => Some("-t7z"),
            Self::Gzip => Some("-tgzip"),
            Self::Xz => Some("-txz"),
            Self::TarGzip | Self::TarXz | Self::TarZstd | Self::Zstd => None,
        }
    }

    pub(crate) fn validate_output(self, output: &Path) -> Result<(), ArchiveError> {
        let expected = self.extension();
        let name = output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.ends_with(&format!(".{expected}")) {
            Ok(())
        } else {
            Err(ArchiveError::new(
                "extension_mismatch",
                format!("The selected format requires a .{expected} output file"),
            ))
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZip => "7z",
            Self::TarGzip => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::TarZstd => "tar.zst",
            Self::Gzip => "gz",
            Self::Xz => "xz",
            Self::Zstd => "zst",
        }
    }

    pub(crate) const fn is_tarball(self) -> bool {
        matches!(self, Self::TarGzip | Self::TarXz | Self::TarZstd)
    }

    pub(crate) const fn is_stream(self) -> bool {
        matches!(self, Self::Gzip | Self::Xz | Self::Zstd)
    }

    pub(crate) fn validate_creation_options(
        self,
        plan: &CreationPlan,
        volume_size: Option<u64>,
        password: Option<&str>,
    ) -> Result<(), ArchiveError> {
        if self.is_stream() && plan.single_file().is_none() {
            return Err(ArchiveError::new(
                "stream_requires_file",
                "GZIP, XZ, and Zstandard streams require exactly one regular file",
            ));
        }
        if !matches!(self, Self::Zip | Self::SevenZip)
            && password.is_some_and(|value| !value.is_empty())
        {
            return Err(ArchiveError::new(
                "encryption_unsupported",
                "Encryption is available only for ZIP and 7z creation",
            ));
        }
        if !matches!(self, Self::Zip | Self::SevenZip) && volume_size.is_some() {
            return Err(ArchiveError::new(
                "volumes_unsupported",
                "Split volumes are available only for ZIP and 7z creation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompressionLevel {
    Store,
    Fast,
    Normal,
    Maximum,
}

impl CompressionLevel {
    fn switch(self) -> &'static str {
        match self {
            Self::Store => "-mx=0",
            Self::Fast => "-mx=3",
            Self::Normal => "-mx=5",
            Self::Maximum => "-mx=9",
        }
    }

    fn zstd_level(self) -> i32 {
        match self {
            Self::Store => 1,
            Self::Fast => 3,
            Self::Normal => 9,
            Self::Maximum => 19,
        }
    }
}

#[derive(Debug)]
pub struct CreationPlan {
    pub root: PathBuf,
    pub total_bytes: u64,
    pub skipped_links: usize,
    pub(crate) names: Vec<String>,
    single_file: Option<PathBuf>,
    listfile: tempfile::NamedTempFile,
}

impl CreationPlan {
    pub(crate) fn single_file(&self) -> Option<PathBuf> {
        self.single_file
            .as_ref()
            .filter(|path| {
                fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
            })
            .cloned()
    }
}

#[derive(Debug)]
pub(crate) struct DeletionPlan {
    listfile: tempfile::NamedTempFile,
}

#[derive(Debug)]
pub(crate) struct RenamePlan {
    pairs: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub packed_size: Option<u64>,
    pub modified: Option<String>,
    pub encrypted: bool,
    pub method: Option<String>,
    pub is_link: bool,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveError {
    pub code: String,
    pub message: String,
}

impl ArchiveError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveError {}

pub fn bundled_engine() -> Result<PathBuf, ArchiveError> {
    let executable_name = if cfg!(windows) { "7zz.exe" } else { "7zz" };

    if let Ok(current_executable) = std::env::current_exe() {
        if let Some(directory) = current_executable.parent() {
            let bundled = directory.join(executable_name);
            if bundled.is_file() {
                return Ok(bundled);
            }
        }
    }

    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("7zz-{}", target_triple()?));
    if development.is_file() {
        Ok(development)
    } else {
        Err(ArchiveError::new(
            "engine_missing",
            format!(
                "The bundled 7-Zip engine was not found at {}",
                development.display()
            ),
        ))
    }
}

pub fn engine_version(binary: &Path) -> Result<String, ArchiveError> {
    let output = run(binary, &[], None, None)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .lines()
        .find(|line| line.starts_with("7-Zip"))
        .map(str::to_owned)
        .ok_or_else(|| {
            ArchiveError::new("engine_protocol", "The 7-Zip version banner was not found")
        })?;

    if version.contains(PINNED_ENGINE_VERSION) {
        Ok(version)
    } else {
        Err(ArchiveError::new(
            "engine_version_mismatch",
            format!("Expected 7-Zip {PINNED_ENGINE_VERSION}, but found {version}"),
        ))
    }
}

pub fn list_archive(
    binary: &Path,
    archive: &Path,
    password: Option<&str>,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    require_file(archive)?;
    let mut args = vec![
        "l".into(),
        "-slt".into(),
        "-ba".into(),
        "-sccUTF-8".into(),
        "--".into(),
    ];
    args.push(archive.as_os_str().to_owned());
    let output = run(binary, &args, password, None)?;
    parse_slt(
        &String::from_utf8_lossy(&output.stdout),
        fallback_entry_name(archive).as_deref(),
    )
}

pub fn prepare_creation(inputs: &[PathBuf], output: &Path) -> Result<CreationPlan, ArchiveError> {
    if !output.is_absolute() || output.file_name().is_none() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The archive output path must be an absolute file path",
        ));
    }
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(ArchiveError::new(
                "output_exists",
                "An item already exists at the archive output path",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ArchiveError::new(
                "invalid_destination",
                format!("Could not inspect the archive output path: {error}"),
            ));
        }
    }
    let output_parent = output
        .parent()
        .ok_or_else(|| ArchiveError::new("invalid_destination", "Output path has no folder"))?
        .canonicalize()
        .map_err(|error| {
            ArchiveError::new(
                "invalid_destination",
                format!("Could not resolve the output folder: {error}"),
            )
        })?;
    if !output_parent.is_dir() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The archive output folder does not exist",
        ));
    }

    prepare_inputs(
        inputs,
        Some(&output_parent.join(output.file_name().unwrap())),
    )
}

pub(crate) fn prepare_addition(
    inputs: &[PathBuf],
    archive: &Path,
) -> Result<CreationPlan, ArchiveError> {
    let archive = archive.canonicalize().map_err(|error| {
        ArchiveError::new(
            "archive_not_found",
            format!("Could not resolve the archive being modified: {error}"),
        )
    })?;
    prepare_inputs(inputs, Some(&archive))
}

fn prepare_inputs(
    inputs: &[PathBuf],
    excluded: Option<&Path>,
) -> Result<CreationPlan, ArchiveError> {
    if inputs.is_empty() {
        return Err(ArchiveError::new(
            "no_inputs",
            "Choose at least one file or folder to archive",
        ));
    }

    let mut canonical = Vec::with_capacity(inputs.len());
    let mut skipped_links = 0;
    for input in inputs {
        if !input.is_absolute() {
            return Err(ArchiveError::new(
                "invalid_source",
                "Archive inputs must use absolute paths",
            ));
        }
        let metadata = fs::symlink_metadata(input).map_err(|error| {
            ArchiveError::new(
                "invalid_source",
                format!("Could not inspect {}: {error}", input.display()),
            )
        })?;
        if metadata.file_type().is_symlink() {
            skipped_links += 1;
            continue;
        }
        let path = input.canonicalize().map_err(|error| {
            ArchiveError::new(
                "invalid_source",
                format!("Could not resolve {}: {error}", input.display()),
            )
        })?;
        if excluded.is_some_and(|excluded| path == excluded) {
            continue;
        }
        canonical.push(path);
    }
    if canonical.is_empty() {
        return Err(ArchiveError::new(
            "no_safe_inputs",
            "No safe inputs remain after excluding links and the archive itself",
        ));
    }
    ensure_same_filesystem(&canonical)?;
    canonical.sort();
    canonical.dedup();

    let mut selected = Vec::<PathBuf>::new();
    for path in canonical {
        if selected
            .iter()
            .any(|parent| parent.is_dir() && path.starts_with(parent))
        {
            continue;
        }
        selected.push(path);
    }
    let mut root = selected[0]
        .parent()
        .ok_or_else(|| ArchiveError::new("invalid_source", "Archive input has no parent"))?
        .to_path_buf();
    for path in &selected[1..] {
        let parent = path
            .parent()
            .ok_or_else(|| ArchiveError::new("invalid_source", "Archive input has no parent"))?;
        while !parent.starts_with(&root) {
            if !root.pop() {
                return Err(ArchiveError::new(
                    "ambiguous_roots",
                    "Archive inputs do not share a filesystem root",
                ));
            }
        }
    }
    if !root.has_root() {
        return Err(ArchiveError::new(
            "ambiguous_roots",
            "Archive inputs do not share a filesystem root",
        ));
    }

    let single_file = (selected.len() == 1 && selected[0].is_file()).then(|| selected[0].clone());

    let mut names = Vec::new();
    let mut total_bytes = 0u64;
    for path in selected {
        collect_creation_entries(
            &path,
            &root,
            excluded,
            &mut names,
            &mut total_bytes,
            &mut skipped_links,
        )?;
    }
    let mut listfile = tempfile::NamedTempFile::new().map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not create an archive input list: {error}"),
        )
    })?;
    for name in &names {
        writeln!(listfile, "{name}").map_err(|error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not write the archive input list: {error}"),
            )
        })?;
    }
    listfile.flush().map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not finish the archive input list: {error}"),
        )
    })?;

    Ok(CreationPlan {
        root,
        total_bytes,
        skipped_links,
        names,
        single_file,
        listfile,
    })
}

pub fn create_archive(
    binary: &Path,
    archive: &Path,
    plan: &CreationPlan,
    format: ArchiveFormat,
    compression: CompressionLevel,
    volume_size: Option<u64>,
    password: Option<&str>,
) -> Result<(), ArchiveError> {
    format.validate_output(archive)?;
    validate_volume_size(volume_size)?;
    format.validate_creation_options(plan, volume_size, password)?;
    if format.is_tarball() {
        let workspace = tempfile::tempdir().map_err(|error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not create archive workspace: {error}"),
            )
        })?;
        let tar = workspace.path().join("payload.tar");
        run(
            binary,
            &tar_args(&tar, plan, compression),
            None,
            Some(&plan.root),
        )?;
        if format == ArchiveFormat::TarZstd {
            encode_zstd(&tar, archive, compression, |_| Ok(()))?;
        } else {
            run(
                binary,
                &stream_args(archive, &tar, format, compression)?,
                None,
                None,
            )?;
        }
    } else if format == ArchiveFormat::Zstd {
        let source = plan.single_file().ok_or_else(|| {
            ArchiveError::new(
                "stream_requires_file",
                "Zstandard streams require exactly one regular file",
            )
        })?;
        encode_zstd(&source, archive, compression, |_| Ok(()))?;
    } else {
        run(
            binary,
            &create_args(archive, plan, format, compression, volume_size, password)?,
            password,
            Some(&plan.root),
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // The arguments map directly to one 7-Zip process.
pub(crate) fn spawn_create(
    binary: &Path,
    archive: &Path,
    plan: &CreationPlan,
    format: ArchiveFormat,
    compression: CompressionLevel,
    volume_size: Option<u64>,
    password: Option<&str>,
    output: (Stdio, Stdio),
) -> Result<Child, ArchiveError> {
    validate_volume_size(volume_size)?;
    format.validate_creation_options(plan, volume_size, password)?;
    spawn_command(
        binary,
        &create_args(archive, plan, format, compression, volume_size, password)?,
        password,
        Some(&plan.root),
        output.0,
        output.1,
    )
}

pub(crate) fn spawn_create_tar(
    binary: &Path,
    archive: &Path,
    plan: &CreationPlan,
    compression: CompressionLevel,
    output: (Stdio, Stdio),
) -> Result<Child, ArchiveError> {
    spawn_command(
        binary,
        &tar_args(archive, plan, compression),
        None,
        Some(&plan.root),
        output.0,
        output.1,
    )
}

pub(crate) fn spawn_compress_stream(
    binary: &Path,
    archive: &Path,
    source: &Path,
    format: ArchiveFormat,
    compression: CompressionLevel,
    output: (Stdio, Stdio),
) -> Result<Child, ArchiveError> {
    spawn_command(
        binary,
        &stream_args(archive, source, format, compression)?,
        None,
        None,
        output.0,
        output.1,
    )
}

pub(crate) fn encode_zstd(
    source: &Path,
    destination: &Path,
    compression: CompressionLevel,
    mut progress: impl FnMut(u64) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let mut input = File::open(source).map_err(|error| {
        ArchiveError::new(
            "invalid_source",
            format!("Could not open {}: {error}", source.display()),
        )
    })?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            ArchiveError::new(
                "creation_failed",
                format!("Could not create {}: {error}", destination.display()),
            )
        })?;
    let mut encoder =
        zstd::stream::Encoder::new(output, compression.zstd_level()).map_err(|error| {
            ArchiveError::new(
                "creation_failed",
                format!("Could not start Zstandard: {error}"),
            )
        })?;
    let mut buffer = [0u8; 128 * 1024];
    let mut processed = 0u64;
    loop {
        let count = input.read(&mut buffer).map_err(|error| {
            ArchiveError::new("creation_failed", format!("Could not read input: {error}"))
        })?;
        if count == 0 {
            break;
        }
        encoder.write_all(&buffer[..count]).map_err(|error| {
            ArchiveError::new(
                "creation_failed",
                format!("Could not write Zstandard stream: {error}"),
            )
        })?;
        processed = processed.saturating_add(count as u64);
        progress(processed)?;
    }
    encoder.finish().map_err(|error| {
        ArchiveError::new(
            "creation_failed",
            format!("Could not finish Zstandard stream: {error}"),
        )
    })?;
    Ok(())
}

pub fn extract_archive(
    binary: &Path,
    archive: &Path,
    destination: &Path,
    password: Option<&str>,
) -> Result<Output, ArchiveError> {
    extract_entries(binary, archive, destination, &[], password)
}

pub fn extract_entries(
    binary: &Path,
    archive: &Path,
    destination: &Path,
    entries: &[String],
    password: Option<&str>,
) -> Result<Output, ArchiveError> {
    require_file(archive)?;
    if destination.exists() && !destination.is_dir() {
        return Err(ArchiveError::new(
            "invalid_destination",
            "The extraction destination is not a folder",
        ));
    }
    std::fs::create_dir_all(destination).map_err(|error| {
        ArchiveError::new(
            "invalid_destination",
            format!("Could not create the extraction folder: {error}"),
        )
    })?;

    let args = extract_args(archive, destination, entries);
    run(binary, &args, password, None)
}

pub(crate) fn spawn_extract_entries(
    binary: &Path,
    archive: &Path,
    destination: &Path,
    entries: &[String],
    password: Option<&str>,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, ArchiveError> {
    require_file(archive)?;
    std::fs::create_dir_all(destination).map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not create the extraction staging folder: {error}"),
        )
    })?;
    crate::safe_paths::quarantine::copy(archive, destination).map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not preserve macOS quarantine metadata: {error}"),
        )
    })?;
    spawn_command(
        binary,
        &extract_args(archive, destination, entries),
        password,
        None,
        stdout,
        stderr,
    )
}

pub(crate) fn engine_failure(status: &ExitStatus, stdout: &str, stderr: &str) -> ArchiveError {
    classify_failure_parts(status, stdout.trim(), stderr.trim())
}

pub fn test_archive(
    binary: &Path,
    archive: &Path,
    password: Option<&str>,
) -> Result<Output, ArchiveError> {
    require_file(archive)?;
    let mut args = vec!["t".into(), "-bsp1".into(), "-bb1".into(), "--".into()];
    args.push(archive.as_os_str().to_owned());
    run(binary, &args, password, None)
}

pub(crate) fn writable_format(path: &Path) -> Option<ArchiveFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "zip" => Some(ArchiveFormat::Zip),
        "7z" => Some(ArchiveFormat::SevenZip),
        _ => None,
    }
}

pub(crate) fn prepare_deletion(entries: &[String]) -> Result<DeletionPlan, ArchiveError> {
    if entries.is_empty() {
        return Err(ArchiveError::new(
            "no_entries",
            "Select at least one archive entry to delete",
        ));
    }
    let mut listfile = tempfile::NamedTempFile::new().map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not create an archive entry list: {error}"),
        )
    })?;
    for entry in entries {
        if entry.is_empty() || entry.contains(['\n', '\r']) {
            return Err(ArchiveError::new(
                "unsafe_path",
                "Archive entry names cannot be empty or contain line breaks",
            ));
        }
        writeln!(listfile, "{entry}").map_err(|error| {
            ArchiveError::new(
                "staging_failed",
                format!("Could not write the archive entry list: {error}"),
            )
        })?;
    }
    listfile.flush().map_err(|error| {
        ArchiveError::new(
            "staging_failed",
            format!("Could not finish the archive entry list: {error}"),
        )
    })?;
    Ok(DeletionPlan { listfile })
}

pub(crate) fn spawn_delete(
    binary: &Path,
    archive: &Path,
    plan: &DeletionPlan,
    password: Option<&str>,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, ArchiveError> {
    require_file(archive)?;
    spawn_command(
        binary,
        &delete_args(archive, plan, password),
        password,
        None,
        stdout,
        stderr,
    )
}

#[cfg(test)]
pub(crate) fn delete_entries(
    binary: &Path,
    archive: &Path,
    plan: &DeletionPlan,
    password: Option<&str>,
) -> Result<Output, ArchiveError> {
    require_file(archive)?;
    run(
        binary,
        &delete_args(archive, plan, password),
        password,
        None,
    )
}

pub(crate) fn spawn_rename(
    binary: &Path,
    archive: &Path,
    plan: &RenamePlan,
    password: Option<&str>,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, ArchiveError> {
    require_file(archive)?;
    spawn_command(
        binary,
        &rename_args(archive, plan, password),
        password,
        None,
        stdout,
        stderr,
    )
}

#[cfg(test)]
pub(crate) fn rename_entries(
    binary: &Path,
    archive: &Path,
    plan: &RenamePlan,
    password: Option<&str>,
) -> Result<Output, ArchiveError> {
    require_file(archive)?;
    run(
        binary,
        &rename_args(archive, plan, password),
        password,
        None,
    )
}

pub(crate) fn prepare_rename(pairs: Vec<(String, String)>) -> Result<RenamePlan, ArchiveError> {
    if pairs.is_empty()
        || pairs.iter().any(|(from, to)| {
            from.is_empty()
                || to.is_empty()
                || from.contains(['\n', '\r'])
                || to.contains(['\n', '\r'])
        })
    {
        return Err(ArchiveError::new(
            "unsafe_path",
            "Archive rename paths cannot be empty or contain line breaks",
        ));
    }
    Ok(RenamePlan { pairs })
}

pub(crate) fn read_zip_comment(path: &Path) -> Result<Option<String>, ArchiveError> {
    if writable_format(path) != Some(ArchiveFormat::Zip) {
        return Ok(None);
    }
    let (comment_offset, comment_len) = zip_comment_location(path)?;
    if comment_len == 0 {
        return Ok(None);
    }
    let mut file = File::open(path).map_err(|error| {
        ArchiveError::new(
            "archive_not_found",
            format!("Could not read ZIP comment: {error}"),
        )
    })?;
    file.seek(SeekFrom::Start(comment_offset))
        .map_err(|error| {
            ArchiveError::new(
                "engine_protocol",
                format!("Could not locate ZIP comment: {error}"),
            )
        })?;
    let mut comment = vec![0; comment_len];
    file.read_exact(&mut comment).map_err(|error| {
        ArchiveError::new(
            "engine_protocol",
            format!("Could not read ZIP comment: {error}"),
        )
    })?;
    Ok(Some(String::from_utf8_lossy(&comment).into_owned()))
}

pub(crate) fn set_zip_comment(path: &Path, comment: &str) -> Result<(), ArchiveError> {
    if writable_format(path) != Some(ArchiveFormat::Zip) {
        return Err(ArchiveError::new(
            "comment_unsupported",
            "Comments can only be edited in single-volume ZIP archives",
        ));
    }
    let bytes = comment.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(ArchiveError::new(
            "comment_too_long",
            "ZIP comments cannot exceed 65,535 UTF-8 bytes",
        ));
    }
    let (comment_offset, _) = zip_comment_location(path)?;
    let eocd_offset = comment_offset - 22;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ArchiveError::new(
                "archive_not_writable",
                format!("Could not edit ZIP comment: {error}"),
            )
        })?;
    file.set_len(eocd_offset + 22).map_err(|error| {
        ArchiveError::new(
            "archive_not_writable",
            format!("Could not resize ZIP comment: {error}"),
        )
    })?;
    file.seek(SeekFrom::Start(eocd_offset + 20))
        .and_then(|_| file.write_all(&(bytes.len() as u16).to_le_bytes()))
        .and_then(|_| file.seek(SeekFrom::Start(eocd_offset + 22)).map(|_| ()))
        .and_then(|_| file.write_all(bytes))
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ArchiveError::new(
                "archive_not_writable",
                format!("Could not save ZIP comment: {error}"),
            )
        })
}

fn zip_comment_location(path: &Path) -> Result<(u64, usize), ArchiveError> {
    require_file(path)?;
    let mut file = File::open(path).map_err(|error| {
        ArchiveError::new(
            "archive_not_found",
            format!("Could not inspect ZIP comment: {error}"),
        )
    })?;
    let len = file
        .metadata()
        .map_err(|error| {
            ArchiveError::new(
                "archive_not_found",
                format!("Could not inspect ZIP comment: {error}"),
            )
        })?
        .len();
    let tail_len = len.min(65_557) as usize;
    if tail_len < 22 {
        return Err(ArchiveError::new(
            "engine_protocol",
            "ZIP end record was not found",
        ));
    }
    file.seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(|error| {
            ArchiveError::new(
                "engine_protocol",
                format!("Could not inspect ZIP end record: {error}"),
            )
        })?;
    let mut tail = vec![0; tail_len];
    file.read_exact(&mut tail).map_err(|error| {
        ArchiveError::new(
            "engine_protocol",
            format!("Could not inspect ZIP end record: {error}"),
        )
    })?;
    for index in (0..=tail_len - 22).rev() {
        if tail[index..index + 4] != *b"PK\x05\x06" {
            continue;
        }
        let comment_len = u16::from_le_bytes([tail[index + 20], tail[index + 21]]) as usize;
        if index + 22 + comment_len == tail_len {
            return Ok((len - tail_len as u64 + index as u64 + 22, comment_len));
        }
    }
    Err(ArchiveError::new(
        "engine_protocol",
        "ZIP end record was not found",
    ))
}

pub(crate) fn spawn_test(
    binary: &Path,
    archive: &Path,
    password: Option<&str>,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, ArchiveError> {
    require_file(archive)?;
    let args = [
        "t".into(),
        "-bsp1".into(),
        "-bb1".into(),
        "--".into(),
        archive.as_os_str().to_owned(),
    ];
    spawn_command(binary, &args, password, None, stdout, stderr)
}

fn target_triple() -> Result<&'static str, ArchiveError> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        ("x86_64", "macos") => Ok("x86_64-apple-darwin"),
        ("x86_64", "windows") => Ok("x86_64-pc-windows-msvc.exe"),
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
        (architecture, operating_system) => Err(ArchiveError::new(
            "unsupported_platform",
            format!("No archive engine is bundled for {architecture}-{operating_system}"),
        )),
    }
}

fn require_file(path: &Path) -> Result<(), ArchiveError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(ArchiveError::new(
            "archive_not_found",
            format!("Archive not found: {}", path.display()),
        ))
    }
}

fn extract_args(archive: &Path, destination: &Path, entries: &[String]) -> Vec<OsString> {
    let mut args = vec![
        "x".into(),
        "-bsp1".into(),
        "-bb1".into(),
        "-y".into(),
        format!("-o{}", destination.display()).into(),
        "--".into(),
        archive.as_os_str().to_owned(),
    ];
    args.extend(entries.iter().map(OsString::from));
    args
}

fn delete_args(archive: &Path, plan: &DeletionPlan, password: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        "d".into(),
        "-bsp1".into(),
        "-bb1".into(),
        "-y".into(),
        "-scsUTF-8".into(),
        format!("-i@{}", plan.listfile.path().display()).into(),
    ];
    if password.is_some_and(|value| !value.is_empty()) {
        args.push("-p".into());
    }
    args.push("--".into());
    args.push(archive.as_os_str().to_owned());
    args
}

fn rename_args(archive: &Path, plan: &RenamePlan, password: Option<&str>) -> Vec<OsString> {
    let mut args = vec!["rn".into(), "-bsp1".into(), "-bb1".into(), "-y".into()];
    if password.is_some_and(|value| !value.is_empty()) {
        args.push("-p".into());
    }
    args.extend(["--".into(), archive.as_os_str().to_owned()]);
    for (from, to) in &plan.pairs {
        args.extend([from.into(), to.into()]);
    }
    args
}

fn create_args(
    archive: &Path,
    plan: &CreationPlan,
    format: ArchiveFormat,
    compression: CompressionLevel,
    volume_size: Option<u64>,
    password: Option<&str>,
) -> Result<Vec<OsString>, ArchiveError> {
    let format_switch = format.switch().ok_or_else(|| {
        ArchiveError::new(
            "invalid_format",
            "This format requires the staged stream creation path",
        )
    })?;
    let mut args = vec![
        "a".into(),
        format_switch.into(),
        compression.switch().into(),
        "-bsp1".into(),
        "-bb1".into(),
        "-y".into(),
        "-scsUTF-8".into(),
        format!("-i@{}", plan.listfile.path().display()).into(),
    ];
    if let Some(bytes) = volume_size {
        args.push(format!("-v{bytes}b").into());
    }
    if password.is_some_and(|value| !value.is_empty()) {
        args.push("-p".into());
        match format {
            ArchiveFormat::Zip => args.push("-mem=AES256".into()),
            ArchiveFormat::SevenZip => args.push("-mhe=on".into()),
            _ => {
                return Err(ArchiveError::new(
                    "encryption_unsupported",
                    "Encryption is available only for ZIP and 7z creation",
                ));
            }
        }
    }
    args.push("--".into());
    args.push(archive.as_os_str().to_owned());
    Ok(args)
}

fn tar_args(archive: &Path, plan: &CreationPlan, compression: CompressionLevel) -> Vec<OsString> {
    vec![
        "a".into(),
        "-ttar".into(),
        compression.switch().into(),
        "-bsp1".into(),
        "-bb1".into(),
        "-y".into(),
        "-scsUTF-8".into(),
        format!("-i@{}", plan.listfile.path().display()).into(),
        "--".into(),
        archive.as_os_str().to_owned(),
    ]
}

fn stream_args(
    archive: &Path,
    source: &Path,
    format: ArchiveFormat,
    compression: CompressionLevel,
) -> Result<Vec<OsString>, ArchiveError> {
    let switch = match format {
        ArchiveFormat::TarGzip | ArchiveFormat::Gzip => "-tgzip",
        ArchiveFormat::TarXz | ArchiveFormat::Xz => "-txz",
        _ => {
            return Err(ArchiveError::new(
                "invalid_format",
                "This stream format is not handled by 7-Zip",
            ));
        }
    };
    Ok(vec![
        "a".into(),
        switch.into(),
        compression.switch().into(),
        "-bsp1".into(),
        "-bb1".into(),
        "-y".into(),
        "--".into(),
        archive.as_os_str().to_owned(),
        source.as_os_str().to_owned(),
    ])
}

pub(crate) fn validate_volume_size(volume_size: Option<u64>) -> Result<(), ArchiveError> {
    if volume_size.is_some_and(|bytes| !(1024..=1024_u64.pow(4)).contains(&bytes)) {
        Err(ArchiveError::new(
            "invalid_volume_size",
            "Archive volumes must be between 1 KiB and 1 TiB",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn generated_archive_paths(
    base: &Path,
    volume_size: Option<u64>,
) -> Result<Vec<PathBuf>, ArchiveError> {
    if volume_size.is_none() {
        return if base.is_file() {
            Ok(vec![base.to_path_buf()])
        } else {
            Err(ArchiveError::new(
                "creation_failed",
                "The archive engine did not produce the requested output",
            ))
        };
    }
    let parent = base.parent().ok_or_else(|| {
        ArchiveError::new("creation_failed", "The archive output has no parent folder")
    })?;
    let prefix = format!("{}.", base.file_name().unwrap().to_string_lossy());
    let mut paths = fs::read_dir(parent)
        .map_err(|error| {
            ArchiveError::new(
                "creation_failed",
                format!("Could not inspect archive volumes: {error}"),
            )
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.strip_prefix(&prefix).is_some_and(|suffix| {
                        suffix.len() == 3 && suffix.bytes().all(|byte| byte.is_ascii_digit())
                    })
                })
        })
        .collect::<Vec<_>>();
    paths.sort();
    let first_name = format!("{prefix}001");
    if paths
        .first()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        != Some(first_name.as_str())
    {
        return Err(ArchiveError::new(
            "creation_failed",
            "The archive engine did not produce a complete volume set",
        ));
    }
    Ok(paths)
}

fn collect_creation_entries(
    path: &Path,
    root: &Path,
    excluded: Option<&Path>,
    names: &mut Vec<String>,
    total_bytes: &mut u64,
    skipped_links: &mut usize,
) -> Result<bool, ArchiveError> {
    if excluded.is_some_and(|excluded| path == excluded) {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ArchiveError::new(
            "invalid_source",
            format!("Could not inspect {}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        *skipped_links += 1;
        return Ok(false);
    }
    if metadata.is_file() {
        *total_bytes = total_bytes.saturating_add(metadata.len());
        names.push(relative_creation_name(path, root)?);
        return Ok(true);
    }
    if !metadata.is_dir() {
        return Err(ArchiveError::new(
            "unsafe_file_type",
            format!("Special files cannot be archived: {}", path.display()),
        ));
    }
    let mut children = fs::read_dir(path)
        .map_err(|error| {
            ArchiveError::new(
                "invalid_source",
                format!("Could not read {}: {error}", path.display()),
            )
        })?
        .map(|entry| {
            entry.map(|entry| entry.path()).map_err(|error| {
                ArchiveError::new(
                    "invalid_source",
                    format!("Could not read an input: {error}"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    let mut included_child = false;
    for child in children {
        included_child |=
            collect_creation_entries(&child, root, excluded, names, total_bytes, skipped_links)?;
    }
    if !included_child {
        names.push(relative_creation_name(path, root)?);
    }
    Ok(true)
}

fn relative_creation_name(path: &Path, root: &Path) -> Result<String, ArchiveError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ArchiveError::new("invalid_source", "Could not derive a relative archive path")
    })?;
    let name = relative.to_str().ok_or_else(|| {
        ArchiveError::new(
            "invalid_source",
            "An archive input name could not be represented as Unicode",
        )
    })?;
    if name.is_empty() || name.contains(['\n', '\r']) {
        return Err(ArchiveError::new(
            "invalid_source",
            "Archive input names cannot be empty or contain line breaks",
        ));
    }
    Ok(name.to_string())
}

#[cfg(unix)]
fn ensure_same_filesystem(paths: &[PathBuf]) -> Result<(), ArchiveError> {
    use std::os::unix::fs::MetadataExt;
    let device = fs::metadata(&paths[0])
        .map_err(|error| {
            ArchiveError::new(
                "invalid_source",
                format!("Could not inspect an input: {error}"),
            )
        })?
        .dev();
    if paths.iter().skip(1).any(|path| {
        fs::metadata(path)
            .map(|metadata| metadata.dev() != device)
            .unwrap_or(true)
    }) {
        Err(ArchiveError::new(
            "ambiguous_roots",
            "Archive inputs span multiple filesystem roots",
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn ensure_same_filesystem(_paths: &[PathBuf]) -> Result<(), ArchiveError> {
    Ok(())
}

fn run(
    binary: &Path,
    args: &[OsString],
    password: Option<&str>,
    current_directory: Option<&Path>,
) -> Result<Output, ArchiveError> {
    let child = spawn_command(
        binary,
        args,
        password,
        current_directory,
        Stdio::piped(),
        Stdio::piped(),
    )?;

    let output = child.wait_with_output().map_err(|error| {
        ArchiveError::new(
            "engine_unavailable",
            format!("Failed to wait for 7-Zip: {error}"),
        )
    })?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(classify_failure(&output))
    }
}

fn spawn_command(
    binary: &Path,
    args: &[OsString],
    password: Option<&str>,
    current_directory: Option<&Path>,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, ArchiveError> {
    let password = password.filter(|value| !value.is_empty());
    if let Some(password) = password {
        if args
            .iter()
            .any(|argument| argument.to_string_lossy().contains(password))
        {
            return Err(ArchiveError::new(
                "unsafe_password_transport",
                "Refusing to place an archive password in process arguments",
            ));
        }
    }

    let mut command = Command::new(binary);
    if let Some((operation, remaining)) = args.split_first() {
        command.arg(operation).arg("-spd").args(remaining);
    }
    command
        .env("LC_ALL", "C")
        .stdin(if password.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(stdout)
        .stderr(stderr);

    if let Some(directory) = current_directory {
        command.current_dir(directory);
    }

    let mut child = command.spawn().map_err(|error| {
        ArchiveError::new(
            "engine_unavailable",
            format!("Failed to start 7-Zip: {error}"),
        )
    })?;

    if let Some(password) = password {
        let prompts = if args
            .first()
            .is_some_and(|command| matches!(command.to_string_lossy().as_ref(), "a" | "d" | "rn"))
        {
            2
        } else {
            1
        };
        write_password(&mut child, password, prompts)?;
    }
    Ok(child)
}

fn classify_failure(output: &Output) -> ArchiveError {
    classify_failure_parts(
        &output.status,
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

fn classify_failure_parts(status: &ExitStatus, stdout: &str, stderr: &str) -> ArchiveError {
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    let normalized = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    let code = if normalized.contains("wrong password") {
        "wrong_password"
    } else if normalized.contains("enter password")
        || normalized.contains("password is required")
        || normalized.contains("can not open encrypted archive")
    {
        "password_required"
    } else if normalized.contains("unsupported method") {
        "unsupported_method"
    } else if normalized.contains("unexpected end")
        || normalized.contains("data error")
        || normalized.contains("crc failed")
    {
        "damaged_archive"
    } else if normalized.contains("is not archive")
        || normalized.contains("cannot open file as archive")
    {
        "invalid_archive"
    } else {
        "engine_failed"
    };
    let status = status
        .code()
        .map_or_else(|| "terminated".to_string(), |value| value.to_string());
    let message = match code {
        "password_required" => "This archive requires a password.".to_string(),
        "wrong_password" => "The archive password is incorrect.".to_string(),
        "damaged_archive" => "The archive is damaged or incomplete.".to_string(),
        "invalid_archive" => "The selected file is not a supported archive.".to_string(),
        "unsupported_method" => {
            "The archive uses a compression or encryption method this engine does not support."
                .to_string()
        }
        _ => format!("7-Zip failed with exit code {status}"),
    };
    ArchiveError::new(code, message)
}

fn write_password(child: &mut Child, password: &str, prompts: usize) -> Result<(), ArchiveError> {
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ArchiveError::new("engine_unavailable", "7-Zip password input was unavailable")
    })?;
    for _ in 0..prompts {
        stdin
            .write_all(password.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|error| {
                ArchiveError::new(
                    "engine_unavailable",
                    format!("Failed to send password to 7-Zip: {error}"),
                )
            })?;
    }
    Ok(())
}

fn parse_slt(text: &str, fallback_path: Option<&str>) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let normalized = text.replace("\r\n", "\n");
    let blocks = normalized
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .collect::<Vec<_>>();
    let fallback_path = (blocks.len() == 1).then_some(fallback_path).flatten();
    blocks
        .into_iter()
        .map(|block| parse_slt_entry(block, fallback_path))
        .collect()
}

fn parse_slt_entry(block: &str, fallback_path: Option<&str>) -> Result<ArchiveEntry, ArchiveError> {
    let mut path = None;
    let mut folder = false;
    let mut size = None;
    let mut packed_size = None;
    let mut modified = None;
    let mut encrypted = false;
    let mut method = None;
    let mut is_link = false;
    let mut link_target = None;

    for line in block.lines() {
        if line.is_empty() || line == "Enter password:" {
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            return Err(ArchiveError::new(
                "engine_protocol",
                format!("7-Zip returned an unrepresentable technical-list line: {line:?}"),
            ));
        };
        match key {
            "Path" => set_once(&mut path, value.to_string(), "Path")?,
            "Folder" => folder = value == "+",
            "Attributes" => folder |= value.starts_with('D'),
            "Mode" => is_link |= value.starts_with('l'),
            "Size" => size = parse_optional_number(value, "Size")?,
            "Packed Size" => packed_size = parse_optional_number(value, "Packed Size")?,
            "Modified" if !value.is_empty() => modified = Some(value.to_string()),
            "Encrypted" => encrypted = value == "+",
            "Method" if !value.is_empty() => method = Some(value.to_string()),
            "Symbolic Link" | "Hard Link" if !value.is_empty() => {
                is_link = true;
                link_target = Some(value.to_string());
            }
            _ => {}
        }
    }

    Ok(ArchiveEntry {
        path: path
            .or_else(|| fallback_path.map(str::to_string))
            .ok_or_else(|| ArchiveError::new("engine_protocol", "7-Zip entry had no path"))?,
        is_directory: folder,
        size,
        packed_size,
        modified,
        encrypted,
        method,
        is_link,
        link_target,
    })
}

fn fallback_entry_name(archive: &Path) -> Option<String> {
    let name = archive.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    for suffix in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        if lower.ends_with(suffix) {
            return Some(name[..name.len() - suffix.len()].to_string() + ".tar");
        }
    }
    for suffix in [".tgz", ".tbz", ".tbz2", ".txz", ".tzst"] {
        if lower.ends_with(suffix) {
            return Some(name[..name.len() - suffix.len()].to_string() + ".tar");
        }
    }
    archive.file_stem()?.to_str().map(str::to_string)
}

fn parse_optional_number(value: &str, field: &str) -> Result<Option<u64>, ArchiveError> {
    if value.is_empty() {
        Ok(None)
    } else {
        value.parse().map(Some).map_err(|_| {
            ArchiveError::new(
                "engine_protocol",
                format!("7-Zip returned an invalid {field}"),
            )
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, field: &str) -> Result<(), ArchiveError> {
    if slot.replace(value).is_some() {
        Err(ArchiveError::new(
            "engine_protocol",
            format!("7-Zip returned duplicate {field} data"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, process, thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn scratch(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("archive-app-{name}-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parses_technical_listing_without_losing_equals_or_unicode() {
        let entries = parse_slt(
            "Path = résumé = final.txt\nFolder = -\nSize = 12\nPacked Size = 9\nModified = 2026-08-26 10:30:00\nEncrypted = +\nMethod = LZMA2\n\n",
            None,
        )
        .unwrap();
        assert_eq!(entries[0].path, "résumé = final.txt");
        assert_eq!(entries[0].size, Some(12));
        assert_eq!(entries[0].modified.as_deref(), Some("2026-08-26 10:30:00"));
        assert!(entries[0].encrypted);
    }

    #[test]
    fn rejects_multiline_names_in_text_protocol() {
        let error = parse_slt("Path = first line\nsecond line\nFolder = -\n", None).unwrap_err();
        assert_eq!(error.code, "engine_protocol");
    }

    #[test]
    fn names_single_streams_without_technical_list_paths() {
        let entries = parse_slt("Size = 20\nPacked Size = 10\n", Some("archive.tar")).unwrap();
        assert_eq!(entries[0].path, "archive.tar");
        assert_eq!(
            fallback_entry_name(Path::new("backup.tbz2")).as_deref(),
            Some("backup.tar")
        );
        assert_eq!(
            fallback_entry_name(Path::new("document.xz")).as_deref(),
            Some("document")
        );
    }

    #[test]
    fn encrypted_round_trip_uses_stdin_password() {
        let root = scratch("encrypted");
        let source = root.join("source");
        let destination = root.join("extracted");
        let archive = root.join("sample.7z");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("hello = नमस्ते.txt"), "archive engine contract").unwrap();

        let engine = bundled_engine().unwrap();
        let plan = prepare_creation(&[source.join("hello = नमस्ते.txt")], &archive).unwrap();
        create_archive(
            &engine,
            &archive,
            &plan,
            ArchiveFormat::SevenZip,
            CompressionLevel::Normal,
            None,
            Some("sprint1-password"),
        )
        .unwrap();
        let entries = list_archive(&engine, &archive, Some("sprint1-password")).unwrap();
        assert_eq!(entries[0].path, "hello = नमस्ते.txt");
        test_archive(&engine, &archive, Some("sprint1-password")).unwrap();
        extract_archive(&engine, &archive, &destination, Some("sprint1-password")).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("hello = नमस्ते.txt")).unwrap(),
            "archive engine contract"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safely_adds_deletes_renames_and_comments_without_mutating_in_place() {
        let root = scratch("rewrite");
        let engine = bundled_engine().unwrap();
        for (name, format) in [
            ("editable.zip", ArchiveFormat::Zip),
            ("editable.7z", ArchiveFormat::SevenZip),
        ] {
            let password = (format == ArchiveFormat::SevenZip).then_some("rewrite-password");
            let archive = root.join(name);
            let first = root.join(format!("{name}-first.txt"));
            let keep = root.join(format!("{name}-keep.txt"));
            let added = root.join(format!("{name}-added.txt"));
            fs::write(&first, "delete me").unwrap();
            fs::write(&keep, "rename me").unwrap();
            fs::write(&added, "added").unwrap();
            let plan = prepare_creation(&[first.clone(), keep.clone()], &archive).unwrap();
            create_archive(
                &engine,
                &archive,
                &plan,
                format,
                CompressionLevel::Normal,
                None,
                password,
            )
            .unwrap();

            let rewrite = crate::safe_paths::ArchiveRewrite::create(&archive).unwrap();
            let plan = prepare_addition(std::slice::from_ref(&added), &archive).unwrap();
            create_archive(
                &engine,
                rewrite.path(),
                &plan,
                format,
                CompressionLevel::Normal,
                None,
                password,
            )
            .unwrap();
            test_archive(&engine, rewrite.path(), password).unwrap();
            rewrite.commit().unwrap();

            let first_name = first.file_name().unwrap().to_string_lossy().into_owned();
            let keep_name = keep.file_name().unwrap().to_string_lossy().into_owned();
            let added_name = added.file_name().unwrap().to_string_lossy().into_owned();
            let rewrite = crate::safe_paths::ArchiveRewrite::create(&archive).unwrap();
            let deletion = prepare_deletion(std::slice::from_ref(&first_name)).unwrap();
            delete_entries(&engine, rewrite.path(), &deletion, password).unwrap();
            test_archive(&engine, rewrite.path(), password).unwrap();
            rewrite.commit().unwrap();

            let renamed = format!("renamed-{name}.txt");
            let rewrite = crate::safe_paths::ArchiveRewrite::create(&archive).unwrap();
            let rename = prepare_rename(vec![(keep_name.clone(), renamed.clone())]).unwrap();
            rename_entries(&engine, rewrite.path(), &rename, password).unwrap();
            test_archive(&engine, rewrite.path(), password).unwrap();
            rewrite.commit().unwrap();
            let paths = list_archive(&engine, &archive, password)
                .unwrap()
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>();
            assert!(!paths.contains(&first_name));
            assert!(!paths.contains(&keep_name));
            assert!(paths.contains(&added_name));
            assert!(paths.contains(&renamed));

            if format == ArchiveFormat::Zip {
                let rewrite = crate::safe_paths::ArchiveRewrite::create(&archive).unwrap();
                set_zip_comment(rewrite.path(), "Sprint 7 comment = नमस्ते").unwrap();
                test_archive(&engine, rewrite.path(), None).unwrap();
                rewrite.commit().unwrap();
                assert_eq!(
                    read_zip_comment(&archive).unwrap().as_deref(),
                    Some("Sprint 7 comment = नमस्ते")
                );
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_installs_and_extracts_zip_and_7z_volume_sets() {
        let root = scratch("volumes");
        let engine = bundled_engine().unwrap();
        let payload = root.join("payload.bin");
        fs::write(&payload, vec![0x5a; 40 * 1024]).unwrap();
        for (name, format) in [
            ("split.zip", ArchiveFormat::Zip),
            ("split.7z", ArchiveFormat::SevenZip),
        ] {
            let workspace = root.join(format!("work-{name}"));
            let installed = root.join(format!("installed-{name}"));
            let extracted = root.join(format!("extracted-{name}"));
            fs::create_dir(&workspace).unwrap();
            let base = workspace.join(name);
            let output = installed.join(name);
            fs::create_dir(&installed).unwrap();
            let plan = prepare_creation(std::slice::from_ref(&payload), &base).unwrap();
            create_archive(
                &engine,
                &base,
                &plan,
                format,
                CompressionLevel::Store,
                Some(8 * 1024),
                None,
            )
            .unwrap();
            let volumes = generated_archive_paths(&base, Some(8 * 1024)).unwrap();
            assert!(volumes.len() > 1);
            let installed =
                crate::safe_paths::install_created_archives(&volumes, &base, &output).unwrap();
            test_archive(&engine, &installed[0], None).unwrap();
            extract_archive(&engine, &installed[0], &extracted, None).unwrap();
            assert_eq!(
                fs::read(extracted.join("payload.bin")).unwrap().len(),
                40 * 1024
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_process_can_be_cancelled() {
        let mut child = Command::new(bundled_engine().unwrap())
            .args(["b", "-bsp1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(150));
        child.kill().unwrap();
        assert!(!child.wait_with_output().unwrap().status.success());
    }

    #[test]
    fn pinned_engine_version_is_available() {
        assert!(engine_version(&bundled_engine().unwrap())
            .unwrap()
            .contains(PINNED_ENGINE_VERSION));
    }

    #[test]
    fn rejects_passwords_in_process_arguments() {
        let error = run(
            &bundled_engine().unwrap(),
            &["t".into(), "-pdo-not-leak".into(), "archive.7z".into()],
            Some("do-not-leak"),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "unsafe_password_transport");
    }

    #[test]
    fn archive_format_requires_matching_output_extension() {
        for (name, format) in [
            ("archive.ZIP", ArchiveFormat::Zip),
            ("archive.7z", ArchiveFormat::SevenZip),
            ("archive.tar.gz", ArchiveFormat::TarGzip),
            ("archive.tar.xz", ArchiveFormat::TarXz),
            ("archive.tar.zst", ArchiveFormat::TarZstd),
            ("archive.gz", ArchiveFormat::Gzip),
            ("archive.xz", ArchiveFormat::Xz),
            ("archive.zst", ArchiveFormat::Zstd),
        ] {
            format
                .validate_output(Path::new("/tmp").join(name).as_path())
                .unwrap();
        }
        assert_eq!(
            ArchiveFormat::SevenZip
                .validate_output(Path::new("/tmp/archive.zip"))
                .unwrap_err()
                .code,
            "extension_mismatch"
        );
    }

    #[test]
    fn stream_creation_rejects_folders_encryption_and_volumes() {
        let root = scratch("stream-options");
        let folder = root.join("folder");
        let output = root.join("output.gz");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("file.txt"), "content").unwrap();
        let plan = prepare_creation(std::slice::from_ref(&folder), &output).unwrap();
        assert_eq!(
            ArchiveFormat::Gzip
                .validate_creation_options(&plan, None, None)
                .unwrap_err()
                .code,
            "stream_requires_file"
        );
        assert_eq!(
            ArchiveFormat::TarGzip
                .validate_creation_options(&plan, None, Some("secret"))
                .unwrap_err()
                .code,
            "encryption_unsupported"
        );
        assert_eq!(
            ArchiveFormat::TarGzip
                .validate_creation_options(&plan, Some(1024), None)
                .unwrap_err()
                .code,
            "volumes_unsupported"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classifies_corruption_password_and_unsupported_method_errors() {
        let status = Command::new(bundled_engine().unwrap())
            .arg("definitely-not-a-command")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        for (message, expected) in [
            ("Unexpected end of data", "damaged_archive"),
            ("Wrong password", "wrong_password"),
            ("Enter password", "password_required"),
            ("Unsupported Method", "unsupported_method"),
            ("Cannot open file as archive", "invalid_archive"),
        ] {
            assert_eq!(
                classify_failure_parts(&status, "", message).code,
                expected,
                "{message}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn staged_extraction_carries_archive_quarantine_marker() {
        let marker = b"0081;66d00000;Archi;00000000-0000-0000-0000-000000000000";
        let root = scratch("quarantine-staging");
        let source = root.join("source.txt");
        let archive = root.join("source.zip");
        let staging = root.join("staging");
        fs::write(&source, "payload").unwrap();
        fs::create_dir(&staging).unwrap();
        let plan = prepare_creation(std::slice::from_ref(&source), &archive).unwrap();
        create_archive(
            &bundled_engine().unwrap(),
            &archive,
            &plan,
            ArchiveFormat::Zip,
            CompressionLevel::Normal,
            None,
            None,
        )
        .unwrap();
        crate::safe_paths::quarantine::write(&archive, marker).unwrap();
        let mut child = spawn_extract_entries(
            &bundled_engine().unwrap(),
            &archive,
            &staging,
            &[],
            None,
            Stdio::null(),
            Stdio::null(),
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            crate::safe_paths::quarantine::read(&staging)
                .unwrap()
                .as_deref(),
            Some(marker.as_slice())
        );
        fs::remove_dir_all(root).unwrap();
    }
}
