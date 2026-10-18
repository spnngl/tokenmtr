# Builds the `.app` and `.dmg`. Run through `just release`, which runs the gates first.
#
# Signs with the stable local identity from `signing.nu`; create it once with `just signing-cert`.
# `--unsigned` skips signing for a local test build (the linker's ad-hoc signature only).
# `--target universal-apple-darwin` builds one bundle for Apple silicon and Intel.
# `tauri build` runs `build-sidecar.nu` (see `tauri.release.conf.json`) before compiling the app.

use signing.nu IDENTITY

def main [
    --target: string # Rust target triple, or `universal-apple-darwin`; defaults to the host.
    --unsigned # Skip code signing (local test builds only).
] {
    let root = ($env.FILE_PWD | path expand | path dirname)
    let signing = if $unsigned {
        {}
    } else {
        let found = (^security find-identity -v -p codesigning | lines | where {|line| $line | str contains $IDENTITY })
        if ($found | is-empty) {
            error make { msg: $"no code-signing identity named '($IDENTITY)'; run `just signing-cert` first" }
        }
        { APPLE_SIGNING_IDENTITY: $IDENTITY }
    }
    let target_args = if $target == null { [] } else { [--target $target] }
    with-env $signing {
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
