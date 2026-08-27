# Archi 0.3.0 release notes

**Release status:** Production macOS release.

Archi 0.3.0 expands archive creation and adds safe native file previews while preserving the local-first, staged-extraction architecture introduced in 0.2.0.

## Highlights

- Open a safe regular file from an archive in its normal macOS application without extracting the whole archive.
- Press Spacebar or choose **Quick Look** to preview supported files with the native macOS Quick Look panel.
- Create TAR.GZ, TAR.XZ, and TAR.ZST archives.
- Create GZIP, XZ, and Zstandard single-file streams.
- Choose all new formats from the New Archive dialog or make one the default in Settings.
- Configure a temporary-preview size limit from 10 MiB to 1 GiB; the default is 100 MiB.
- Use the redesigned public website with an authentic app screenshot, mobile navigation, current format matrix, and direct release download.

## Preview safety and privacy

- Only a selected regular file is extracted into a random owner-only directory under `~/Library/Caches/com.nitivar.archi/Previews/`.
- Directories, links, special files, executables, scripts, Mach-O files, and entries above the configured preview limit are blocked.
- Preview contents never pass through JavaScript or WebView memory.
- Opening or previewing a file does not modify the archive; changes in the external application are discarded from Archi's perspective.
- Stale preview directories are removed on startup after approximately 24 hours.

## Existing archive features

- Browse, search, sort, test, and safely extract supported archives.
- Extract all entries or selected files and folders with Ask, Replace, Skip, or Keep Both conflict handling.
- Create encrypted ZIP and 7z archives, including encrypted 7z file names.
- Add, delete, and rename entries in single-volume ZIP and 7z archives.
- Create and extract ZIP and 7z multi-volume sets and edit UTF-8 ZIP comments.
- Open completed extraction destinations in Finder and use five Finder Services.

## Distribution

- Product: Archi 0.3.0
- Bundle identifier: `com.nitivar.archi`
- Minimum system: macOS 13
- Architecture: universal Apple Silicon and Intel
- Archive engine: official 7-Zip 26.02 universal macOS binary
- Packaging: Developer-ID-signed, hardened-runtime, notarized, and stapled DMG

See [Supported formats](SUPPORTED_FORMATS.md), [Known limitations](KNOWN_LIMITATIONS.md), [Privacy](PRIVACY.md), and [Third-party notices](../THIRD_PARTY_NOTICES.md).

## Verification

- Final artifact: `Archi_0.3.0_universal.dmg`
- SHA-256: `512792d13aeaee379c12a1b111a53cb6bfeebd0592cc748b181319b196f5de8e`
- Apple notarization: app submission `0cd8d24c-eafc-4c4d-9d2e-be10dcacb41c` and DMG submission `b6f75f73-cb09-4f20-98c6-752fabebcb4e`, both accepted with no issues
- TypeScript/Vite production build
- Rust formatting and strict Clippy
- Rust unit, engine-contract, security, 100,000-entry, and 1,000-operation stress tests
- Universal Mach-O verification for Archi and the bundled `7zz`
- Deep strict code-signature verification
- Apple notarization, stapling, and Gatekeeper assessment

Clean-machine installation, upgrade, Finder lifecycle, and uninstall checks remain part of the manual release checklist and are not claimed by the automated verification summary.
