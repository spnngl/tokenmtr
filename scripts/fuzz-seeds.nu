#!/usr/bin/env nu
# Rebuilds fuzz/seeds/<target>/ from fixtures/ and the crates' test fixtures.
# Run it after a fixture changes, then commit the result.

# Most lines taken from one transcript file; keeps the corpus small.
const MAX_LINES = 8

def write-seeds [root: string, target: string, seeds: list<record<name: string, body: string>>] {
    let dir = ($root | path join fuzz seeds $target)
    rm --recursive --force $dir
    mkdir $dir
    for seed in $seeds {
        $seed.body | save --force ($dir | path join $seed.name)
    }
}

# One seed per file.
def whole-files [paths: list<string>] {
    $paths | each {|p| {name: ($p | path basename), body: (open --raw $p)} }
}

# One seed per line, each prefixed with `prefix_first` lines' worth of header when `header` is set.
def line-seeds [paths: list<string>, header: bool] {
    $paths | each {|p|
        let lines = (open --raw $p | lines | where {|l| ($l | str trim) != "" })
        let stem = ($p | path parse | get stem)
        let head = if $header { ($lines | first) + "\n" } else { "" }
        let rest = if $header { $lines | skip 1 } else { $lines }
        $rest | first $MAX_LINES | enumerate | each {|e|
            {name: $"($stem)-($e.index)", body: $"($head)($e.item)"}
        }
    } | flatten
}

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    let fixtures = ($root | path join fixtures)

    let claude = (glob ($fixtures | path join claude projects **/*.jsonl))
    let pi = (glob ($fixtures | path join pi sessions **/*.jsonl))
    let http = {|names: list<string>| $names | each {|n| $fixtures | path join http $n } }

    write-seeds $root claude_line (line-seeds $claude false)
    write-seeds $root pi_line (line-seeds $pi true)
    write-seeds $root stats_cache (whole-files [($fixtures | path join claude stats-cache.json)])
    write-seeds $root litellm_catalog (whole-files (do $http [litellm_small.json]))
    write-seeds $root quota_response (whole-files (do $http [claude_usage.json claude_usage_sparse.json claude_usage_limits.json claude_profile.json claude_credentials.json]))
    write-seeds $root ecb_xml (whole-files (do $http [ecb_daily.xml ecb_hist.xml]))
    write-seeds $root config (whole-files [($root | path join crates tokenmtr-config tests fixtures example.toml)])

    ls ($root | path join fuzz seeds) | each {|d| {target: ($d.name | path basename), seeds: (ls $d.name | length)} }
}
