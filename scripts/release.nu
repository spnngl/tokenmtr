# Builds the signed `.app` and `.dmg` (D51). Run through `just release`, which runs the gates first.
#
# Signs with the stable local identity from `signing.nu`; create it once with `just signing-cert`.
# `tauri build` runs `build-sidecar.nu` (see `tauri.release.conf.json`) before compiling the app.

use signing.nu IDENTITY

def main [] {
    let root = ($env.FILE_PWD | path expand | path dirname)
    let found = (^security find-identity -v -p codesigning | lines | where {|line| $line | str contains $IDENTITY })
    if ($found | is-empty) {
        error make { msg: $"no code-signing identity named '($IDENTITY)'; run `just signing-cert` first" }
    }
    with-env { APPLE_SIGNING_IDENTITY: $IDENTITY } {
        cd ($root | path join "apps" "desktop")
        ^pnpm tauri build --config src-tauri/tauri.release.conf.json
    }
    let target_dir = (^cargo metadata --no-deps --format-version 1 | from json | get target_directory)
    glob ($target_dir | path join "release" "bundle" "dmg" "*.dmg") | each {|dmg| print $"dmg: ($dmg)" } | ignore
}
