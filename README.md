# squadreplay

[![Crates.io](https://img.shields.io/crates/v/squadreplay)](https://crates.io/crates/squadreplay)
[![docs.rs](https://img.shields.io/docsrs/squadreplay)](https://docs.rs/squadreplay)
[![License: MPL-2.0](https://img.shields.io/crates/l/squadreplay)](LICENSE)

`squadreplay` is a library-first Rust parser and CLI for Squad UE5 replay data.

The library exposes typed bundle APIs for:

- parsing `.replay` files into a `Bundle`
- reading and writing `.sqrj.json` bundles
- reading and writing `.sqrb` bundles
- deriving compatibility JSON from a parsed bundle

## Library

Add as a dependency (without the CLI):

```toml
[dependencies]
squadreplay = { version = "0.1.0-alpha.1", default-features = false }
```

```rust,no_run
use squadreplay::{parse_file, ParseOptions};

fn main() -> Result<(), squadreplay::Error> {
    let bundle = parse_file("match.replay", &ParseOptions::default())?;
    println!("players: {}", bundle.players.len());
    Ok(())
}
```

## CLI

Install the binary:

```bash
cargo install squadreplay
```

```bash
squadreplay parse match.replay --format sqrj,sqrb --output out/match
squadreplay inspect match.replay
squadreplay show out/match.sqrb
squadreplay unpack out/match.sqrb --output out/unpacked
```

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `cli`   | yes     | Builds the `squadreplay` binary (pulls in `clap`) |

Library-only consumers should disable default features to avoid the `clap` dependency.

## Minimum Supported Rust Version

Rust **1.85** (edition 2024).
