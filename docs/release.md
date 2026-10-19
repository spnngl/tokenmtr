# Releasing TokenMtr

A release is a signed `TokenMtr.app` and `.dmg` built on the maintainer's Mac. There is no CI release job: the signing certificate lives in the login keychain, and macOS ties user grants to that signature.

## One-time setup

1. `just signing-cert` creates the self-signed certificate `TokenMtr Local Signing` (the `IDENTITY` constant in `scripts/signing.nu`). Keep the name and the certificate: the Keychain access-list entry and the macOS folder grants (Documents, Desktop, ...) are tied to the signature, so a new identity makes every user grant again.
2. Install Rust with rustup; it reads `rust-toolchain.toml`, which adds the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets. Put rustup's `~/.cargo/bin` before Homebrew in `PATH`: Homebrew's cargo ignores `rust-toolchain.toml` and has no `x86_64` standard library.
3. Install `pnpm` (the version `packageManager` pins in `apps/desktop/package.json`), Node 22 or newer, `just` and Nushell. Then `pnpm -C apps/desktop install --frozen-lockfile` and `pnpm -C apps/desktop build`: the app crate embeds `apps/desktop/dist` (git-ignored), so `just check` fails to compile it until the UI is built once.

**Done when:** `just check` passes and `security find-identity -v -p codesigning` lists `TokenMtr Local Signing`.

## Checklist

1. Set the new version in the workspace `Cargo.toml` (`[workspace.package] version`). It is the only place: the CLI, the daemon and the app read it, and `tauri.conf.json` has no `version` key on purpose.
2. Move the `CHANGELOG.md` entries from "Unreleased" under the new version and date.
3. Build: `just release --target universal-apple-darwin` for the published dmg (Apple silicon and Intel), or plain `just release` for the host architecture. `--unsigned` skips signing for a local test build. Without the identity the build warns and is unsigned: never publish it.
4. Commit, tag `v<version>`; the human pushes.

**Done when:** the dmg passes [Verify the artifact](#verify-the-artifact) and the tag points at the release commit.

## What `just release` does

1. `just check`: Rust format, clippy and tests for the whole workspace, TypeScript typecheck, lint and tests. `bindings.ts` must be current (`just bindings` rewrites it).
2. `scripts/release.nu`: signs when the login keychain holds the identity (unless `--unsigned`), else warns and builds unsigned, then runs `pnpm tauri build --config src-tauri/tauri.release.conf.json [--target <triple>]` with `APPLE_SIGNING_IDENTITY` set.
3. `tauri.release.conf.json` adds two things to `tauri.conf.json`: `beforeBuildCommand` runs `scripts/build-sidecar.nu` (builds `tokenmtr` in release mode and copies it to `apps/desktop/src-tauri/binaries/tokenmtr-<target-triple>`; for `universal-apple-darwin` it builds both Apple triples and merges them with `lipo`) before the frontend build, and `bundle.externalBin` bundles it. Tauri strips the triple, so the daemon binary ends up at `TokenMtr.app/Contents/MacOS/tokenmtr`.
4. The script prints the dmg path: `target/release/bundle/dmg/` for the host build, `target/universal-apple-darwin/release/bundle/dmg/` for the universal one.

The sidecar is not in the base `tauri.conf.json` so that `cargo clippy --workspace` and `cargo test --workspace` build the app crate without staging a binary first.

## Build gotchas

- **Headless or SSH session:** run `CI=true just release …`. Tauri's `bundle_dmg.sh` then skips the Finder AppleScript that lays out the dmg window, which fails without a logged-in Finder.
- **`rust-objcopy` cannot load `libLLVM.dylib`** (rustup toolchain 1.99.1): link the library next to the tool's expected path, once per toolchain:

  ```sh
  sysroot="$(rustc --print sysroot)"
  ln -s "$sysroot/lib/libLLVM.dylib" "$sysroot/lib/rustlib/aarch64-apple-darwin/lib/libLLVM.dylib"
  ```

## Verify the artifact

- `codesign --verify --deep --strict --verbose=2 <bundle dir>/macos/TokenMtr.app` passes, and `codesign -dv` shows the `TokenMtr Local Signing` authority.
- `lipo -archs TokenMtr.app/Contents/MacOS/tokenmtr` lists `x86_64 arm64` for a universal build.
- `TokenMtr.app/Contents/MacOS/tokenmtr --version` prints the new version.
- Copy the app to `/Applications` and start it with no daemon installed: onboarding offers "Install daemon"; afterwards `launchctl print gui/$(id -u)/dev.tokenmtr.daemon` shows the program path inside the `.app`. Started from the dmg instead, the app asks to be moved to Applications.
- Move the `.app` to another folder and launch it: the app repairs the plist to the new path on launch; `tokenmtr service status` reports running.

## Things the build does not check

- The app is not sandboxed and not notarised: `Entitlements.plist` is empty by design, because the daemon reads agent files across the home folder. Gatekeeper asks once on first launch of a downloaded copy.
