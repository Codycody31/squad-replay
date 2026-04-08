mod common;

use common::{sample_bundle, unique_path};
use squadreplay::{Error, ParseOptions, compat, parse_bytes, parse_file, read_bundle, sqrb, sqrj};
use std::fs;
use std::path::PathBuf;

fn fixture_dir() -> Option<PathBuf> {
    std::env::var_os("SQUADREPLAY_TEST_FIXTURE_DIR").map(PathBuf::from)
}

#[test]
fn read_bundle_auto_detects_sqrj_and_sqrb() {
    let bundle = sample_bundle();
    let sqrj_path = unique_path("squadreplay-library", ".sqrj.json");
    let sqrb_path = unique_path("squadreplay-library", ".sqrb");

    sqrj::write(&bundle, &sqrj_path).expect("sqrj write should succeed");
    sqrb::write(&bundle, &sqrb_path).expect("sqrb write should succeed");

    let sqrj_roundtrip = read_bundle(&sqrj_path).expect("sqrj read should succeed");
    let sqrb_roundtrip = read_bundle(&sqrb_path).expect("sqrb read should succeed");

    fs::remove_file(&sqrj_path).expect("temporary sqrj should be removable");
    fs::remove_file(&sqrb_path).expect("temporary sqrb should be removable");

    assert_eq!(
        serde_json::to_value(&sqrj_roundtrip).expect("bundle should serialize"),
        serde_json::to_value(&bundle).expect("bundle should serialize")
    );
    assert_eq!(
        serde_json::to_value(&sqrb_roundtrip).expect("bundle should serialize"),
        serde_json::to_value(&bundle).expect("bundle should serialize")
    );
}

#[test]
fn compat_projection_uses_bundle_content() {
    let bundle = sample_bundle();
    let compat_bundle = compat::from_bundle(&bundle);

    assert_eq!(compat_bundle.map_name, "Jensens_Range");
    assert_eq!(compat_bundle.squad_version, "//Squad/v10.3.1");
    assert_eq!(compat_bundle.helicopter_positions_per_second.len(), 11);
}

#[test]
fn parse_bytes_rejects_invalid_replay_data() {
    let error = parse_bytes(
        b"not-a-replay",
        Some("invalid.replay".to_string()),
        &ParseOptions::default(),
    )
    .expect_err("invalid bytes should fail");

    assert!(matches!(error, Error::InvalidReplay(_)));
}

#[test]
fn parse_file_rejects_invalid_replay_data() {
    let path = unique_path("squadreplay-invalid", ".replay");
    fs::write(&path, b"not-a-replay").expect("temporary invalid replay should be written");

    let error =
        parse_file(&path, &ParseOptions::default()).expect_err("invalid file should fail to parse");
    fs::remove_file(&path).expect("temporary invalid replay should be removable");

    assert!(matches!(error, Error::InvalidReplay(_)));
}

#[test]
fn no_properties_keeps_derived_outputs_when_fixture_is_available() {
    let Some(fixture_dir) = fixture_dir() else {
        return;
    };
    let fixture = fixture_dir.join("rtb-jensens-range-wpmc-vs-turkey-20260407.replay");
    if !fixture.exists() {
        return;
    }

    let bundle = parse_file(
        &fixture,
        &ParseOptions {
            include_property_events: false,
        },
    )
    .expect("fixture replay should parse without raw property retention");

    assert!(bundle.events.properties.is_empty());
    assert_eq!(bundle.teams.len(), 2);
    assert_eq!(bundle.squads.len(), 1);
    assert_eq!(bundle.players.len(), 2);
    assert!(!bundle.events.seat_changes.is_empty());
    assert!(!bundle.tracks.helicopters.is_empty());
}

#[test]
#[ignore = "fixture corpus smoke test; run explicitly when validating parser behavior"]
fn all_fixture_replays_parse_when_fixture_dir_is_set() {
    let Some(fixture_dir) = fixture_dir() else {
        return;
    };

    for entry in fs::read_dir(&fixture_dir).expect("fixture dir should be readable") {
        let entry = entry.expect("fixture dir entry should be readable");
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("replay") {
            continue;
        }

        parse_file(&path, &ParseOptions::default())
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", path.display()));
    }
}
