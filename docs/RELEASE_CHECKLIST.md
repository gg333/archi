# macOS release checklist

Run these checks against each packaged, signed, notarized, and stapled release
candidate. Record the completed checklist with the release notes or release
issue.

## Automated verification

- [ ] Run `pnpm verify:macos -- /absolute/path/to/Archi_<version>_universal.dmg`.
- [ ] Confirm the verifier reports the expected release tag and final SHA-256.
- [ ] Confirm all five packaged Finder Services use the packaged
  `CFBundleExecutable` as their `NSPortName`.

## Packaged application

- [ ] Open a test archive, choose **Copy Path** on an entry, paste it into a text
  field, and confirm the exact archive-entry path was copied.
- [ ] Open, create, test, extract, preview, and modify representative supported
  archives.
- [ ] Confirm cancellation, conflict handling, and destination reveal behavior.

## Finder Services

Test all five entries after every executable, product-name, bundle, or
`Info.plist` change:

- [ ] **Extract Here with Archi** extracts beside the selected archive.
- [ ] **Extract to Folder… with Archi** asks for and uses the chosen destination.
- [ ] **Test with Archi** opens Archi and reports the archive test result.
- [ ] **Compress to ZIP with Archi** creates the expected sibling ZIP.
- [ ] **Compress with Options… in Archi** opens the creation dialog with the
  selected Finder items.

## Clean-machine lifecycle

- [ ] Install and launch from the DMG on a clean supported Mac.
- [ ] Verify Gatekeeper acceptance, upgrade behavior, file associations, Finder
  Service discovery, and removal of Services after uninstall.
