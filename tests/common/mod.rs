use squadreplay::bundle::{
    ActorEntity, ActorGroups, Bundle, ComponentEntity, ComponentStateEvent, DecodedPropertyValue,
    Diagnostics, EventGroups, PropertyEvent, ProvenanceEntry, ReplayInfoSection, ReplaySourceInfo,
    Track3, TrackGroups, TrackSample3,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn unique_path(prefix: &str, suffix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{unique}{suffix}"))
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
