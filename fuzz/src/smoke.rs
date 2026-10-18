//! Stable-toolchain stand-in for a short fuzz run (D54): replays every seed in `seeds/<target>/`
//! plus truncated and bit-flipped variants through its target. Any panic fails the test and the
//! offending input is written under `target/fuzz-smoke-crash/` (turn it into a fixture and a
//! regression test).

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use crate::{Entry, TARGETS};

/// Single-bit flips tried per seed.
const FLIPS: usize = 512;
/// Truncation points tried per seed.
const CUTS: usize = 200;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = root().join("seeds").join(target);
    let mut seeds: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            (name, fs::read(&path).expect("read seed"))
        })
        .collect();
    seeds.sort();
    seeds
}

/// Deterministic xorshift64: reproducible variants without a `rand` dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % bound as u64).expect("below bound")
    }
}

/// Inputs that are not derived from a seed: empty, binary and pathological nesting.
fn edge_cases() -> Vec<(String, Vec<u8>)> {
    vec![
        ("empty".into(), Vec::new()),
        ("0xff".into(), vec![0xFF; 64]),
        ("nul".into(), vec![0; 64]),
        ("newlines".into(), vec![b'\n'; 64]),
        ("deep-array".into(), vec![b'['; 100_000]),
        ("deep-object".into(), b"{\"a\":".repeat(100_000)),
        (
            "deep-toml-array".into(),
            [b"a=".as_slice(), &[b'['; 100_000]].concat(),
        ),
        ("deep-xml".into(), b"<a>".repeat(100_000)),
        (
            "long-string".into(),
            [b"\"".as_slice(), &[b'x'; 1_000_000]].concat(),
        ),
    ]
}

fn variants(name: &str, seed: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![(name.to_owned(), seed.to_vec())];
    let step = (seed.len() / CUTS).max(1);
    out.extend(
        (0..seed.len())
            .step_by(step)
            .map(|n| (format!("{name} cut at {n}"), seed[..n].to_vec())),
    );
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    if !seed.is_empty() {
        for _ in 0..FLIPS {
            let (byte, bit) = (rng.below(seed.len()), rng.below(8));
            let mut flipped = seed.to_vec();
            flipped[byte] ^= 1 << bit;
            out.push((format!("{name} flip byte {byte} bit {bit}"), flipped));
        }
    }
    out
}

fn run(target: &str, entry: Entry, label: &str, input: &[u8]) {
    if catch_unwind(AssertUnwindSafe(|| entry(input))).is_ok() {
        return;
    }
    let dir: PathBuf = root().join("target/fuzz-smoke-crash");
    let saved = fs::create_dir_all(&dir)
        .and_then(|()| {
            let file = dir.join(format!(
                "{target}-{:016x}",
                input
                    .iter()
                    .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b))
                        .wrapping_mul(0x100_0000_01b3))
            ));
            fs::write(&file, input).map(|()| file)
        })
        .map_or_else(
            |e| format!("not saved: {e}"),
            |f| format!("saved to {}", f.display()),
        );
    panic!(
        "{target} panicked on `{label}` ({} bytes), {saved}",
        input.len()
    );
}

#[test]
fn every_target_survives_seeds_and_mutations() {
    for (target, entry) in TARGETS {
        let seeds = seeds(target);
        assert!(
            !seeds.is_empty(),
            "{target}: no seeds, run scripts/fuzz-seeds.nu"
        );
        for (name, seed) in &seeds {
            for (label, input) in variants(name, seed) {
                run(target, *entry, &label, &input);
            }
        }
        for (label, input) in edge_cases() {
            run(target, *entry, &label, &input);
        }
    }
}

#[test]
fn targets_seeds_and_shims_agree() {
    let mut listed: Vec<_> = TARGETS.iter().map(|(name, _)| (*name).to_owned()).collect();
    listed.sort();
    let names = |dir: &str, strip: fn(&str) -> &str| {
        let mut names: Vec<_> = fs::read_dir(root().join(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|entry| strip(&entry.expect("dir entry").file_name().to_string_lossy()).to_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        listed,
        names("fuzz_targets", |f| f.trim_end_matches(".rs")),
        "fuzz_targets/"
    );
    assert_eq!(listed, names("seeds", |f| f), "seeds/");
}
