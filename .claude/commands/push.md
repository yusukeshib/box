Perform the release workflow:

1. Read the current version from `Cargo.toml` (line 3, format: `version = "X.Y.Z"`).
2. Bump the patch version (e.g. `0.0.5` -> `0.0.6`). Update the version in both:
   - `Cargo.toml` line 3
   - `flake.nix` line 16
3. Run `cargo fmt`.
4. Run `cargo clippy` — if there are any warnings, fix them.
5. Commit all changes with the message `vX.Y.Z` (the new version).
6. Push to remote.
