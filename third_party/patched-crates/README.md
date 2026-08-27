# Patched crates

## glib 0.18.5

Archi vendors the crates.io `glib` 0.18.5 source because Tauri 2.11.5's Linux
GTK3 stack requires the 0.18 API. The published 0.18 release contains
[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html),
while the first published fixed release, 0.20.0, is not compatible with that
stack.

The vendored copy has only the upstream fix from
[gtk-rs-core pull request 1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343):
the `g_variant_get_child` out-pointer is mutable and passed as `&mut p`. The
original crates.io checksum is recorded in `src-tauri/Cargo.lock` history as
`233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`.

Remove this patch when Tauri's Linux dependencies support `glib` 0.20 or later.
The vendored crate remains licensed under its included MIT license.
