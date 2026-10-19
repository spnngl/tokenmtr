# Releasing TokenMtr

A release is a signed universal `TokenMtr.app` and `.dmg`. Pushing a `v<version>` tag runs `.github/workflows/release.yml`, which builds it, signs it and publishes a GitHub release. Renovate in [spnngl/homebrew-tap](https://github.com/spnngl/homebrew-tap) then bumps the Homebrew cask. `just release` builds the same artifact on the maintainer's Mac for testing.

Every build must be signed by the same certificate, `TokenMtr Local Signing`: macOS ties the users' Keychain and folder grants to that signature. CI imports it from repository secrets.

## One-time setup

1. `just signing-cert` creates the self-signed certificate `TokenMtr Local Signing` (the `IDENTITY` constant in `scripts/signing.nu`). Keep the name and the certificate: the Keychain access-list entry and the macOS folder grants (Documents, Desktop, ...) are tied to the signature, so a new identity makes every user grant again.
2. Install Rust with rustup; it reads `rust-toolchain.toml`, which adds the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets. Put rustup's `~/.cargo/bin` before Homebrew in `PATH`: Homebrew's cargo ignores `rust-toolchain.toml` and has no `x86_64` standard library.
3. Install `pnpm` (the version `packageManager` pins in `apps/desktop/package.json`), Node 22 or newer, `just` and Nushell. Then `pnpm -C apps/desktop install --frozen-lockfile` and `pnpm -C apps/desktop build`: the app crate embeds `apps/desktop/dist` (git-ignored), so `just check` fails to compile it until the UI is built once.

**Done when:** `just check` passes and `security find-identity -v -p codesigning` lists `TokenMtr Local Signing`.

### CI signing secrets

The `.p12` holds the private key: it goes into repository secrets only, never into the repository or a release.

1. In Keychain Access, open the **login** keychain, **My Certificates**, right-click `TokenMtr Local Signing` (the row whose disclosure triangle shows its private key) and choose **Export**. Save it as a Personal Information Exchange (`.p12`) file with a new random password: GitHub masks the secret's value in every log, so a word such as `tokenmtr` turns into `***` wherever it appears.
2. Check that the file holds the private key: `/usr/bin/openssl pkcs12 -in TokenMtr.p12 -info -noout` asks for the password and must print both `Certificate bag` (the certificate) and `Shrouded Keybag` (the private key). Without both the release fails with "The .p12 does not hold both the certificate and the private key".
3. Store the file and its password, then delete the file:

   ```sh
   base64 -i TokenMtr.p12 | gh secret set MACOS_SIGNING_P12_BASE64 --repo spnngl/tokenmtr
   gh secret set MACOS_SIGNING_P12_PASSWORD --repo spnngl/tokenmtr   # prompts for the password
   rm TokenMtr.p12
   ```

**Done when:** `gh secret list --repo spnngl/tokenmtr` lists both secrets.

### Homebrew tap

1. Enable the Renovate app on `spnngl/homebrew-tap`.
2. After the first release, set `sha256` in `Casks/tokenmtr.rb` to the dmg line of the release's `SHA256SUMS`. Renovate needs a real current digest to find the next one.

**Done when:** the tap's CI passes on `main`.

## Checklist

1. Set the new version in the workspace `Cargo.toml` (`[workspace.package] version`). It is the only place: the CLI, the daemon and the app read it, and `tauri.conf.json` has no `version` key on purpose.
2. Move the `CHANGELOG.md` entries from "Unreleased" under the new version and date.
3. Optional local build: `just release --target universal-apple-darwin` for the published dmg (Apple silicon and Intel), or plain `just release` for the host architecture. `--unsigned` skips signing for a local test build. Without the identity the build warns and is unsigned: never publish it.
4. Commit, tag `v<version>`; the human pushes the commit, then the tag.

**Done when:** the release workflow passed, the GitHub release lists the dmg, `SHA256SUMS` and `tokenmtr-signing.pem`, and Renovate's pull request in the tap is merged.

## What the release workflow does

1. Fails unless the tag is `v` plus the workspace version.
2. Imports the `.p12` into a throwaway keychain and trusts the certificate for code signing, so `release.nu` finds the identity. Fails when a secret is missing: an unsigned build is never published.
3. Builds the UI, then runs `just release --target universal-apple-darwin`, which runs `just check` first.
4. Checks the bundle as in [Verify the artifact](#verify-the-artifact): signature and authority, both architectures, `tokenmtr --version` equal to the tag.
5. Publishes the release with the dmg, the public certificate `tokenmtr-signing.pem` and `SHA256SUMS`. The notes give the certificate's SHA-256 fingerprint, the `git shortlog` since the previous tag and GitHub's generated notes.

The job uses no build cache, so a release never builds from state that a pull request wrote. Renovate in the tap reads `SHA256SUMS` to bump the cask's `sha256`: keep the file name and the dmg name `TokenMtr_<version>_universal.dmg`.

## What `just release` does

1. `just check`: Rust format, clippy and tests for the whole workspace, TypeScript typecheck, lint and tests. `bindings.ts` must be current (`just bindings` rewrites it).
2. `scripts/release.nu`: signs when the login keychain holds the identity (unless `--unsigned`), else warns and builds unsigned, then runs `pnpm tauri build --config src-tauri/tauri.release.conf.json [--target <triple>]` with `APPLE_SIGNING_IDENTITY` set.
3. `tauri.release.conf.json` adds two things to `tauri.conf.json`: `beforeBuildCommand` runs `scripts/build-sidecar.nu` (builds `tokenmtr` in release mode and copies it to `apps/desktop/src-tauri/binaries/tokenmtr-<target-triple>`; for `universal-apple-darwin` it builds both Apple triples and merges them with `lipo`) before the frontend build, and `bundle.externalBin` bundles it. Tauri strips the triple, so the daemon binary ends up at `TokenMtr.app/Contents/MacOS/tokenmtr`.
4. The script prints the dmg path: `target/release/bundle/dmg/` for the host build, `target/universal-apple-darwin/release/bundle/dmg/` for the universal one.

The sidecar is not in the base `tauri.conf.json` so that `cargo clippy --workspace` and `cargo test --workspace` build the app crate without staging a binary first.

## Build gotchas

- **Plain dmg layout:** `release.nu` sets `CI=true`, so Tauri's `bundle_dmg.sh` skips the Finder AppleScript that lays out the dmg window. That script fails without a logged-in Finder, and its open window often keeps the volume busy (`hdiutil: couldn't unmount "diskN" - Resource busy`). If a dmg build still fails that way, eject the leftover `TokenMtr` volume in Finder and rerun.
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
