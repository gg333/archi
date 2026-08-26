# Archi

A local-first archive manager built with Tauri 2, React, TypeScript, Rust, and a bundled 7-Zip 26.02 engine.

Public documentation: [Release notes](docs/RELEASE_NOTES.md) · [Privacy](docs/PRIVACY.md) · [Supported formats](docs/SUPPORTED_FORMATS.md) · [Known limitations](docs/KNOWN_LIMITATIONS.md) · [Third-party notices](THIRD_PARTY_NOTICES.md) · [Brand assets](BRAND_ASSETS.md)

Revised Sprint 7 completes the feature MVP: recoverable ZIP/7z add, delete and rename; ZIP comments; ZIP/7z volume sets; private recent-archive history; and per-format compression defaults. Existing safe extraction, encryption, testing, progress, cancellation, macOS document associations, and five Finder Services remain available.

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

Create an unsigned local test DMG:

```bash
pnpm bundle:macos
```

For a distributable release, install a **Developer ID Application** certificate and set `APPLE_SIGNING_IDENTITY` to its full Keychain identity. Configure notarization with either App Store Connect API-key variables (`APPLE_API_ISSUER`, `APPLE_API_KEY`, and `APPLE_API_KEY_PATH`) or Apple-ID variables (`APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`), then run `pnpm release:macos`. The command refuses to create a public release without both a valid signing identity and complete notarization credentials. Tauri signs the app and bundled 7-Zip sidecar with the hardened runtime, submits the DMG for notarization, and staples the accepted ticket.

The DMG is written to `src-tauri/target/release/bundle/dmg/`. Verify it with `codesign --verify --deep --strict --verbose=2`, `spctl --assess --type execute --verbose=4`, and `xcrun stapler validate`.

Validation:

```bash
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

The current release candidate DMG is Developer-ID signed. A public build is complete only when the DMG also passes Apple notarization, Gatekeeper, stapler, and clean-machine checks.
