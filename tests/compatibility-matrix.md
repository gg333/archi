# Sprint 6 compatibility matrix

Observed on macOS arm64 with the bundled 7-Zip 26.02 engine. “Pass” means an automated contract test exercised the operation; “Best effort” is an engine capability that is not a release claim yet.

| Format | Detect/list | Test/extract | Create | Encryption | Evidence |
|---|---:|---:|---:|---:|---|
| ZIP | Pass | Pass | Pass | Pass | Bundled fixture; AES-256 creation; independent `/usr/bin/unzip -t` check |
| 7z | Pass | Pass | Pass | Pass | Bundled fixture; encrypted-header creation and password retry |
| RAR/RAR5 | Pass | Pass | No | Read only | Redistributable libarchive fixture |
| TAR | Pass | Pass | No (MVP) | No | Generated with macOS `bsdtar` |
| TGZ, TXZ | Pass | Pass | Pass | No | Created and round-tripped with the bundled engine |
| TBZ2 | Pass | Pass | No | No | Generated with macOS `bsdtar` |
| GZIP, XZ streams | Pass | Pass | Pass | No | Created and round-tripped with the bundled engine |
| TAR.ZST, Zstandard stream | Pass | Pass | Pass | No | Created with Rust zstd and round-tripped through the bundled engine |
| AR, CPIO | Pass | Pass | No | No | Generated with macOS `ar` and `bsdtar` |
| CAB, ISO, LZH/LHA | Best effort | Best effort | No | Engine-dependent | Not release-validated in the compatibility corpus |

Compound and single-stream files may list as one logical inner file (for example, `archive.tar`). The user can extract and then open that inner archive. ZIP signature detection succeeds even with a wrong extension. Truncated, invalid, password-protected, invalid-method, traversal, absolute-path, link-escape, collision, reserved-name and self-inclusion cases are covered by the engine and security suites.
