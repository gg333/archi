# Archi supported formats

This matrix describes Archi 0.4.0 on macOS with the bundled 7-Zip 26.02 engine. **Supported** means the operation is covered by Archi's automated compatibility corpus. **Best effort** means the bundled engine may handle the format, but Archi does not make a release guarantee for it yet.

| Format | Browse | Test/extract | Create | Encryption | Modify | Multi-volume | Comments |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ZIP | Supported | Supported | Supported | Read and AES-256 create | Add/delete/rename | Create/extract | Read/edit UTF-8 |
| 7z | Supported | Supported | Supported | Read and AES-256 create | Add/delete/rename | Create/extract | No |
| RAR/RAR5 | Supported | Supported | No | Read when supported by engine | No | Engine-dependent | No |
| TAR | Supported | Supported | No | No | No | No | No |
| TGZ / TAR.GZ | Supported | Supported | Supported (`.tar.gz`) | No | No | No | No |
| TBZ2 / TAR.BZ2 | Supported | Supported | No | No | No | No | No |
| TXZ / TAR.XZ | Supported | Supported | Supported (`.tar.xz`) | No | No | No | No |
| GZIP stream | Supported | Supported | Supported | No | No | No | No |
| BZIP2 stream | Supported | Supported | No | No | No | No | No |
| XZ stream | Supported | Supported | Supported | No | No | No | No |
| AR | Supported | Supported | No | No | No | No | No |
| CPIO | Supported | Supported | No | No | No | No | No |
| TAR.ZST | Supported | Supported | Supported (`.tar.zst`) | No | No | No | No |
| Zstandard stream | Supported | Supported | Supported | No | No | No | No |
| CAB | Best effort | Best effort | No | Engine-dependent | No | No | No |
| ISO | Best effort | Best effort | No | Engine-dependent | No | No | No |
| LZH / LHA | Best effort | Best effort | No | Engine-dependent | No | No | No |

## Important behavior

- RAR is read-only. Archi does not create or modify RAR archives.
- GZIP, XZ, and Zstandard streams accept exactly one regular file. Use a TAR format for folders or multiple items.
- Modification is available only for single-volume ZIP and 7z archives.
- ZIP comments are supported; 7z and multi-volume comments are not exposed.
- Compound archives and single compressed streams may initially appear as one logical inner file, such as `archive.tar`. Extract that file and open it separately.
- Content detection uses signature and engine probing; an incorrect extension can produce a non-blocking format warning.
- Unsupported compression or encryption methods may prevent an otherwise recognized archive from opening.
- Encrypted ZIP creation uses AES-256, which some older ZIP utilities cannot read.

The internal test evidence is available in [`tests/compatibility-matrix.md`](../tests/compatibility-matrix.md).
