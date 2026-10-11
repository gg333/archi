# Archi 0.6.1 known limitations

## Platform and distribution

- The macOS release is a universal Apple Silicon and Intel application.
- Windows and Linux releases are not available yet.
- macOS 13 or later is required.
- Finder Services may need to be enabled under **System Settings → Keyboard → Keyboard Shortcuts → Services**.

## Archive operations

- Archi creates ZIP, 7z, TAR.GZ, TAR.XZ, TAR.ZST, GZIP, XZ, and Zstandard files. Other supported formats are read-only.
- GZIP, XZ, and Zstandard streams accept one regular file; folders and multiple selections require a TAR format.
- Add, delete, and rename are limited to single-volume ZIP and 7z archives. Items are added to the folder being browsed; replacing an existing file asks first, and a file cannot replace a folder of the same name or the reverse.
- Comments are editable only for ZIP. 7z and multi-volume comments are not supported.
- Symbolic links are skipped during archive creation. During extraction, symbolic links are left out and counted in the result; they are never recreated, and a folder that holds only symbolic links is not created. Symbolic links cannot be opened or previewed. A hard link is extracted as its own copy of the file it names, so the extracted files no longer share storage; a hard link that names something outside the archive is skipped.
- Opening or extracting a compressed tarball (`.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar.zst`) decompresses the whole stream each time; opening can be cancelled.
- Entry names containing line breaks are rejected because the bundled engine's technical text listing cannot represent them safely.
- ACLs, ownership, macOS extended attributes, and platform-specific metadata may not round-trip across every format.
- Some recognized formats and compression/encryption methods remain engine-dependent. See [Supported formats](SUPPORTED_FORMATS.md).

## Performance and jobs

- Archi runs one archive job at a time. Starting another operation while one is active reports that Archi is busy rather than running both concurrently.
- The 100,000-entry gate passes through Rust-side paging, but performance still depends on archive structure, storage speed, compression method, and available memory.
- Extraction and creation are staged on the destination's volume and finished files are moved into place. When the destination is on another volume than the system temporary folder, the staging folder is a hidden `.archive-app-extract-…` folder beside the destination; it is removed when the job ends, or at the next launch after a crash.
- On exFAT and FAT volumes macOS stores extended attributes in `._` companion files, which Archi counts among the extracted files.
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
