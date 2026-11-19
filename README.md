# Bilibili service

Independent Windows x64 JSON-lines service for LazyBuddy. Video search, official QR login, homepage recommendations, followed-UP video feed, multi-P/UGC collection selection and controlled MP4 resources. No embedded browser or decoder runtime. Real QR confirmation and new-process secure credential restore have been verified in an isolated account scope.

- [API and security boundaries](docs/module-api.md)
- [Dependency licenses](licenses/README.md)

Build and test: `cargo test --locked`. Prepare a complete package with `powershell -File scripts/package.ps1 -Destination <absolute-staging-root>`; each immutable version requires a new destination/version. Host integration uses installed packages only.

Protocol references: MIT-licensed [bpi-rs 0.3.1](https://github.com/Yuelioi/bpi-rs) and the upstream endpoints listed in the API document. No SDK code is bundled. Dependencies and exact versions are in Cargo.lock.
