# Archi 0.2.0 release notes

**Release status:** Release candidate. Publication remains gated on Apple notarization, stapling, and clean-machine acceptance.

Archi 0.2.0 is the first feature-complete macOS MVP of the local-first archive manager.

## Highlights

- Open, browse, search, sort, test, and safely extract supported archives without extracting them first.
- Create ZIP and 7z archives from files, folders, mixed selections, and empty folders.
- Create encrypted ZIP and 7z archives; 7z supports encrypted file names.
- Extract all entries or only selected files and folders.
- Safely add, delete, and rename entries in single-volume ZIP and 7z archives.
- Create and extract ZIP and 7z multi-volume sets.
- Read and edit UTF-8 ZIP comments.
- Resolve extraction conflicts with Ask, Replace, Skip, or Keep Both.
- Show progress, current entry, elapsed time, speed, warnings, and cancellation state.
- Open the completed extraction destination in Finder.
- Open archives through file associations and use five Finder Services: Extract Here, Extract to Folder, Test Archive, Compress to ZIP, and Compress with Options.
- Keep up to ten recent archive paths locally, with controls to disable and clear history.

## Safety and privacy

- Extraction is staged and validated before files are committed to the selected destination.
- Traversal paths, absolute paths, archive links, unsafe collisions, and source replacement are blocked.
- Passwords are sent to the bundled engine through standard input, never process arguments or logs.
- Archive contents, names, passwords, and usage data are not uploaded.
- The application uses a restrictive Content Security Policy and narrow Tauri permissions.

## Distribution

- Product: Archi 0.2.0
- Bundle identifier: `com.nitivar.archi`
- Minimum system: macOS 13
- Current build architecture: Apple Silicon (`arm64`)
- Archive engine: official 7-Zip 26.02 universal macOS binary
- Packaging: Developer-ID-signed DMG with hardened runtime

See [Supported formats](SUPPORTED_FORMATS.md), [Known limitations](KNOWN_LIMITATIONS.md), [Privacy](PRIVACY.md), and [Third-party notices](../THIRD_PARTY_NOTICES.md).

## Verification completed

- TypeScript/Vite production build
- Rust formatting and strict Clippy
- 36 Rust unit tests
- 8 active engine contract tests
- 10 security tests
- 100,000-entry listing gate
- 1,000-operation real-engine stress gate
- Developer ID signing of Archi, the bundled `7zz`, and the DMG

Final Gatekeeper, stapler, clean-install, Finder lifecycle, upgrade, and uninstall results will be recorded before publication.
