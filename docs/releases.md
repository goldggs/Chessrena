# Windows portable releases

The `Windows portable release` workflow creates a **draft** GitHub Release when a semantic-version tag is pushed. It verifies that the tag matches the Cargo package version, runs Windows x64 formatting/Clippy/tests and the Python helper syntax check, builds the optimized MSVC executable, then stages:

- `Chessrena-vX.Y.Z-windows-x64.exe`
- `Chessrena-vX.Y.Z-windows-x64.zip` (app, license, README/security notes, agent helper and API guide)
- `SHA256SUMS.txt` for the executable and zip

To prepare a release:

1. Update `version` in `Cargo.toml` and commit the intended release contents.
2. Push a matching tag, for example `v0.1.1` for version `0.1.1`.
3. Review the draft release and CI result. Download and smoke-test the executable on a clean Windows x64 machine before publishing.
4. Publish the draft only after review.

The current workflow **does not sign** the executable. A signing certificate and protected GitHub Actions secrets are required before a signed release; until then, Windows may show SmartScreen warnings. Test each new binary on Windows before publishing.
