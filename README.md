# Archive App

A local-first archive manager built with Tauri 2, React, TypeScript, Rust, and a bundled 7-Zip 26.02 engine.

Sprint 5 adds macOS double-click/Open With associations, five Finder Services, a private validated request queue, and Finder-to-app routing for open, extract, test, and compress workflows. The Sprint 4 browsing, settings, safe extraction, ZIP/7z creation, encryption, testing, progress, and cancellation features remain available.

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

The current app is installed at `/Applications/Archive App.app`, ad-hoc signed, and not notarized. Update/uninstall validation, Developer ID signing, and notarization are still required before release use.
