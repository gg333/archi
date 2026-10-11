# Archi 0.6.0 release notes

**Release status:** Signed and notarized macOS build. The manual release checklist has not been completed for this build.

Archi 0.6.0 opens compressed tarballs directly, extracts archives that contain links, edits archives in place more naturally, and fixes a set of problems found in a full review of 0.5.0.

## Highlights

- Open compressed tarballs (.tar.gz, .tar.bz2, .tar.xz, .tar.zst) straight to their contents, and extract, preview, or drag files from them.
- Extract archives that contain links: symbolic links are left out and counted, and hard links are extracted as copies of the file they name.
- Add files into the folder you are viewing inside a ZIP or 7z archive, and replace an existing file after confirming.
- Enter an archive's password once; Archi remembers it in memory until the archive is closed.
- Cancel a slow open, with a progress shelf that appears only when opening takes a while.
- Drop an archive on the window to open it. Dropping several asks before extracting them next to the originals.
- Drag large files and scripts out to Finder; drag-out now follows extraction rules instead of preview rules.
- Scroll through a folder instead of paging, with Command-A selecting every item in it.
- Use a standard menu bar: About and Settings in the application menu, a new Archive menu, an Open Recent submenu, and items that disable when they do not apply.
- See recent archives on the welcome screen.

## Finder and desktop

- Finder's Extract Here, Extract to Folder, Test, and Compress to ZIP no longer replace the archive you are browsing.
- Extracting from Finder puts an archive with several top-level items into a new folder named after it.
- A Finder selection of several archives continues past a password prompt or a damaged archive and ends with a summary.
- A Finder request Archi cannot handle is shown instead of being dropped, and selections of up to 1,000 items are accepted.
- Quitting or closing the window while an operation runs asks first.

## Fixes

- Passwords that happen to appear in the archive's name or path are accepted.
- File names such as aux.c, con.txt, or a name ending in a dot no longer block an archive on macOS.
- Extraction and archive creation work on exFAT and FAT drives.
- Extraction and creation stage on the destination's volume and move finished files into place instead of writing them twice.
- Keyboard shortcuts no longer start a second operation while one is running, and Command-A and Command-C work normally in the filter field.
- New Archive replaces an existing file once the save panel has confirmed it, and can replace an existing split-volume set after asking.
- Large selections are prepared quickly before extraction or deletion.
- Cancelling an archive edit no longer leaves a hidden temporary copy beside the archive.
- A deleted or unmounted default extraction folder no longer prevents settings from loading.

## Interface

- Errors in New Archive and Settings appear inside the dialog.
- Right-clicking a selected row keeps the selection and acts on all of it.
- Error, change, and progress banners sit beneath the toolbar; error titles are plain language.
- Dates are shown in your locale, sizes use the same units as Finder, and long names no longer push columns out of view.
- The sidebar follows navigation, and the context menu can be opened and used from the keyboard.
- Encrypted entries show a lock beside the name in place of the Encrypted column.

## Behaviour that changed

- Dropping a single archive opens it; in 0.5.0 it was extracted beside the original.
- The password for the open archive is kept in memory until it is closed; in 0.5.0 it was kept for one operation only. See Privacy.
- Copies made for Open, Quick Look, and drag-out are removed when Archi quits and at launch, instead of after about 24 hours.
- File > Open Recent is a submenu; the Open Recent dialog is gone.

## Security and privacy

- [Security design](SECURITY_DESIGN.md) is rewritten for this release, including link handling, staging on the destination volume, the exFAT rename fallback, password handling, and drag-out rules.
- [Privacy](PRIVACY.md) now describes the in-memory password, preview copy cleanup, and the hidden staging folder used on other volumes.
- On exFAT and FAT volumes a file cannot be installed atomically without replacement; Archi checks that the name is free and then renames.

## Distribution

- Product: Archi 0.6.0
- Bundle identifier: `com.nitivar.archi`
- Minimum system: macOS 13
- Architecture: universal Apple Silicon and Intel
- Archive engine: official 7-Zip 26.02 universal macOS binary
- Packaging: Developer-ID-signed, hardened-runtime, notarized, and stapled DMG

See [Supported formats](SUPPORTED_FORMATS.md), [Known limitations](KNOWN_LIMITATIONS.md), [Privacy](PRIVACY.md), [Security design](SECURITY_DESIGN.md), and [Third-party notices](../THIRD_PARTY_NOTICES.md).

## Verification

- Final artifact: `Archi_0.6.0_universal.dmg`
- SHA-256: `29916ec39a94442a82f093ef01790bb6266067169458d7a5336581be8105969d`
- Apple notarization: app submission `057213ed-b311-4458-964a-a8b040820637` and DMG submission `57adf059-dfdd-46e0-b192-74f17f61ace4`, both accepted
- TypeScript/Vite production build and frontend tests
- Rust formatting, strict Clippy, unit, engine-contract, and security tests
- 100,000-entry listing and selection gates, and the 1,000-operation real-engine stress gate
- Universal Mach-O verification for Archi and the bundled `7zz`
- Deep strict code-signature verification
- Apple notarization, stapling, and Gatekeeper assessment of the app and the DMG
- Release verifier (`pnpm verify:macos`) against the `v0.6.0` tag

The packaged-application, Finder Services, and clean-machine checks in the release checklist have not been performed for this build and are not claimed here. The artifact was built from the commit immediately before this record; the two differ only in these release notes, the screenshot, and its captions, none of which are packaged into the app.
