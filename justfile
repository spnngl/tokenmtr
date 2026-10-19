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

# Regenerate apps/desktop/src/shared/bindings.ts (command, event and error names) from the Rust shell.
bindings:
    UPDATE_BINDINGS=1 cargo test -p tokenmtr-desktop --lib bindings

# Regenerate the UI's golden fixtures (apps/desktop/src/shared/schemas/__fixtures__) from the Rust response types.
goldens:
    UPDATE_GOLDENS=1 cargo test -p tokenmtr-desktop --lib goldens

# .app and .dmg (docs/release.md): gates, then sidecar and `tauri build`, signed with the local identity when it exists.
# Extra flags pass through, e.g. `just release --target universal-apple-darwin` or `--unsigned`.
release *FLAGS: check
    nu scripts/release.nu {{FLAGS}}

# Fuzz one target for five minutes (nightly toolchain): just fuzz claude_line
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{target}} -- -max_total_time=300 "$PWD/seeds/{{target}}"

# Replay fuzz seeds plus truncated and bit-flipped variants on stable.
fuzz-smoke:
    cd fuzz && cargo test --lib

# Rebuild fuzz/seeds/ from the fixtures.
fuzz-seeds:
    nu scripts/fuzz-seeds.nu

# Create the stable local code-signing certificate once; --dry-run prints the commands.
signing-cert *args:
    nu scripts/signing-cert.nu {{args}}
