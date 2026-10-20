# Install TokenMtr

TokenMtr runs on macOS and on Linux (amd64 and arm64). The numbered steps below are for macOS; Linux
has [its own section](#linux).

On macOS, run TokenMtr from `/Applications`: the background service refuses a binary inside a
disk image (`/Volumes/…`) or a translocated copy (`…/AppTranslocation/…`), because launchd would
point at a path that disappears. The CLI `tokenmtr` lives at
`/Applications/TokenMtr.app/Contents/MacOS/tokenmtr`; to call it by name:

```sh
ln -s /Applications/TokenMtr.app/Contents/MacOS/tokenmtr /usr/local/bin/tokenmtr
```

## Where TokenMtr keeps its files

| What | Path |
|---|---|
| Config | `$TOKENMTR_CONFIG` (absolute path), else `$XDG_CONFIG_HOME/tokenmtr/config.toml` when `XDG_CONFIG_HOME` is absolute, else `~/.config/tokenmtr/config.toml` |
| Store, backups, exports | macOS `~/Library/Application Support/dev.tokenmtr.app/`; Linux `$XDG_DATA_HOME/dev.tokenmtr.app/`, else `~/.local/share/dev.tokenmtr.app/` (exports in `exports/`) |
| Logs | macOS `~/Library/Logs/tokenmtr/`; Linux `$XDG_STATE_HOME/tokenmtr/`, else `~/.local/state/tokenmtr/` |
| Cache | macOS `~/Library/Caches/dev.tokenmtr.app/`; Linux `$XDG_CACHE_HOME/dev.tokenmtr.app/`, else `~/.cache/dev.tokenmtr.app/` |
| Daemon service | macOS LaunchAgent `~/Library/LaunchAgents/dev.tokenmtr.daemon.plist`; Linux systemd user unit `$XDG_CONFIG_HOME/systemd/user/tokenmtr.service`, else `~/.config/systemd/user/tokenmtr.service` |

`tokenmtr config path` prints the config path in use.

## Homebrew

```sh
brew install --cask spnngl/tap/tokenmtr
```

The cask installs the app to `/Applications` and links `tokenmtr` into Homebrew's `bin`. It also
removes the quarantine flag, so step 2 does not apply. It does not start the daemon: Homebrew
sandboxes cask install steps away from launchd. The onboarding installs it
([step 3](#3-first-run)). `brew upgrade` keeps the daemon, and the app restarts it on the new
version at launch. Then continue at [step 3](#3-first-run).

**Done when:** `TokenMtr.app` is in `/Applications` and `tokenmtr --version` prints the version.

## 1. Install the app

Without Homebrew, download the `.dmg` from the [GitHub releases](https://github.com/spnngl/tokenmtr/releases) and [verify it](#verify-a-download).

1. Open the `.dmg`.
2. Drag `TokenMtr.app` onto `Applications`.
3. Eject the disk image.

**Done when:** `TokenMtr.app` is in `/Applications`.

## 2. First launch: Open Anyway

macOS blocks an app that is not notarized. Since macOS 15, the only way through is System Settings.

1. Open `/Applications/TokenMtr.app`. macOS says it cannot verify the app: click **Done**.
2. Open System Settings → Privacy & Security, scroll to **Security**.
3. Click **Open Anyway** next to "TokenMtr was blocked", then authenticate.
4. Open `/Applications/TokenMtr.app` again and click **Open**.

macOS asks once. Updates keep the answer because every release is signed with the same
certificate (see [Signing](#signing-maintainers)).

**Done when:** the menu-bar icon appears.

## 3. First run

The full window opens an onboarding, in order:

1. Detected agents and profiles.
2. Claude transcript retention: Claude Code deletes transcripts older than `cleanupPeriodDays`
   (default 30). Click **Keep transcripts for 10 years** to set it to 3650, the only change
   TokenMtr ever makes to another tool's files.
3. Install the background daemon (see [step 4](#4-daemon-without-the-app)). The app shows
   "Move TokenMtr to Applications" when it runs from the disk image: quit, move it, reopen it.
4. Allow notifications.

TokenMtr then imports the history on disk, which can take a moment, and fetches EUR rates on the
first run.

**Done when:** the menu-bar recap shows your usage and `tokenmtr service status` reports a running daemon.

## 4. Daemon without the app

The daemon is a launchd LaunchAgent that runs `tokenmtr run` at login. launchd restarts it only
after a crash or a failed exit, at most every 30 seconds; a clean exit (a requested shutdown, or a
second instance finding the first one running) leaves it stopped until the next login or
`service install`. The app installs it during onboarding. A headless install:

```sh
tokenmtr service install
tokenmtr service status
```

Run `service install` again after you move the app: the plist stores the binary's path. The app
repairs it on every launch.

**Done when:** `service status` prints a running pid, and `~/Library/LaunchAgents/dev.tokenmtr.daemon.plist` exists.

## 5. Folder access

macOS asks per app before the daemon reads or writes a protected folder: Documents, Desktop,
Downloads, Pictures, Movies, Music, iCloud Drive (`~/Library/Mobile Documents`), cloud storage
(`~/Library/CloudStorage`, for example Dropbox), other apps' containers
(`~/Library/Containers`, `~/Library/Group Containers`) and external volumes (`/Volumes`). The
defaults sit outside them: exports go to `~/Library/Application Support/dev.tokenmtr.app/exports/`.

Grant access only when a sink path or a project's working directory is in one of those folders:

1. Open System Settings → Privacy & Security → **Full Disk Access** (or **Files and Folders**).
2. Click **+** and add `TokenMtr.app/Contents/MacOS/tokenmtr`.
3. Restart the daemon: **Restart daemon** in Settings, or `tokenmtr service install`.

Without access nothing blocks: a refused project falls back to its working directory as its key and
records one `protected_folder` issue; retry from **Retry** on `/projects` or by restarting the
daemon. A `[[projects]]` rule in [`config.md`](config.md) avoids the prompt without any grant.

`tokenmtr doctor` lists the sinks and working directories under protected folders.

**Done when:** `tokenmtr doctor` reports no `protected_folder` issue.

## 6. Keychain access for quota

The quota poll reads Claude Code's OAuth token, read-only, from:

- the `default` profile: the login-keychain item `Claude Code-credentials`, then
  `~/.claude/.credentials.json`;
- any other profile: `<config_dir>/.credentials.json`, then the item
  `Claude Code-credentials-<first 8 hex characters of sha256(config_dir)>`.

The item belongs to Claude Code, so macOS refuses the daemon's background read until you allow it.
Polls never show a prompt: after a refusal the quota shows `keychain_denied` and the daemon reads
only the credentials files. To allow it once:

1. Click **Connect Claude account** on the Quota screen or in the menu bar recap, or run
   `tokenmtr quota connect` while the daemon runs.
2. macOS asks whether `tokenmtr` may use the item: enter your login password and choose
   **Always Allow**.

The daemon then polls silently. The grant is tied to the code signature, which stays the same
across updates of a signed build. If the prompt never appears, grant access by hand:

1. Open Keychain Access, select the **login** keychain and search for `Claude Code-credentials`.
2. Double-click the item, open **Access Control**, click **+** and add
   `/Applications/TokenMtr.app/Contents/MacOS/tokenmtr` (⇧⌘G accepts the path).
3. Click **Save Changes** and enter your login password.
4. Restart the daemon: **Restart daemon** in Settings, or `tokenmtr service install`.

**Done when:** `tokenmtr quota` reports `ok` for the profile, also after an update and relaunch.

## 7. Uninstall

With Homebrew, `brew uninstall --cask --zap tokenmtr` quits the app and removes the daemon, the
app, the login item, the config, the store, its backups, the logs and the cache. A plain
`brew uninstall --cask tokenmtr` keeps the daemon, which also survives `brew upgrade`: run
`tokenmtr service uninstall` before it. Without Homebrew:

1. Optional: erase TokenMtr's data with the CLI before deleting it:

   ```sh
   tokenmtr erase --service --yes                  # store, backups, logs, the plist
   tokenmtr erase --delete-config --service --yes  # the same, plus the config file
   ```

   `erase` stops the daemon first. It leaves sink outputs and agent files in place; without
   `--service`, it restarts the daemon, which rebuilds the store from the agent files still on disk.
2. Without `erase`, run `tokenmtr service uninstall`.
3. Delete `TokenMtr.app`.
4. Remove the login item in System Settings → General → Login Items & Extensions.
5. Optional: delete `~/Library/Caches/dev.tokenmtr.app`.

**Done when:** `launchctl print gui/$(id -u)/dev.tokenmtr.daemon` fails, `dev.tokenmtr.daemon.plist`
is gone and no `tokenmtr` process runs.

## Linux

The packages require glibc 2.43 or newer: Ubuntu 26.04 and later, Fedora 44 and later, Debian
testing and openSUSE Tumbleweed. Older releases refuse the install. Both architectures are
published: `amd64` or `x86_64`, and `arm64` or `aarch64`.

### Install

Download the package for your architecture from the
[GitHub releases](https://github.com/spnngl/tokenmtr/releases), [verify it](#verify-a-download),
then:

```sh
sudo apt install ./tokenmtr_<version>_amd64.deb         # Debian, Ubuntu
sudo dnf install ./tokenmtr-<version>-1.x86_64.rpm      # Fedora
sudo zypper install ./tokenmtr-<version>-1.x86_64.rpm   # openSUSE
```

The package installs the app as `/usr/bin/tokenmtr-desktop` with an application-menu entry
(**TokenMtr**), and the CLI as `/usr/bin/tokenmtr`. The daemon runs the CLI binary. The only
runtime dependency beyond glibc is `libayatana-appindicator3`, which draws the tray icon.

**Done when:** `tokenmtr --version` prints the version and **TokenMtr** is in the application menu.

### First run

Open **TokenMtr** from the application menu: the main window shows the onboarding of
[step 3](#3-first-run), and the tray icon appears in the panel. At login the app starts in the
tray only.

- **Tray icon:** GNOME shows none without the AppIndicator extension. Ubuntu ships it enabled;
  on Fedora Workstation run `sudo dnf install gnome-shell-extension-appindicator` and enable the
  extension. Linux trays send no click events, so open the recap from the tray menu's
  **Show recap**.
- **Global shortcut:** works under X11 sessions only. Under Wayland the registration fails and
  the app logs it.
- **Quota:** Claude Code keeps its token in `~/.claude/.credentials.json` on Linux, and TokenMtr
  reads that file only. `tokenmtr quota connect` is a macOS command.
- **Folder access:** nothing to grant. The sections on protected folders and the Keychain
  ([step 5](#5-folder-access), [step 6](#6-keychain-access-for-quota)) apply to macOS only.

### Daemon

The daemon is a systemd user unit, `tokenmtr.service`, that runs `/usr/bin/tokenmtr run`. systemd
restarts it only after a crash or a failed exit, at most every 30 seconds; a clean exit leaves it
stopped until the next login or `service install`. The app installs it during onboarding. A
headless install:

```sh
tokenmtr service install
tokenmtr service status
journalctl --user -u tokenmtr   # the daemon's log
```

- A user unit starts at login and stops at logout. On a server or after `ssh`, keep it running
  with `loginctl enable-linger "$USER"`.
- A package upgrade does not restart the daemon. The app restarts an outdated one when it
  launches; without the app, run `tokenmtr service install` after the upgrade.
- `tokenmtr service install` refuses to run when systemd loads `tokenmtr.service` from another
  file, for example the unit of a different `HOME`: it would restart that daemon.
- Large `~/.claude/projects` trees can exhaust the inotify limit that file watching uses. Raise
  `fs.inotify.max_user_watches` with `sysctl` if the daemon lags behind new transcripts.

**Done when:** `systemctl --user status tokenmtr` shows `active (running)`.

### Uninstall

1. Optional: erase TokenMtr's data with the CLI, as in [step 7](#7-uninstall).
2. Without `erase`, run `tokenmtr service uninstall`. Do this before removing the package: the
   unit points at `/usr/bin/tokenmtr`.
3. Remove the package: `sudo apt remove tokenmtr` or `sudo dnf remove tokenmtr`.
4. Delete the autostart entry in `~/.config/autostart/`, if the app created one.

**Done when:** `systemctl --user status tokenmtr` reports the unit as not found, and no
`tokenmtr` process runs.

## Verify a download

Every release lists a `SHA256SUMS` file, an SPDX software bill of materials
(`tokenmtr_<version>.spdx.json`) and two kinds of Sigstore proof, all produced by the release
workflow without any long-lived key. Either check proves the file came from that workflow at the
release's tag.

With the GitHub CLI, for the dmg, a deb or an rpm:

```sh
gh attestation verify tokenmtr_<version>_amd64.deb --repo spnngl/tokenmtr
```

With `cosign` alone, offline: verify the signed checksum list, then the checksums.

```sh
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-identity https://github.com/spnngl/tokenmtr/.github/workflows/release.yml@refs/tags/v<version> \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum -c --ignore-missing SHA256SUMS
```

**Done when:** the command prints a successful verification, and `sha256sum` prints `OK` for each
file you downloaded.

## Troubleshooting

Start with `tokenmtr doctor`: agent roots per profile, file counts, last checkpoints, issue counts,
format drift, unresolved working directories, paths under protected folders, and Claude's
`cleanupPeriodDays`.

| Symptom | Check |
|---|---|
| App shows "daemon offline" | `tokenmtr service status`; read `~/Library/Logs/tokenmtr/launchd.err.log` (macOS) or run `journalctl --user -u tokenmtr` (Linux); run `tokenmtr service install`. |
| "Move TokenMtr to Applications" | The app runs from the disk image or a translocated copy. Move it to `/Applications` and open it from there. |
| "Update required" | The daemon speaks an older control protocol than the app. Click **Restart daemon from this app**: it repairs the plist to the bundled binary. |
| Quota says `keychain_denied` | TokenMtr is missing from the item's access list: [step 6](#6-keychain-access-for-quota). After an update, `codesign -dv --verbose=2 /Applications/TokenMtr.app` must show `Authority=TokenMtr Local Signing`; another authority, or none, means the app was rebuilt or re-signed: reinstall it from the release `.dmg` and add it again. To compare the certificate itself, run `codesign -d --extract-certificates=/tmp/tokenmtr-cert /Applications/TokenMtr.app && shasum -a 256 /tmp/tokenmtr-cert0`: the hash must equal the fingerprint in the release notes. |
| Quota says `signed_out` or `token_expired` | Run `claude` and sign in again; TokenMtr never refreshes the token. |
| Usage older than 30 days is missing | `doctor` shows `cleanupPeriodDays`: Claude already deleted those transcripts. Set it as in step 3. |
| `protected_folder` issue | [Step 5](#5-folder-access). |

## Signing (maintainers)

Release builds sign the app and the bundled CLI with the self-signed certificate **TokenMtr Local
Signing** until Developer ID notarization replaces it. The release workflow imports the same
certificate from repository secrets ([release.md](release.md#ci-signing-secrets)); each GitHub
release attaches its public part, `tokenmtr-signing.pem`. The name is the constant `IDENTITY` in
[`scripts/signing.nu`](../scripts/signing.nu); `just release` passes it as `APPLE_SIGNING_IDENTITY`
whenever the login keychain holds it, so local builds are signed too and keep the Keychain grant
across rebuilds. Without the certificate `just release` warns and builds unsigned: macOS then asks
for the Keychain again after every rebuild.

1. Run `just signing-cert` once per Mac: it creates the certificate in the login keychain and asks
   you to trust it for code signing. It does nothing if the certificate exists;
   `just signing-cert --dry-run` prints the commands ([`scripts/signing-cert.nu`](../scripts/signing-cert.nu)).
2. Keep the certificate: a new one changes the signature, and every user redoes the Keychain and
   folder grants.

**Done when:** `security find-identity -p codesigning` lists `TokenMtr Local Signing`.
