# Archive App

A local-first archive manager built with Tauri 2, React, TypeScript, Rust, and a bundled 7-Zip 26.02 engine.

Revised Sprint 7 completes the feature MVP: recoverable ZIP/7z add, delete and rename; ZIP comments; ZIP/7z volume sets; private recent-archive history; and per-format compression defaults. Existing safe extraction, encryption, testing, progress, cancellation, macOS document associations, and five Finder Services remain available.

```bash
pnpm install
pnpm tauri dev
```

Run the release bundle after `pnpm tauri build --bundles app`:

```bash
open "src-tauri/target/release/bundle/macos/Archive App.app"
```

Validation:

```bash
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The current app is installed at `/Applications/Archive App.app`, ad-hoc signed, and not notarized. Final branding, update/uninstall validation, Developer ID signing, and notarization remain revised Sprint 8 work before production release.
