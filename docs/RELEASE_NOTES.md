# Archi 0.5.0 release notes

**Release status:** Production macOS release.

Archi 0.5.0 makes everyday archive work faster with drag-and-drop, filename and file-type filtering, and clearer public documentation of Archi's extraction security model.

## Highlights

- Drag archives onto the home screen to extract them.
- Drag files and folders onto the home screen to start a new archive.
- Drag files and folders into an open, editable ZIP or 7z archive to add them.
- Drag a regular file from an archive directly into Finder to extract a copy there.
- Filter archive entries by filename or file type and sort the Type column.
- Keep large listings bounded: filename and type filtering, sorting, and paging run in Rust before results cross into the WebView.

## Security and reliability

- Publish Archi's threat model, trust boundaries, staged-extraction pipeline, safety ceilings, collision defenses, quarantine behavior, preview restrictions, and explicit limitations in [Security design](SECURITY_DESIGN.md).
- Preserve quarantine metadata when the destination filesystem supports extended attributes. Extraction and archive rewrites now continue on filesystems such as exFAT that explicitly report extended attributes as unsupported; other quarantine I/O errors remain fatal.
- Validate that the five Finder Services keep their registered port name aligned with the packaged executable.
- Keep the latest release only in the supported-version table and retain private vulnerability reporting through GitHub.
- Move the current-archive ref update out of React rendering and retain a packaged-app Copy Path regression check in the release checklist.

## Existing archive workflow

- Browse archives with paged listings, native file icons, a lazily loaded folder sidebar, filename search, type filtering, and sortable columns.
- Open a safe archived file in its default Mac app, or press Spacebar for native Quick Look.
- Extract all entries or selected files and folders, with conflict handling and destination reveal.
- Create ZIP, 7z, TAR.GZ, TAR.XZ, TAR.ZST, GZIP, XZ, and Zstandard files.
- Create encrypted and multi-volume ZIP and 7z archives; add, delete, or rename entries in single-volume ZIP and 7z archives.
- Open archives through file associations and use five Finder Services: Extract Here, Extract to Folder, Test Archive, Compress to ZIP, and Compress with Options.

## Distribution

- Product: Archi 0.5.0
- Bundle identifier: `com.nitivar.archi`
- Minimum system: macOS 13
- Architecture: universal Apple Silicon and Intel
- Archive engine: official 7-Zip 26.02 universal macOS binary
- Packaging: Developer-ID-signed, hardened-runtime, notarized, and stapled DMG

See [Supported formats](SUPPORTED_FORMATS.md), [Known limitations](KNOWN_LIMITATIONS.md), [Privacy](PRIVACY.md), [Security design](SECURITY_DESIGN.md), and [Third-party notices](../THIRD_PARTY_NOTICES.md).

## Verification

- Final artifact: `Archi_0.5.0_universal.dmg`
- SHA-256: `07fc1740912019b6392f89ac707cff33be93b3057e3397ed65cbb0bf957c1077`
- Apple notarization: app submission `de6eb095-b6b7-4f8d-bb7b-1d9754c7b79d` and DMG submission `c0c13dd2-a5ed-4183-be29-4384ff4f80da`, both accepted
- TypeScript/Vite production build and frontend tests
- Rust formatting, strict Clippy, unit, engine-contract, and security tests
- 100,000-entry listing and 1,000-operation real-engine stress gates
- Universal Mach-O verification for Archi and the bundled `7zz`
- Deep strict code-signature verification
- Apple notarization, stapling, and Gatekeeper assessment

Clean-machine installation, upgrade, Finder lifecycle, and uninstall checks remain part of the manual release checklist and are not claimed by the automated verification summary.
