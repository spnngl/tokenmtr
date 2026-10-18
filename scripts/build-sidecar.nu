# Builds the `tokenmtr` CLI and stages it as the Tauri sidecar (D23).
#
# The app bundles it as `externalBin` "binaries/tokenmtr"; Tauri wants the file named with the
# target triple and strips the suffix inside the bundle (`TokenMtr.app/Contents/MacOS/tokenmtr`),
# where the daemon's launchd plist points. Runs from `beforeBuildCommand` in
# `apps/desktop/src-tauri/tauri.release.conf.json`.

def main [] {
    let triple = (
        ^rustc -vV
        | lines
        | where {|line| $line | str starts-with "host: " }
        | first
        | str replace "host: " ""
    )
    let target_dir = (^cargo metadata --no-deps --format-version 1 | from json | get target_directory)
    ^cargo build --release -p tokenmtr
    let staged = ($env.FILE_PWD | path expand | path dirname | path join "apps" "desktop" "src-tauri" "binaries")
    mkdir $staged
    let destination = ($staged | path join $"tokenmtr-($triple)")
    cp ($target_dir | path join "release" "tokenmtr") $destination
    print $"sidecar: ($destination)"
}
