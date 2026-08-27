# Archi 0.3.0 known limitations

## Platform and distribution

- The macOS release is a universal Apple Silicon and Intel application.
- Windows and Linux releases are not available yet.
- macOS 13 or later is required.
- Finder Services may need to be enabled under **System Settings → Keyboard → Keyboard Shortcuts → Services**.

## Archive operations

- Archi creates ZIP, 7z, TAR.GZ, TAR.XZ, TAR.ZST, GZIP, XZ, and Zstandard files. Other supported formats are read-only.
- GZIP, XZ, and Zstandard streams accept one regular file; folders and multiple selections require a TAR format.
- Add, delete, and rename are limited to single-volume ZIP and 7z archives.
- Comments are editable only for ZIP. 7z and multi-volume comments are not supported.
- Symbolic links are skipped during archive creation. Archives containing links or special files are rejected during safe extraction rather than recreating them.
- Entry names containing line breaks are rejected because the bundled engine's technical text listing cannot represent them safely.
- ACLs, ownership, macOS extended attributes, and platform-specific metadata may not round-trip across every format.
- Some recognized formats and compression/encryption methods remain engine-dependent. See [Supported formats](SUPPORTED_FORMATS.md).

## Performance and jobs

- Archi runs one archive job at a time. Starting another operation while one is active reports that Archi is busy rather than running both concurrently.
- The 100,000-entry gate passes through Rust-side paging, but performance still depends on archive structure, storage speed, compression method, and available memory.
- Progress percentages may be approximate or unavailable when the archive engine cannot determine a total size.

## Extraction safety ceiling

Archi validates declared expanded size against a user-configurable limit and asks before extracting archives with unknown or excessive declared size. This reduces accidental expansion risk but cannot guarantee detection of every decompression bomb when an archive omits or misreports size metadata.

## Features not included

- RAR creation
- password recovery or cracking
- archive repair or recovery records
- self-extracting archive creation
- archive mounting
- editing files opened or previewed from an archive back into the archive
- remote URL or cloud-storage browsing
- automatic updates
- antivirus or malware classification
- opening scripts, applications, or executable files directly from an archive

Limitations will be narrowed only after compatibility and clean-machine evidence supports a broader release claim.
