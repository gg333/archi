# 7-Zip console engine

- Version: 26.02, released 25 June 2026
- Upstream binary: https://www.7-zip.org/a/7z2602-mac.tar.xz
- Upstream source: https://www.7-zip.org/a/7z2602-src.tar.xz
- Download archive SHA-256: `1cf6760579502f87e591ff5c73a005ec50b3e4d6f507e8b038382d563c3175b9`
- Bundled `7zz` SHA-256: `9c56cf3379a0d8544e9244958b96fdc7c17f9ce70f5a160eb2b41f5f3df96d8c`
- Included source archive SHA-256: `cf967c98bca02a4b8b16375f441825a8e141362f14be1969bbec8e1ca0bff9dd`
- Architectures: universal Mach-O (`arm64`, `x86_64`)
- License: [License.txt](./License.txt)
- Full GNU LGPL 2.1 text: [LGPL-2.1.txt](./LGPL-2.1.txt)
- unRAR restriction: [unRarLicense.txt](./unRarLicense.txt)
- Exact corresponding source: [7z2602-src.tar.xz](./7z2602-src.tar.xz)

Tauri resolves external binaries by target triple, so the same verified universal
executable is stored under the `aarch64`, `x86_64`, and `universal` target names.

The upstream macOS executable is linker-signed ad hoc. Release packaging must
sign the nested executable with the application's Developer ID before the outer
application is signed and notarized.
