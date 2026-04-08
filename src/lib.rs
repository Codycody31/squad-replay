//! Parse Squad UE5 replay data into typed bundles.
//!
//! The library exposes replay parsing plus helpers for reading and writing
//! `.sqrj.json` and `.sqrb` bundles. CLI behavior lives in the binary target.
//!
//! ```no_run
//! use squadreplay::{parse_file, ParseOptions};
//!
//! let bundle = parse_file("match.replay", &ParseOptions::default())?;
//! println!("players: {}", bundle.players.len());
//! # Ok::<(), squadreplay::Error>(())
//! ```

use std::path::Path;

#[path = "model.rs"]
pub mod bundle;
mod classify;
pub mod compat;
mod error;
mod formats;
mod parser;
pub mod sqrb;
pub mod sqrj;
mod unreal_names;

pub use bundle::{Bundle, ParseOptions};
pub use error::{Error, Result};

/// Parse a replay file from disk.
pub fn parse_file(path: impl AsRef<Path>, options: &ParseOptions) -> Result<Bundle> {
    parser::parse_file(path, options.include_property_events)
}

/// Parse replay bytes that are already loaded in memory.
pub fn parse_bytes(
    bytes: impl AsRef<[u8]>,
    file_name: Option<String>,
    options: &ParseOptions,
) -> Result<Bundle> {
    parser::parse_bytes(bytes.as_ref(), file_name, options.include_property_events)
}

/// Read a serialized bundle from disk.
///
/// The format is inferred from the file extension.
pub fn read_bundle(path: impl AsRef<Path>) -> Result<Bundle> {
    let path = path.as_ref();
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if name.ends_with(".sqrb") {
        return sqrb::read(path);
    }
    if name.ends_with(".sqrj") || name.ends_with(".sqrj.json") || name.ends_with(".json") {
        return sqrj::read(path);
    }

    Err(Error::Unsupported(format!(
        "cannot infer bundle format from `{}`; expected .sqrb or .sqrj.json",
        path.display()
    )))
}
