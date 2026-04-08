//! Golden-snapshot tests that run the parser against the replay corpus
//! committed under `tests/fixtures/`.
//!
//! Each fixture is parsed once, summarized into a `FixtureSnapshot` (counts,
//! diagnostic counters, and a handful of stable identity fields), and diffed
//! against a committed JSON file. The intent is to make parser regressions —
//! dropped players, missing kills, zeroed diagnostic counters — impossible to
//! merge silently while keeping golden files small enough to review by hand.
//!
//! To bless intentional parser changes:
//!
//! ```bash
//! UPDATE_SNAPSHOTS=1 cargo test --test fixture_snapshots
//! ```
//!
//! Fixtures are skipped entirely when the repo corpus is unavailable (for
//! example, when running from a packaged crate that excludes the binary
//! replays), so CI matrix jobs without the corpus still compile and pass.

mod common;

use common::{
    FIXTURES, FixtureSnapshot, assert_snapshot_matches, fixtures_root,
    update_snapshots_requested,
};
use squadreplay::{ParseOptions, parse_file};

fn fixtures_available() -> bool {
    fixtures_root().is_dir()
        && FIXTURES
            .iter()
            .all(|fixture| fixture.replay_path().exists())
}

#[test]
fn fixture_corpus_parses_and_matches_snapshots() {
    if !fixtures_available() {
        eprintln!(
            "skipping fixture_corpus_parses_and_matches_snapshots: \
             tests/fixtures/ corpus not present"
        );
        return;
    }

    for fixture in FIXTURES {
        let replay_path = fixture.replay_path();
        let bundle = parse_file(&replay_path, &ParseOptions::default())
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", fixture.name));

        assert!(
            !bundle.replay.source.sha256.is_empty(),
            "{}: replay.source.sha256 must be populated after parse",
            fixture.name
        );
        assert!(
            bundle.replay.source.size_bytes > 0,
            "{}: replay.source.size_bytes must be > 0 after parse",
            fixture.name
        );
        assert!(
            bundle.replay.map_name.is_some(),
            "{}: map name should be decoded from the replay header",
            fixture.name
        );

        let snapshot = FixtureSnapshot::from_bundle(&bundle);
        assert_snapshot_matches(&fixture.snapshot_path(), &snapshot);
    }
}

#[test]
fn parse_options_suppress_properties_without_affecting_snapshot_counts() {
    // Turning off raw property retention should not delete derived tracks,
    // component states, or other grouped projections. This is the invariant
    // documented on `ParseOptions::include_property_events`, so lock it in.
    if !fixtures_available() || update_snapshots_requested() {
        return;
    }

    for fixture in FIXTURES {
        let replay_path = fixture.replay_path();
        let with_props = parse_file(&replay_path, &ParseOptions::default())
            .unwrap_or_else(|error| panic!("{}: parse with props failed: {error}", fixture.name));
        let without_props = parse_file(
            &replay_path,
            &ParseOptions {
                include_property_events: false,
            },
        )
        .unwrap_or_else(|error| panic!("{}: parse without props failed: {error}", fixture.name));

        assert!(
            without_props.events.properties.is_empty(),
            "{}: properties should be cleared when include_property_events=false",
            fixture.name
        );

        // Everything except the raw property stream should match between the
        // two parses. We diff the snapshot shape rather than the full bundle
        // to keep the failure message small.
        let mut with_snapshot = FixtureSnapshot::from_bundle(&with_props);
        let without_snapshot = FixtureSnapshot::from_bundle(&without_props);
        with_snapshot.events.properties = without_snapshot.events.properties;
        assert_eq!(
            with_snapshot, without_snapshot,
            "{}: disabling property events altered derived bundle fields",
            fixture.name
        );
    }
}
