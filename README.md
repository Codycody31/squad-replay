# squadreplay

`squadreplay` is a library-first Rust parser and CLI for Squad UE5 replay data.

The library exposes typed bundle APIs for:

- parsing `.replay` files into a `Bundle`
- reading and writing `.sqrj.json` bundles
- reading and writing `.sqrb` bundles
- deriving compatibility JSON from a parsed bundle

## Library

```rust,no_run
use squadreplay::{parse_file, ParseOptions};

fn main() -> Result<(), squadreplay::Error> {
    let bundle = parse_file("match.replay", &ParseOptions::default())?;
    println!("players: {}", bundle.players.len());
    Ok(())
}
```

## CLI

```bash
squadreplay parse match.replay --format sqrj,sqrb --output out/match
squadreplay inspect match.replay
squadreplay show out/match.sqrb
squadreplay unpack out/match.sqrb --output out/unpacked
```
