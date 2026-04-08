#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use squadreplay::bundle::{
    ActorEntity, ActorGroups, Bundle, ComponentEntity, ComponentStateEvent, DecodedPropertyValue,
    Diagnostics, EventGroups, PropertyEvent, ProvenanceEntry, ReplayInfoSection, ReplaySourceInfo,
    Track3, TrackGroups, TrackSample3,
};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn unique_path(prefix: &str, suffix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{unique}{suffix}"))
}

/// Absolute path to `<workspace>/tests/fixtures` where the committed replay
/// corpus and golden snapshots live.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Directory that tests should read replay files from. Honors
/// `SQUADREPLAY_TEST_FIXTURE_DIR` so external corpora can still be plugged in,
/// but defaults to the in-repo fixtures directory when it exists.
pub fn fixture_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("SQUADREPLAY_TEST_FIXTURE_DIR") {
        return Some(PathBuf::from(value));
    }
    let root = fixtures_root();
    if root.is_dir() { Some(root) } else { None }
}

/// A replay fixture that ships with the repo and has a committed snapshot.
pub struct FixtureSpec {
    /// Stable identifier used for the snapshot file name.
    pub name: &'static str,
    /// File name of the replay under `tests/fixtures/`.
    pub replay_file: &'static str,
}

impl FixtureSpec {
    pub fn replay_path(&self) -> PathBuf {
        fixtures_root().join(self.replay_file)
    }

    pub fn snapshot_path(&self) -> PathBuf {
        fixtures_root()
            .join("snapshots")
            .join(format!("{}.json", self.name))
    }
}

/// Ordered list of committed replay fixtures. Keep this list sorted by name so
/// that snapshot regeneration is deterministic across platforms.
pub const FIXTURES: &[FixtureSpec] = &[
    FixtureSpec {
        name: "rtb-fallujah-seeding-20260406",
        replay_file: "rtb-fallujah-seeding-20260406.replay",
    },
    FixtureSpec {
        name: "rtb-jensens-range-wpmc-vs-turkey-20260407",
        replay_file: "rtb-jensens-range-wpmc-vs-turkey-20260407.replay",
    },
];

