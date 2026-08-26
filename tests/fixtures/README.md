# Archive fixtures

- `sample.zip` and `sample.7z` are generated with the pinned bundled 7-Zip 26.02 engine from the files in `files/`.
- `sample.rar` is decoded from libarchive's BSD-licensed `test_read_format_rar.rar.uu` fixture: <https://github.com/libarchive/libarchive/blob/master/libarchive/test/test_read_format_rar.rar.uu>.
- `traversal.tar` contains a `../../outside.txt` header and `symlink.tar` contains a symbolic link. They are rejected by the Sprint 2 preflight before 7-Zip is allowed to extract them.

The contract tests generate encrypted-header, truncated, invalid-method and wrong-extension cases in a temporary directory so passwords and redundant binary fixtures are not committed. On macOS they also generate TAR, TGZ, TBZ2, TXZ, CPIO, AR, GZIP, BZIP2 and XZ samples with native tools or the bundled engine.

See `tests/compatibility-matrix.md` for the observed Sprint 6 operation matrix. Large performance fixtures and the 1,000-operation stress corpus are generated during tests and are never committed.
