//! Parse Squad UE5 replay data into typed bundles.
//!
//! **squadreplay** is a library-first parser for
//! [Squad](https://joinsquad.com/) UE5 `.replay` files. It extracts teams,
//! squads, players, kills, vehicle tracks, deployments, and more into a
//! single [`Bundle`] struct that can be serialized to JSON (`.sqrj.json`) or
//! binary MessagePack (`.sqrb`) for downstream tooling.
//!
//! # Quick start
//!
//! ```no_run
//! use squadreplay::{parse_file, ParseOptions};
//!
//! let bundle = parse_file("match.replay", &ParseOptions::default())?;
//! println!("map: {:?}", bundle.replay.map_name);
//! println!("players: {}", bundle.players.len());
//! println!("kills: {}", bundle.events.kills.len());
//! # Ok::<(), squadreplay::Error>(())
//! ```
//!
//! # Serialization round-trip
//!
//! ```no_run
//! use squadreplay::{parse_file, sqrb, sqrj, ParseOptions};
//!
//! let bundle = parse_file("match.replay", &ParseOptions::default())?;
//!
//! // Write to both formats
//! sqrj::write(&bundle, "match.sqrj.json")?;
//! sqrb::write(&bundle, "match.sqrb")?;
//!
//! // Read back
//! let from_json = sqrj::read("match.sqrj.json")?;
//! let from_bin  = sqrb::read("match.sqrb")?;
//! # Ok::<(), squadreplay::Error>(())
//! ```
//!
//! # Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `cli`   | yes     | Builds the `squadreplay` binary (pulls in [`clap`](https://docs.rs/clap)) |
//!
//! Library-only consumers should disable default features:
//!
//! ```toml
//! [dependencies]
//! squadreplay = { version = "0.1.0-alpha.1", default-features = false }
//! ```

#![warn(missing_docs)]

use std::path::Path;

/// Data model for parsed Squad replay bundles.
///
/// All top-level types ([`Bundle`], [`Team`](bundle::Team),
/// [`Player`](bundle::Player), etc.) live in this module.
#[path = "model.rs"]
pub mod bundle;
mod classify;
/// Compatibility layer producing the legacy JSON format used by older Squad
/// replay tools.
pub mod compat;
mod error;
mod formats;
mod parser;
/// Read and write `.sqrb` (binary MessagePack) bundles.
pub mod sqrb;
/// Read and write `.sqrj.json` (human-readable JSON) bundles.
pub mod sqrj;
mod unreal_names;

pub use bundle::{Bundle, GameStateInfo, ParseOptions};
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
