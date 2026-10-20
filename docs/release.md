# Releasing TokenMtr

A release is a signed universal `TokenMtr.app` and `.dmg`, plus a `.deb` and an `.rpm` for each of amd64 and arm64. Pushing a `v<version>` tag runs `.github/workflows/release.yml`, which builds them, signs and attests them, and publishes one GitHub release. Renovate in [spnngl/homebrew-tap](https://github.com/spnngl/homebrew-tap) then bumps the Homebrew cask. `just release` builds the host's artifacts for testing: the dmg on a Mac, the deb and rpm on Linux.

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
2. Move the `CHANGELOG.md` entries from "Unreleased" under `## [<version>] - <date>`, and update the comparison links at the end of the file. The release notes start with that section; the workflow fails when it is empty.
3. Optional local build: `just release --target universal-apple-darwin` for the published dmg (Apple silicon and Intel), or plain `just release` for the host architecture. `--unsigned` skips signing for a local test build. Without the identity the build warns and is unsigned: never publish it.
4. Commit, open a pull request and merge it. Then tag the merged commit and push the tag: `main` takes rebase merges only, which rewrite the commit, so a tag made before the merge points at a commit on no branch, and the workflow fails.

   ```sh
   git switch main && git pull --ff-only
   git tag v<version> && git push origin v<version>
   ```

A version with a pre-release suffix (`0.2.0-rc.1`) is not supported: the deb and rpm version schemes order it differently, and the workflow rejects it.

**Done when:** the release workflow passed, the GitHub release lists ten assets (the dmg, two debs, two rpms, `tokenmtr-signing.pem`, the SBOM, `SHA256SUMS` and its signature), and Renovate's pull request in the tap is merged.

## What the release workflow does

Three files, so the signing secrets and the write permission each reach one job only:

- `release.yml` runs on the tag and calls the other two. Its `publish` job is the only one that can write the release or request an OIDC token.
- `release-macos.yml` builds, signs and verifies the dmg (steps 2 to 4 below).
- `release-linux.yml` builds and checks the deb and rpm on `ubuntu-26.04` and `ubuntu-26.04-arm`, then installs the deb on a fresh runner of each architecture (see [Linux packages](#linux-packages)).

1. `release.yml` fails unless the tagged commit is on `main`, the tag is `v` plus the workspace version, without a pre-release suffix, and `CHANGELOG.md` has entries under that version.
2. macOS: imports the `.p12` into a throwaway keychain and trusts the certificate for code signing, so `release.nu` finds the identity. Fails when a secret is missing: an unsigned build is never published.
3. macOS: builds the UI, then runs `just release --target universal-apple-darwin`, which runs `just check` first.
4. macOS: checks the bundle as in [Verify the artifact](#verify-the-artifact): signature and authority, both architectures, `tokenmtr --version` equal to the tag.
5. `publish`, only when every build and check passed: generates the SBOM `tokenmtr_<version>.spdx.json` from the lockfiles (syft; `.syft.yaml` excludes `fuzz/`), writes `SHA256SUMS` over the dmg, packages, certificate and SBOM, signs it with keyless cosign (`SHA256SUMS.sigstore.json`), attests provenance and the SBOM for the dmg, debs and rpms, and publishes the release. The notes give the version's `CHANGELOG.md` section, the certificate's SHA-256 fingerprint, the commands to verify a download, the `git shortlog` since the previous tag and GitHub's generated notes.

A failed build publishes nothing. The jobs use no build cache, so a release never builds from state that a pull request wrote. Renovate in the tap reads `SHA256SUMS` to bump the cask's `sha256`: keep the file name and the dmg name `TokenMtr_<version>_universal.dmg`.

## Linux packages

`just release` on Linux needs the packages of `.github/actions/linux-deps` and prints `bundle/deb/*.deb` and `bundle/rpm/*.rpm`; it does not sign them. Tauri names the packages after `productName`, which `tauri.linux.conf.json` sets to `tokenmtr` (a test pins it to the CLI name). That file also holds the maintainer, the descriptions, the runtime dependencies Tauri does not add (glibc: `libc6 (>= 2.43)` for the deb, `libc.so.6(GLIBC_2.43)(64bit)` for the rpm, because Tauri passes an rpm dependency as a bare capability name and a `glibc >= 2.43` string would never resolve; and `libayatana-appindicator3`, which the tray loads at run time and `ldd` cannot see) and the extra files (`linux/copyright`, `LICENSE`, the changelog). The packages are built on Ubuntu 26.04, so they require its glibc; building on an older image lowers the requirement.

`release-linux.yml` fails the build unless, for each architecture:

- the deb and rpm have the expected names, version and architecture, the maintainer is set, and each dependency above appears exactly once;
- both install `/usr/bin/tokenmtr`, `/usr/bin/tokenmtr-desktop` and the desktop entry, and the deb the copyright and changelog, the rpm the licence;
- `lintian --fail-on error` and `rpmlint` pass. When one reports a false positive, suppress that tag in a file under `apps/desktop/src-tauri/linux/`, with the reason;
- the unpacked `tokenmtr --version` equals the tag;
- on a fresh runner without the `-dev` packages, `apt-get install ./tokenmtr_<version>_<arch>.deb` succeeds, `tokenmtr --version` runs, `ldd` finds no missing library and `libayatana-appindicator3-1` is installed.

The rpm is not installed in CI; `rpm -qpR` and `rpmlint` check it.

## What `just release` does

1. `just check`: Rust format, clippy and tests for the whole workspace, TypeScript typecheck, lint and tests. `bindings.ts` must be current (`just bindings` rewrites it).
2. `scripts/release.nu`: on macOS signs when the login keychain holds the identity (unless `--unsigned`), else warns and builds unsigned; on Linux it never signs. Then it runs `pnpm tauri build --config src-tauri/tauri.release.conf.json [--target <triple>]` with `APPLE_SIGNING_IDENTITY` set.
3. Tauri merges `tauri.macos.conf.json` or `tauri.linux.conf.json` into `tauri.conf.json` by OS: the bundle targets (`app`, `dmg` or `deb`, `rpm`) and the platform settings. `tauri.release.conf.json` adds two things: `beforeBuildCommand` runs `scripts/build-sidecar.nu` (builds `tokenmtr` in release mode and copies it to `apps/desktop/src-tauri/binaries/tokenmtr-<target-triple>`; for `universal-apple-darwin` it builds both Apple triples and merges them with `lipo`) before the frontend build, and `bundle.externalBin` bundles it. Tauri strips the triple, so the daemon binary ends up at `TokenMtr.app/Contents/MacOS/tokenmtr`.
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

On macOS:

- `codesign --verify --deep --strict --verbose=2 <bundle dir>/macos/TokenMtr.app` passes, and `codesign -dv` shows the `TokenMtr Local Signing` authority.
- `lipo -archs TokenMtr.app/Contents/MacOS/tokenmtr` lists `x86_64 arm64` for a universal build.
- `TokenMtr.app/Contents/MacOS/tokenmtr --version` prints the new version.
- Copy the app to `/Applications` and start it with no daemon installed: onboarding offers "Install daemon"; afterwards `launchctl print gui/$(id -u)/dev.tokenmtr.daemon` shows the program path inside the `.app`. Started from the dmg instead, the app asks to be moved to Applications.
- Move the `.app` to another folder and launch it: the app repairs the plist to the new path on launch; `tokenmtr service status` reports running.
- Quit the app, run `tokenmtr service uninstall` and launch it again: it reinstalls the daemon, as after a `brew upgrade`; `tokenmtr service status` reports running.

On Linux, CI covers the package checks above. By hand, on a graphical session of each family (Ubuntu 26.04 with GNOME on Wayland, Fedora 44 with KDE):

- `apt install ./tokenmtr_<version>_amd64.deb` (or `dnf install` the rpm) and open **TokenMtr** from the application menu: the main window opens and the tray icon shows, on GNOME after enabling the AppIndicator extension.
- Onboarding installs the daemon: `systemctl --user status tokenmtr` is `active (running)` and `FragmentPath` is `~/.config/systemd/user/tokenmtr.service`.
- The tray menu opens the recap (**Show recap**), and the recap sits at a corner of the work area.
- Log out and in: the app starts in the tray only, with no window.

## Things the build does not check

- Nothing runs the Linux app in CI: only the build, the package checks and `tokenmtr --version`. The tray and the recap placement need the manual check above.
- The app is not sandboxed and not notarised: `Entitlements.plist` is empty by design, because the daemon reads agent files across the home folder. Gatekeeper asks once on first launch of a downloaded copy.
