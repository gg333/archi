# 7-Zip console engine

- Version: 26.02, released 25 June 2026
- Upstream binary: https://github.com/ip7z/7zip/releases/download/26.02/7z2602-mac.tar.xz
- Upstream source: https://github.com/ip7z/7zip/releases/download/26.02/7z2602-src.tar.xz
- Download archive SHA-256: `1cf6760579502f87e591ff5c73a005ec50b3e4d6f507e8b038382d563c3175b9`
- Bundled `7zz` SHA-256: `9c56cf3379a0d8544e9244958b96fdc7c17f9ce70f5a160eb2b41f5f3df96d8c`
- Architectures: universal Mach-O (`arm64`, `x86_64`)
- License: [License.txt](./License.txt)

The current Tauri target uses the filename `7zz-aarch64-apple-darwin` because
Tauri resolves external binaries by target triple. The file itself is universal.
Copy the same verified upstream binary to the x86_64 target-triple filename when
the Intel build job is added.

The upstream macOS executable is linker-signed ad hoc. Release packaging must
sign the nested executable with the application's Developer ID before the outer
application is signed and notarized.
