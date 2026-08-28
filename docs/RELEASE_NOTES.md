# Archi 0.4.0 release notes

**Release status:** Production macOS release.

Archi 0.4.0 introduces a quieter, more native macOS archive browser while preserving the local-first, staged-extraction architecture and bounded large-archive browsing of earlier releases.

## Highlights

- Browse archives in an edge-to-edge macOS window with an overlay toolbar and native traffic lights.
- Navigate folders from a hideable sidebar that loads only immediate child folders as they are expanded.
- Search from the toolbar and use native file-type icons throughout the archive listing.
- Use separate Add Files and Add Folder actions from one compact Add menu.
- Choose Extract Selected or Extract All from the Extract menu.
- Open recent archives explicitly through **File → Open Recent** without exposing their paths on the home screen.
- Return to the home screen with **File → Close Archive** without modifying the archive or quitting Archi.
- Use accessible popup menus that dismiss on outside click or Escape and restore keyboard focus.
- Follow the system light or dark appearance with improved dark-mode contrast.
- Read Size, Compressed, and Ratio values in right-aligned numeric columns.

## Performance and privacy

- The folder sidebar filters hidden folders consistently with the archive table.
- Sidebar expansion sends only immediate child folders over the Tauri boundary instead of constructing the entire folder tree in JavaScript.
- Archive listings remain paged in Rust, keeping WebView memory bounded for very large archives.
- Recent archive paths remain local, owner-only, disableable, and clearable. They appear only after the user chooses **File → Open Recent**.
- Archive contents, entry names, passwords, history, and diagnostics are never uploaded.

## Security and reliability

- Extraction safety ceilings cover declared expanded size, actual staged output, path depth, and entry count.
- Quarantine metadata is preserved through archive rewrites, previews, and staged extraction.
- Literal wildcard characters in archive paths are never expanded by the bundled engine.
- Add, delete, rename, extraction, cancellation, and preview operations retain their existing staged and fail-safe behavior.

## Distribution

- Product: Archi 0.4.0
- Bundle identifier: `com.nitivar.archi`
- Minimum system: macOS 13
- Architecture: universal Apple Silicon and Intel
- Archive engine: official 7-Zip 26.02 universal macOS binary
- Packaging: Developer-ID-signed, hardened-runtime, notarized, and stapled DMG

See [Supported formats](SUPPORTED_FORMATS.md), [Known limitations](KNOWN_LIMITATIONS.md), [Privacy](PRIVACY.md), and [Third-party notices](../THIRD_PARTY_NOTICES.md).

## Verification

- Final artifact: `Archi_0.4.0_universal.dmg`
- SHA-256: `5c0c071d0948379010b33d4be94aa82546c4e96d421ed917c16f7d9a82906a69`
- Apple notarization: app submission `0da4daf5-56d0-431b-90dd-a333070f01c4` and DMG submission `6859ab5f-2ed2-4935-af1a-277efe14b9b2`, both accepted
- TypeScript/Vite production build and frontend tests
- Rust formatting, strict Clippy, unit, engine-contract, and security tests
- 100,000-entry listing and 1,000-operation real-engine stress gates
- Universal Mach-O verification for Archi and the bundled `7zz`
- Deep strict code-signature verification
- Apple notarization, stapling, and Gatekeeper assessment

Clean-machine installation, upgrade, Finder lifecycle, and uninstall checks remain part of the manual release checklist and are not claimed by the automated verification summary.
