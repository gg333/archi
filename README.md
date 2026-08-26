<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" height="128" alt="Archi app icon">
</p>

<h1 align="center">Archi</h1>

<p align="center">
  A fast, private archive manager for macOS.
</p>

<p align="center">
  <a href="https://github.com/gg333/archi/releases/tag/v0.2.0"><strong>Download Archi 0.2.0</strong></a>
</p>

Archi makes it easy to open, create, inspect, test, modify, and safely extract
archives without sending your files anywhere. It runs locally and uses the
bundled 7-Zip 26.02 engine for archive operations.

## Features

- Browse, search, sort, and test archives before extracting them.
- Extract an entire archive or selected files and folders.
- Create ZIP and 7z archives, including encrypted archives.
- Add, delete, and rename entries in ZIP and 7z archives.
- Create and extract multi-volume ZIP and 7z archives.
- Read and edit ZIP comments.
- Resolve file conflicts with Ask, Replace, Skip, or Keep Both.
- Open completed extraction destinations automatically.
- Open archives from Finder and use Finder Services for common operations.
- Cancel long-running operations and monitor progress, speed, and warnings.

## Requirements

- macOS 13 or later
- Apple Silicon or Intel Mac

Windows and Linux builds are not available yet.

## Install

1. Download `Archi_0.2.0_universal.dmg` from the
   [v0.2.0 release](https://github.com/gg333/archi/releases/tag/v0.2.0).
2. Open the DMG and drag **Archi** into **Applications**.
3. Open Archi from Applications.

The release is signed with an Apple Developer ID and notarized by Apple.

## Supported formats

Archi creates and modifies ZIP and 7z archives. It can browse and extract ZIP,
7z, RAR/RAR5, TAR, GZIP, BZIP2, XZ, AR, CPIO, and several other formats
supported by the bundled engine.

RAR support is read-only; Archi does not create or modify RAR archives. See the
[supported-format matrix](docs/SUPPORTED_FORMATS.md) for operation-level details.

## Privacy and safety

Archive operations run locally. Archi has no accounts, advertising, analytics,
or usage tracking, and it does not upload archive contents, file names,
passwords, history, or diagnostics.

Extraction is staged and checked for unsafe paths, links, collisions, and source
replacement before files are installed in the destination. See the full
[privacy statement](docs/PRIVACY.md) and [known limitations](docs/KNOWN_LIMITATIONS.md).

## Development

Install Node.js, pnpm, Rust, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), then run:

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

Run the automated checks with:

```bash
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

## Legal

Archi bundles third-party software, including 7-Zip 26.02. See
[third-party notices](THIRD_PARTY_NOTICES.md) for licenses and corresponding
source information.

The Archi name, logo, and application icons are reserved brand assets of
Nitivar. See the [brand-assets policy](BRAND_ASSETS.md).

Unless a repository `LICENSE` file states otherwise, publication of this source
code does not grant permission to copy, modify, or redistribute it.