// ---------------------------------------------------------------------------
// Snapshot shape
//
// The snapshot captures counts, diagnostic counters, and a handful of stable
// text fields from a parsed `Bundle`. The goal is to make meaningful parser
// regressions (a dropped player, a missing kill, a zeroed counter) loud and
// obvious in a diff, while keeping the committed file small enough for humans
// to review. Raw sample payloads are intentionally summarized, not copied.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FixtureSnapshot {
    pub source: SourceSnapshot,
    pub replay: ReplaySnapshot,
    pub counts: CountsSnapshot,
    pub actors: ActorsSnapshot,
    pub tracks: TracksSnapshot,
    pub events: EventsSnapshot,
    pub diagnostics: DiagnosticsSnapshot,
    pub compat: CompatSnapshot,
    pub players: Vec<PlayerSnapshot>,
    pub first_kill: Option<KillSnapshot>,
    pub last_kill: Option<KillSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceSnapshot {
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplaySnapshot {
    pub map_name: Option<String>,
    pub squad_version: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CountsSnapshot {
    pub teams: usize,
    pub squads: usize,
    pub players: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorsSnapshot {
    pub vehicles: usize,
    pub helicopters: usize,
    pub deployables: usize,
    pub components: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracksSnapshot {
    pub players: TrackKindSnapshot,
    pub vehicles: TrackKindSnapshot,
    pub helicopters: TrackKindSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackKindSnapshot {
    pub tracks: usize,
    pub total_samples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsSnapshot {
    pub kills: usize,
    pub deployments: usize,
    pub seat_changes: usize,
    pub component_states: usize,
    pub vehicle_states: usize,
    pub weapon_states: usize,
    pub properties: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagnosticsSnapshot {
    pub frames_processed: u64,
    pub packets_processed: u64,
    pub actor_opens: u64,
    pub export_groups_discovered: usize,
    pub guid_to_path_size: usize,
    pub property_replications: u64,
    pub position_samples: u64,
    pub vehicle_position_samples: u64,
    pub replay_data_chunks: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompatSnapshot {
    pub map_name: String,
    pub squad_version: String,
    pub match_duration_seconds: u32,
    pub kills: usize,
    pub kills_by_second_buckets: usize,
    pub player_stat_entries: usize,
    pub positions_seconds: usize,
    pub helicopter_positions_seconds: usize,
    pub vehicle_positions_seconds: usize,
    pub deployable_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerSnapshot {
    pub name: Option<String>,
    pub team_id: Option<u32>,
    pub squad_id: Option<u32>,
    pub has_steam_id: bool,
    pub has_eos_id: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KillSnapshot {
    pub t_ms: u64,
    pub victim_name: Option<String>,
    pub killer_name: Option<String>,
    pub was_incap: Option<bool>,
}

fn track_kind(tracks: &[Track3]) -> TrackKindSnapshot {
    TrackKindSnapshot {
        tracks: tracks.len(),
        total_samples: tracks.iter().map(|track| track.samples.len()).sum(),
    }
}

impl FixtureSnapshot {
    pub fn from_bundle(bundle: &Bundle) -> Self {
        let compat = squadreplay::compat::from_bundle(bundle);

        let mut players: Vec<PlayerSnapshot> = bundle
            .players
            .iter()
            .map(|player| PlayerSnapshot {
                name: player.name.clone(),
                team_id: player.team_id,
                squad_id: player.squad_id,
                has_steam_id: player.steam_id.is_some(),
                has_eos_id: player.eos_id.is_some(),
            })
            .collect();
        // Stable ordering: sort by name (None last), then team/squad for ties.
        players.sort_by(|a, b| {
            (a.name.is_none(), &a.name, a.team_id, a.squad_id).cmp(&(
                b.name.is_none(),
                &b.name,
                b.team_id,
                b.squad_id,
            ))
        });

        let to_kill_snapshot = |kill: &squadreplay::bundle::KillEvent| KillSnapshot {
            t_ms: kill.t_ms,
            victim_name: kill.victim_name.clone(),
            killer_name: kill.killer_name.clone(),
            was_incap: kill.was_incap,
        };

        Self {
            source: SourceSnapshot {
                file_name: bundle.replay.source.file_name.clone(),
                size_bytes: bundle.replay.source.size_bytes,
                sha256: bundle.replay.source.sha256.clone(),
            },
            replay: ReplaySnapshot {
                map_name: bundle.replay.map_name.clone(),
                squad_version: bundle.replay.squad_version.clone(),
                duration_ms: bundle.replay.duration_ms,
            },
            counts: CountsSnapshot {
                teams: bundle.teams.len(),
                squads: bundle.squads.len(),
                players: bundle.players.len(),
            },
            actors: ActorsSnapshot {
                vehicles: bundle.actors.vehicles.len(),
                helicopters: bundle.actors.helicopters.len(),
                deployables: bundle.actors.deployables.len(),
                components: bundle.actors.components.len(),
            },
            tracks: TracksSnapshot {
                players: track_kind(&bundle.tracks.players),
                vehicles: track_kind(&bundle.tracks.vehicles),
                helicopters: track_kind(&bundle.tracks.helicopters),
            },
            events: EventsSnapshot {
                kills: bundle.events.kills.len(),
                deployments: bundle.events.deployments.len(),
                seat_changes: bundle.events.seat_changes.len(),
                component_states: bundle.events.component_states.len(),
                vehicle_states: bundle.events.vehicle_states.len(),
                weapon_states: bundle.events.weapon_states.len(),
                properties: bundle.events.properties.len(),
            },
            diagnostics: DiagnosticsSnapshot {
                frames_processed: bundle.diagnostics.frames_processed,
                packets_processed: bundle.diagnostics.packets_processed,
                actor_opens: bundle.diagnostics.actor_opens,
                export_groups_discovered: bundle.diagnostics.export_groups_discovered,
                guid_to_path_size: bundle.diagnostics.guid_to_path_size,
                property_replications: bundle.diagnostics.property_replications,
                position_samples: bundle.diagnostics.position_samples,
                vehicle_position_samples: bundle.diagnostics.vehicle_position_samples,
                replay_data_chunks: bundle.diagnostics.replay_data_chunks,
                warnings: bundle.diagnostics.warnings.len(),
            },
            compat: CompatSnapshot {
                map_name: compat.map_name,
                squad_version: compat.squad_version,
                match_duration_seconds: compat.match_duration_seconds,
                kills: compat.kills.len(),
                kills_by_second_buckets: compat.kills_by_second.len(),
                player_stat_entries: compat.player_stats.len(),
                positions_seconds: compat.positions_per_second.len(),
                helicopter_positions_seconds: compat.helicopter_positions_per_second.len(),
                vehicle_positions_seconds: compat.vehicle_positions_per_second.len(),
                deployable_events: compat.deployable_events.len(),
            },
            players,
            first_kill: bundle.events.kills.first().map(to_kill_snapshot),
            last_kill: bundle.events.kills.last().map(to_kill_snapshot),
        }
    }
}

/// True when callers want golden snapshots rewritten instead of asserted.
pub fn update_snapshots_requested() -> bool {
    matches!(
        std::env::var("UPDATE_SNAPSHOTS").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Compare `actual` against the snapshot stored at `path`.
///
/// When `UPDATE_SNAPSHOTS=1` is set the file is rewritten in place and the
/// check becomes a no-op; this is the mechanism for intentionally blessing a
/// new parser behavior. Otherwise a mismatch panics with a JSON diff that
/// highlights the keys that changed.
pub fn assert_snapshot_matches(path: &Path, actual: &FixtureSnapshot) {
    let pretty = serde_json::to_string_pretty(actual)
        .expect("fixture snapshot should serialize to json")
        + "\n";

    if update_snapshots_requested() || !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("snapshot parent dir should be creatable");
        }
        std::fs::write(path, &pretty).unwrap_or_else(|error| {
            panic!(
                "failed to write snapshot {}: {error}",
                path.display()
            )
        });
        return;
    }

    let existing = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("failed to read snapshot {}: {error}", path.display())
    });
    let expected: FixtureSnapshot = serde_json::from_str(&existing).unwrap_or_else(|error| {
        panic!(
            "snapshot {} is not valid FixtureSnapshot json: {error}\n\
             re-run with UPDATE_SNAPSHOTS=1 to regenerate after intentional changes",
            path.display()
        )
    });

    if &expected != actual {
        let expected_pretty = serde_json::to_string_pretty(&expected).unwrap_or_default();
        panic!(
            "fixture snapshot drift detected for {}\n\
             --- expected ---\n{expected_pretty}\n\
             --- actual ---\n{pretty}\n\
             If the change is intentional, rerun with UPDATE_SNAPSHOTS=1 to bless it.",
            path.display()
        );
    }
}

pub fn sample_bundle() -> Bundle {
    Bundle {
        replay: ReplayInfoSection {
            source: ReplaySourceInfo {
                file_name: "sample.replay".to_string(),
                size_bytes: 123,
                sha256: "abc".to_string(),
            },
            map_name: Some("Jensens_Range".to_string()),
            squad_version: Some("//Squad/v10.3.1".to_string()),
            duration_ms: 10_000,
            notes: vec![
                "Canonical bundle produced directly from a single replay ingest.".to_string(),
            ],
            ..ReplayInfoSection::default()
        },
        actors: ActorGroups {
            helicopters: vec![ActorEntity {
                actor_guid: 754,
                class_name: Some("BP_Loach_CAS_Small_C".to_string()),
                ..ActorEntity::default()
            }],
            components: vec![ComponentEntity {
                component_guid: 3334,
                owner_actor_guid: Some(754),
                class_name: Some("rotor".to_string()),
                component_class: Some("SQRotorComponent".to_string()),
                path_hint: Some("MainRotorComponent".to_string()),
                group_path: Some("/Script/Squad.SQRotorComponent".to_string()),
                first_seen_ms: 16,
                ..ComponentEntity::default()
            }],
            ..ActorGroups::default()
        },
        tracks: TrackGroups {
            helicopters: vec![Track3 {
                key: "LOACH_754".to_string(),
                actor_guid: Some(754),
                class_name: Some("BP_Loach_CAS_Small_C".to_string()),
                source: "movement_component_anchored".to_string(),
                samples: vec![TrackSample3 {
                    t_ms: 16,
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }],
                ..Track3::default()
            }],
            ..TrackGroups::default()
        },
        events: EventGroups {
            component_states: vec![ComponentStateEvent {
                t_ms: 16,
                second: 0,
                component_guid: Some(3334),
                owner_actor_guid: Some(754),
                component_type: "rotor".to_string(),
                component_name: Some("MainRotorComponent".to_string()),
                component_class: Some("SQRotorComponent".to_string()),
                group_path: "/Script/Squad.SQRotorComponent".to_string(),
                property_name: "Health".to_string(),
                decoded: DecodedPropertyValue {
                    bits: 32,
                    int32: Some(1137180672),
                    float32: Some(400.0),
                    ..DecodedPropertyValue::default()
                },
                value_float: Some(400.0),
                ..ComponentStateEvent::default()
            }],
            properties: vec![PropertyEvent {
                t_ms: 16,
                second: 0,
                channel_index: 1,
                actor_guid: Some(754),
                group_path: "/Script/Squad.SQRotorComponent".to_string(),
                property_name: "Health".to_string(),
                sub_object_net_guid: Some(3334),
                decoded: DecodedPropertyValue {
                    bits: 32,
                    int32: Some(1137180672),
                    float32: Some(400.0),
                    ..DecodedPropertyValue::default()
                },
            }],
            ..EventGroups::default()
        },
        diagnostics: Diagnostics {
            provenance_report: vec![ProvenanceEntry {
                family: "events.component_states".to_string(),
                provenance: "grouped_projection_with_raw_payload_preserved".to_string(),
                notes: vec!["test".to_string()],
            }],
            ..Diagnostics::default()
        },
        ..Bundle::default()
    }
}

