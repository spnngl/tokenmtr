# Releasing TokenMtr

A release is a signed `TokenMtr.app` and `.dmg` built on the maintainer's Mac (D51, D57). There is no CI release job: the signing certificate lives in the login keychain.

## One-time setup

1. `just signing-cert` creates the self-signed certificate `TokenMtr Local Signing` (the `IDENTITY` constant in `scripts/signing.nu`). Keep the name and the certificate: Keychain "Always Allow" and the macOS folder grants (Documents, Desktop, ...) are tied to the signature, so a new identity makes every user grant again.
2. Install the toolchain from `rust-toolchain.toml`, `pnpm` 10, Node 24, `just` and Nushell. Then `pnpm -C apps/desktop install --frozen-lockfile`.

## Checklist

1. Set the new version in the workspace `Cargo.toml` (`[workspace.package] version`). It is the only place: the CLI, the daemon and the app read it, and `tauri.conf.json` has no `version` key on purpose.
2. Move the `CHANGELOG.md` entries from "Unreleased" under the new version and date.
3. `just release`.
4. Commit, tag `v<version>`; the human pushes.

## What `just release` does

1. `just check`: Rust format, clippy and tests for the whole workspace, TypeScript typecheck, lint and tests. `bindings.ts` must be current (`just bindings` rewrites it).
2. `scripts/release.nu`: checks the signing identity exists, then runs `pnpm tauri build --config src-tauri/tauri.release.conf.json` with `APPLE_SIGNING_IDENTITY` set.
3. `tauri.release.conf.json` adds two things to `tauri.conf.json`: `beforeBuildCommand` runs `scripts/build-sidecar.nu` (builds `tokenmtr` in release mode, copies it to `apps/desktop/src-tauri/binaries/tokenmtr-<target-triple>`) before the frontend build, and `bundle.externalBin` bundles it. Tauri strips the triple, so the daemon binary ends up at `TokenMtr.app/Contents/MacOS/tokenmtr`.
4. The result is `target/release/bundle/dmg/TokenMtr_<version>_aarch64.dmg` (the path is printed at the end).

The sidecar is not in the base `tauri.conf.json` so that `cargo clippy --workspace` and `cargo test --workspace` build the app crate without staging a binary first.

## Verify the artifact

- `codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/TokenMtr.app` passes, and `codesign -dv` shows the `TokenMtr Local Signing` authority.
- `TokenMtr.app/Contents/MacOS/tokenmtr --version` prints the new version.
- Start the app from the dmg with no daemon installed: onboarding offers "Install daemon"; afterwards `launchctl print gui/$(id -u)/dev.tokenmtr.daemon` shows the program path inside the `.app`.
- Move the `.app` to another folder and launch it: the plist is repaired to the new path (D23); `tokenmtr service status` reports running.

## Things the build does not check

- The app is not sandboxed and not notarised (`Entitlements.plist` is empty by design, D2); Gatekeeper asks once on first launch of a downloaded copy.
- Universal binaries are not built: the dmg matches the build machine's architecture.
