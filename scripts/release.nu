# Builds the `.app` and `.dmg`. Run through `just release`, which runs the gates first.
#
# Signs with the stable local identity from `signing.nu` when the login keychain holds it; create
# it once with `just signing-cert`. A stable signature keeps the Keychain's "Always Allow" grant and
# the macOS folder grants across rebuilds. Without the identity the build is unsigned (the linker's
# ad-hoc signature only) and says so; `--unsigned` skips signing even when the identity exists.
# `--target universal-apple-darwin` builds one bundle for Apple silicon and Intel.
# `tauri build` runs `build-sidecar.nu` (see `tauri.release.conf.json`) before compiling the app.
# `CI=true` makes Tauri's `bundle_dmg.sh` skip its Finder AppleScript: the window it opens on the
# mounted image keeps the volume busy, so the detach fails ("Resource busy"), and headless sessions
# have no Finder at all. The dmg then opens with Finder's default layout.

use signing.nu [IDENTITY has-identity]

def main [
    --target: string # Rust target triple, or `universal-apple-darwin`; defaults to the host.
    --unsigned # Skip code signing (local test builds only).
] {
    let root = ($env.FILE_PWD | path expand | path dirname)
    let signing = if $unsigned {
        {}
    } else if (has-identity) {
        { APPLE_SIGNING_IDENTITY: $IDENTITY }
    } else {
        print -e $"warning: no code-signing identity named '($IDENTITY)': building unsigned, so macOS asks for the Keychain again after each rebuild. Run `just signing-cert` once to sign."
        {}
    }
    let target_args = if $target == null { [] } else { [--target $target] }
    with-env ($signing | merge { CI: "true" }) {
        cd ($root | path join "apps" "desktop")
        ^pnpm tauri build --config src-tauri/tauri.release.conf.json ...$target_args
    }
    let target_dir = (^cargo metadata --no-deps --format-version 1 | from json | get target_directory)
    let release_dir = if $target == null {
        $target_dir | path join "release"
    } else {
        $target_dir | path join $target "release"
    }
    glob ($release_dir | path join "bundle" "dmg" "*.dmg") | each {|dmg| print $"dmg: ($dmg)" } | ignore
}
