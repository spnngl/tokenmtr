set shell := ["bash", "-euo", "pipefail", "-c"]

ui := "apps/desktop"

# List recipes.
default:
    @just --list

# Every gate command: Rust format, lint, tests, then TypeScript typecheck, lint, tests.
check: check-rust check-ui

check-rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

check-ui:
    pnpm -C {{ui}} run typecheck
    pnpm -C {{ui}} run lint
    pnpm -C {{ui}} run test

test:
    cargo test --workspace
    pnpm -C {{ui}} run test

# Parser and store benchmarks (divan).
bench:
    cargo bench --workspace

# Run the desktop app with hot reload.
dev:
    pnpm -C {{ui}} tauri dev

# Fuzz one target for five minutes (nightly toolchain, D54): just fuzz claude_line
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{target}} -- -max_total_time=300 "$PWD/seeds/{{target}}"

# Replay fuzz seeds plus truncated and bit-flipped variants on stable (D54).
fuzz-smoke:
    cd fuzz && cargo test --lib

# Rebuild fuzz/seeds/ from the fixtures.
fuzz-seeds:
    nu scripts/fuzz-seeds.nu

# Create the stable local code-signing certificate once (D51); --dry-run prints the commands.
signing-cert *args:
    nu scripts/signing-cert.nu {{args}}
