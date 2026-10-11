# Archi privacy statement

**Effective date:** with the release of Archi 0.6.0

Archi is a local-first archive utility. Archive creation, listing, testing, modification, and extraction run on your Mac using the bundled 7-Zip engine.

## Data Archi does not collect

Archi does not include analytics, advertising, user tracking, or crash-report uploads. It does not upload:

- archive contents;
- file or archive names;
- archive passwords;
- recent archive history;
- diagnostic logs; or
- usage statistics.

Archi has no account system and does not require an internet connection for archive operations.

## Data stored on your Mac

Archi may store the following inside its macOS application configuration directory for your user account:

- application settings;
- up to ten canonical recent-archive paths when history is enabled; and
- a small rotated diagnostic log.

Recent-history storage is enabled by default. You can disable it or select **Clear History** in Settings. Disabling history deletes the stored recent-archive file.

Diagnostics contain only a timestamp, Archi version, operating system, architecture, 7-Zip version, operation category, and sanitized error code. They do not contain passwords, file contents, or archive entry lists. Logs rotate at 256 KiB and can be exported or deleted explicitly from Settings.

## Temporary files and Finder requests

Archi uses private staging directories during archive operations so output can be validated before it is installed. When the destination is on another volume than the system temporary folder, staging is a hidden `.archive-app-extract-…` folder beside the destination, so finished files are moved into place rather than written twice. Job-owned temporary data is removed after success, failure, or cancellation, and anything a crashed job left behind is cleaned on startup.

Opening, previewing, or dragging an archived file into Finder extracts only that regular file to `~/Library/Caches/com.nitivar.archi/Previews/` inside a random, owner-only directory. The default size limit is 100 MiB and can be configured up to 1 GiB. The copy is read-only with respect to the archive, and these copies are removed when Archi quits and again at the next launch. Dragged-out files keep their archived permissions rather than being read-only.

Finder Services pass selections through private, owner-only request files. Requests are validated, consumed once, and deleted before the action is dispatched. A request Archi cannot handle is shown to you rather than dropped.

## Passwords

Passwords are never saved in settings or history and are not placed in process arguments or diagnostics. So that you are not asked repeatedly, the password for the archive you have open is kept in memory until you close that archive, open another, or quit Archi. Passwords for archives handled from Finder without being opened are used for that operation only.

## Notifications and file access

If you enable completion notifications, macOS controls notification permission and delivery. Archi accesses only files and folders selected through the app, drag-and-drop, file associations, or Finder Services, plus its own configuration and temporary directories.

## Network access and updates

Archi 0.6.0 does not contain an automatic updater or network-based archive features. Apple may perform its own Gatekeeper and notarization checks when you download or open a signed application; those checks are provided by macOS, not Archi telemetry.

## Third-party software

Archi bundles 7-Zip 26.02 and other open-source dependencies. Their notices and license materials are documented in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

This statement should be reviewed whenever telemetry, crash reporting, automatic updates, remote files, accounts, or cloud features are introduced.
