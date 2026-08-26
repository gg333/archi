# Archi 0.2.0 known limitations

## Platform and distribution

- The current release candidate contains an Apple Silicon (`arm64`) Archi executable. The bundled 7-Zip engine is universal, but an Intel or universal Archi build is still required before claiming Intel support.
- Windows and Linux releases are not available yet.
- macOS 13 or later is required.
- Finder Services may need to be enabled under **System Settings → Keyboard → Keyboard Shortcuts → Services**.

## Archive operations

- Archi creates only ZIP and 7z archives.
- RAR and other supported non-ZIP/7z formats are read-only.
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
- in-archive document or media previews
- remote URL or cloud-storage browsing
- automatic updates
- antivirus or malware classification
- executing files directly from an archive

Limitations will be narrowed only after compatibility and clean-machine evidence supports a broader release claim.
