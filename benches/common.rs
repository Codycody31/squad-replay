//! Shared utilities for Criterion benchmarks.
//!
//! Mirrors the fixture-loading pattern from `tests/common/mod.rs` so that
//! benchmarks use the same replay corpus and honour the same env-var overrides.

#![allow(dead_code, unused_macros, unused_imports)]

use squadreplay::bundle::Bundle;
use squadreplay::ParseOptions;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Fixture constants
// ---------------------------------------------------------------------------

pub const FALLUJAH: &str = "rtb-fallujah-seeding-20260406.replay";
pub const JENSENS: &str = "rtb-jensens-range-wpmc-vs-turkey-20260407.replay";

// ---------------------------------------------------------------------------
// Fixture resolution (same logic as tests/common/mod.rs)
// ---------------------------------------------------------------------------

/// Absolute path to `<workspace>/tests/fixtures`.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Directory that benchmarks should read replay files from.
///
/// Honours `SQUADREPLAY_TEST_FIXTURE_DIR` so external corpora can be plugged
/// in, but defaults to the in-repo fixtures directory when it exists.
pub fn fixture_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("SQUADREPLAY_TEST_FIXTURE_DIR") {
        return Some(PathBuf::from(value));
    }
    let root = fixtures_root();
    if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

/// Full path to a specific replay fixture, or `None` if unavailable.
pub fn fixture_path(replay_file: &str) -> Option<PathBuf> {
    let dir = fixture_dir()?;
    let path = dir.join(replay_file);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Skip macro
// ---------------------------------------------------------------------------

/// Return early from a benchmark function when fixtures are not present.
///
/// Usage:
/// ```ignore
/// fn my_bench(c: &mut Criterion) {
///     skip_if_missing!();
///     // ... benchmark code ...
/// }
/// ```
macro_rules! skip_if_missing {
    () => {
        if $crate::common::fixture_dir().is_none() {
            eprintln!(
                "benchmark skipped: fixture directory not found (set SQUADREPLAY_TEST_FIXTURE_DIR)"
            );
            return;
        }
    };
    ($path:expr) => {
        if !$path.exists() {
            eprintln!("benchmark skipped: {} not found", $path.display());
            return;
        }
    };
}

pub(crate) use skip_if_missing;

// ---------------------------------------------------------------------------
// Fixture caching
// ---------------------------------------------------------------------------

/// Load raw bytes for a replay fixture. Panics if the file is missing or
/// the fixture name is not one of the known constants.
pub fn load_fixture_bytes(replay_file: &str) -> &'static [u8] {
    static FALLUJAH_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    static JENSENS_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

    let lock = match replay_file {
        FALLUJAH => &FALLUJAH_BYTES,
        JENSENS => &JENSENS_BYTES,
        _ => panic!("unsupported fixture for caching: {replay_file}"),
    };

    lock.get_or_init(|| {
        let path = fixture_path(replay_file)
            .unwrap_or_else(|| panic!("fixture not found: {replay_file}"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
    })
}

/// Parse a fixture into a [`Bundle`], caching the result for the process
/// lifetime so Criterion's measurement loop only times the target function.
pub fn parse_fixture_bundle(replay_file: &str, with_props: bool) -> &'static Bundle {
    // We cache four variants: {fallujah,jensens} x {with,without} props.
    static FALLUJAH_PROPS: OnceLock<Bundle> = OnceLock::new();
    static FALLUJAH_NO_PROPS: OnceLock<Bundle> = OnceLock::new();
    static JENSENS_PROPS: OnceLock<Bundle> = OnceLock::new();
    static JENSENS_NO_PROPS: OnceLock<Bundle> = OnceLock::new();

    let lock = match (replay_file, with_props) {
        (FALLUJAH, true) => &FALLUJAH_PROPS,
        (FALLUJAH, false) => &FALLUJAH_NO_PROPS,
        (JENSENS, true) => &JENSENS_PROPS,
        (JENSENS, false) => &JENSENS_NO_PROPS,
        _ => panic!("unsupported fixture for caching: {replay_file}"),
    };

    lock.get_or_init(|| {
        let bytes = load_fixture_bytes(replay_file);
        let opts = ParseOptions {
            include_property_events: with_props,
        };
        squadreplay::parse_bytes(bytes, Some(replay_file.to_string()), &opts)
            .unwrap_or_else(|e| panic!("failed to parse {replay_file}: {e}"))
    })
}

// ---------------------------------------------------------------------------
// Temp file helpers
// ---------------------------------------------------------------------------

/// RAII guard that removes a temporary file on drop.
pub struct TempFile(pub PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Generate a unique temporary file path. Uses an atomic counter in addition
/// to the timestamp to avoid collisions inside tight Criterion loops where
/// clock resolution may be coarser than iteration frequency.
pub fn unique_path(prefix: &str, suffix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{ts}-{seq}{suffix}"))
}

/// Fixture metadata for parameterised benchmarks.
pub struct FixtureInfo {
    pub label: &'static str,
    pub file: &'static str,
    pub size: u64,
}

/// Build the list of available fixtures with their on-disk sizes.
pub fn available_fixtures() -> Vec<FixtureInfo> {
    let mut out = Vec::new();
    for (label, file) in [("jensens_11mb", JENSENS), ("fallujah_34mb", FALLUJAH)] {
        if let Some(path) = fixture_path(file) {
            let size = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(0);
            out.push(FixtureInfo {
                label,
                file,
                size,
            });
        }
    }
    out
}
