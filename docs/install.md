# Install TokenMtr

macOS only. Paths below assume `TokenMtr.app` in `/Applications`. The CLI `tokenmtr` lives at
`/Applications/TokenMtr.app/Contents/MacOS/tokenmtr`; to call it by name:

```sh
ln -s /Applications/TokenMtr.app/Contents/MacOS/tokenmtr /usr/local/bin/tokenmtr
```

## 1. Install the app

1. Open the `.dmg`.
2. Drag `TokenMtr.app` onto `Applications`.
3. Eject the disk image.

**Done when:** `TokenMtr.app` is in `/Applications`.

## 2. First launch: Open Anyway (D51)

macOS blocks an app that is not notarized. Since macOS 15, the only way through is System Settings.

1. Open `TokenMtr.app`. macOS says it cannot verify the app: click **Done**.
2. Open System Settings → Privacy & Security, scroll to **Security**.
3. Click **Open Anyway** next to "TokenMtr was blocked", then authenticate.
4. Open `TokenMtr.app` again and click **Open**.

macOS asks once. Updates keep the answer because every release is signed with the same
certificate (see [Signing](#signing-maintainers)).

**Done when:** the menu-bar icon appears.

## 3. First run

The full window opens an onboarding, in order:

1. Detected agents and profiles.
2. Claude transcript retention (D53): Claude Code deletes transcripts older than `cleanupPeriodDays`
   (default 30). Click **Keep transcripts for 10 years** to set it to 3650, the only change
   TokenMtr ever makes to another tool's files.
3. Install the background daemon (see [step 4](#4-daemon-without-the-app)).
4. Allow notifications.

TokenMtr then imports the history on disk, which can take a moment, and fetches EUR rates on the
first run.

**Done when:** the menu-bar recap shows your usage and `tokenmtr service status` reports a running daemon.

## 4. Daemon without the app

The daemon is a launchd LaunchAgent (D23) that runs `tokenmtr run` at login and restarts after a crash.
The app installs it during onboarding. A headless install:

```sh
tokenmtr service install
tokenmtr service status
```

Run `service install` again after you move the app: the plist stores the binary's path. The app
repairs it on every launch.

**Done when:** `service status` prints a running pid, and `~/Library/LaunchAgents/dev.tokenmtr.daemon.plist` exists.

## 5. Folder access (D50)

macOS asks per app before the daemon reads or writes Documents, Desktop, Downloads, iCloud Drive
or cloud storage (`~/Library/CloudStorage`, for example Dropbox). The defaults sit outside them:
exports go to `~/Library/Application Support/dev.tokenmtr.app/exports/`.

Grant access only when a sink path or a project's working directory is in one of those folders:

1. Open System Settings → Privacy & Security → **Full Disk Access** (or **Files and Folders**).
2. Click **+** and add `TokenMtr.app/Contents/MacOS/tokenmtr`.
3. Restart the daemon: **Restart daemon** in Settings, or `tokenmtr service install`.

Without access nothing blocks: a refused project falls back to its working directory as its key and
records one `protected_folder` issue; retry from **Retry** on `/projects` or by restarting the
daemon. A `[[projects]]` rule in [`config.md`](config.md) avoids the prompt without any grant.

`tokenmtr doctor` lists the sinks and working directories under protected folders.

**Done when:** `tokenmtr doctor` reports no `protected_folder` issue.

## 6. Keychain: Always Allow (D51)

The quota poll reads Claude Code's token from the login keychain (item `Claude Code-credentials`,
read-only). On the first poll macOS asks whether `tokenmtr` may use it.

1. Enter your login password.
2. Click **Always Allow**, not **Allow**.

The grant is tied to the code signature, which stays the same across updates, so the prompt does
not return.

**Done when:** after an update and relaunch, quota still refreshes with no prompt.

## 7. Uninstall

1. Optional: erase TokenMtr's data with the CLI before deleting it:

   ```sh
   tokenmtr erase --service --yes            # store, backups, logs, checkpoints, the plist
   tokenmtr erase --config --service --yes   # the same, plus ~/.config/tokenmtr/config.toml
   ```

   `erase` stops the daemon first. It never deletes sink outputs or agent files; without
   `--service`, the restarted daemon rebuilds the store from the agent files still on disk.
2. Without `erase`, run `tokenmtr service uninstall`.
3. Delete `TokenMtr.app`.
4. Remove the login item in System Settings → General → Login Items & Extensions.
5. Optional: delete `~/Library/Caches/dev.tokenmtr.app`.

**Done when:** `launchctl print gui/$(id -u)/dev.tokenmtr.daemon` fails, `dev.tokenmtr.daemon.plist`
is gone and no `tokenmtr` process runs.

## Troubleshooting

Start with `tokenmtr doctor`: agent roots per profile, file counts, last checkpoints, issue counts,
format drift, unresolved working directories, paths under protected folders, and Claude's
`cleanupPeriodDays`.

| Symptom | Check |
|---|---|
| App shows "daemon offline" | `tokenmtr service status`; read `~/Library/Logs/tokenmtr/launchd.err.log`; run `tokenmtr service install`. |
| "Update required" (D52) | Click **Restart daemon from this app**: it repairs the plist to the bundled binary. |
| Keychain prompt returns after an update | `codesign -dv --verbose=2 /Applications/TokenMtr.app` must show `Authority=TokenMtr Local Signing`. Another authority, or none, means the app was rebuilt or re-signed: reinstall it from the release `.dmg`. Then **Always Allow** once more. |
| Quota says `signed_out` or `token_expired` | Run `claude` and sign in again; TokenMtr never refreshes the token. |
| Usage older than 30 days is missing | `doctor` shows `cleanupPeriodDays`: Claude already deleted those transcripts. Set it as in step 3. |
| `protected_folder` issue | [Step 5](#5-folder-access-d50). |

## Signing (maintainers)

Release builds sign the app and the bundled CLI with the self-signed certificate **TokenMtr Local
Signing** (D51) until notarization replaces it. The name is the constant `IDENTITY` in
[`scripts/signing.nu`](../scripts/signing.nu); `just release` passes it as `APPLE_SIGNING_IDENTITY`.

1. Run `just signing-cert` once per Mac: it creates the certificate in the login keychain and asks
   you to trust it for code signing. It does nothing if the certificate exists;
   `just signing-cert --dry-run` prints the commands ([`scripts/signing-cert.nu`](../scripts/signing-cert.nu)).
2. Keep the certificate: a new one changes the signature, and every user sees the Keychain and
   folder prompts again.

**Done when:** `security find-identity -p codesigning` lists `TokenMtr Local Signing`.
