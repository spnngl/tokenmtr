# Builds the `tokenmtr` CLI and stages it as the Tauri sidecar (D23).
#
# The app bundles it as `externalBin` "binaries/tokenmtr"; Tauri wants the file named with the
# target triple and strips the suffix inside the bundle (`TokenMtr.app/Contents/MacOS/tokenmtr`),
# where the daemon's launchd plist points. Runs from `beforeBuildCommand` in
# `apps/desktop/src-tauri/tauri.release.conf.json`.
#
# The triple comes from `TAURI_ENV_TARGET_TRIPLE` (set by `tauri build --target <triple>`), else the
# host. `universal-apple-darwin` builds both Apple triples and merges them with `lipo`; it stages
# the per-triple files too, because Tauri compiles (and checks `externalBin` for) each slice.

const UNIVERSAL = "universal-apple-darwin"
const APPLE_TRIPLES = ["aarch64-apple-darwin", "x86_64-apple-darwin"]

def host-triple [] {
    ^rustc -vV
    | lines
    | where {|line| $line | str starts-with "host: " }
    | first
    | str replace "host: " ""
}

# Builds `tokenmtr` for one triple and returns the path of the binary.
def build-for [target_dir: string, triple: string] {
    ^cargo build --release -p tokenmtr --target $triple
    $target_dir | path join $triple "release" "tokenmtr"
}

def main [] {
    let triple = ($env.TAURI_ENV_TARGET_TRIPLE? | default (host-triple))
    let target_dir = (^cargo metadata --no-deps --format-version 1 | from json | get target_directory)
    let staged = ($env.FILE_PWD | path expand | path dirname | path join "apps" "desktop" "src-tauri" "binaries")
    mkdir $staged
    let destination = ($staged | path join $"tokenmtr-($triple)")
    if $triple == $UNIVERSAL {
        let slices = ($APPLE_TRIPLES | each {|apple|
            let slice = (build-for $target_dir $apple)
            cp $slice ($staged | path join $"tokenmtr-($apple)")
            $slice
        })
        ^lipo -create -output $destination ...$slices
    } else {
        cp (build-for $target_dir $triple) $destination
    }
    print $"sidecar: ($destination)"
}
