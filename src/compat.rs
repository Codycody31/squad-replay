use crate::bundle::{
    Bundle, CompatDebugStats, CompatDeployableEvent, CompatKillEvent, CompatMatch,
    CompatPlayerStat, Track3,
};
use crate::classify::classify_deployable_event_type;
use std::collections::{BTreeMap, HashMap};

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn fill_forward_tracks(
    tracks: &[Track3],
    end_second: u32,
) -> BTreeMap<String, BTreeMap<String, [f64; 3]>> {
    let mut out: BTreeMap<String, BTreeMap<String, [f64; 3]>> = BTreeMap::new();

    for track in tracks {
        if track.samples.is_empty() {
            continue;
        }
        let mut samples = track.samples.clone();
        samples.sort_by_key(|sample| sample.t_ms);

        for idx in 0..samples.len() {
            let current = &samples[idx];
            let start = (current.t_ms / 1000) as u32;
            let end = if let Some(next) = samples.get(idx + 1) {
                (next.t_ms / 1000) as u32
            } else {
                end_second
            };
            let end = end.max(start);

            for second in start..=end {
                out.entry(second.to_string()).or_default().insert(
                    track.key.clone(),
                    [round2(current.x), round2(current.y), round2(current.z)],
                );
            }
        }
    }

    out
}

pub fn from_bundle(bundle: &Bundle) -> CompatMatch {
    let duration_seconds = (bundle.replay.duration_ms / 1000) as u32;

    let mut kills = Vec::new();
    let mut kills_by_second: BTreeMap<String, Vec<CompatKillEvent>> = BTreeMap::new();
    let mut player_stats: HashMap<String, CompatPlayerStat> = HashMap::new();

    for kill in &bundle.events.kills {
        let event = CompatKillEvent {
            timestamp: kill.second,
            victim_name: kill
                .victim_name
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            killer_name: kill
                .killer_name
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            victim_guid_str: kill
                .victim_guid
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string()),
            killer_guid_str: kill
                .killer_guid
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string()),
            was_incap: kill.was_incap.unwrap_or(false),
        };
        kills_by_second
            .entry(event.timestamp.to_string())
            .or_default()
            .push(event.clone());
        kills.push(event.clone());

        if !event.killer_name.is_empty() && event.killer_name != "Unknown" {
            player_stats
                .entry(event.killer_name.clone())
                .or_default()
                .kills += 1;
        }
        if !event.victim_name.is_empty() && event.victim_name != "Unknown" {
            player_stats
                .entry(event.victim_name.clone())
                .or_default()
                .deaths += 1;
        }
    }

    for player in &bundle.players {
        if let Some(name) = &player.name {
            player_stats.entry(name.clone()).or_default();
        }
    }

    let player_tracks = fill_forward_tracks(&bundle.tracks.players, duration_seconds);
    let helicopter_tracks = fill_forward_tracks(&bundle.tracks.helicopters, duration_seconds);
    let vehicle_tracks = fill_forward_tracks(&bundle.tracks.vehicles, duration_seconds);

    let deployable_events = bundle
        .events
        .deployments
        .iter()
        .map(|deployment| CompatDeployableEvent {
            r#type: deployment.deployment_type.clone(),
            class_path: deployment
                .class_name
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            second: deployment.second,
            x: round1(deployment.x.unwrap_or_default()),
            y: round1(deployment.y.unwrap_or_default()),
            z: round1(deployment.z.unwrap_or_default()),
        })
        .collect::<Vec<_>>();

    let mut player_stats_sorted = BTreeMap::new();
    for (name, stat) in player_stats {
        player_stats_sorted.insert(name, stat);
    }

    CompatMatch {
        map_name: bundle.replay.map_name.clone().unwrap_or_default(),
        squad_version: bundle.replay.squad_version.clone().unwrap_or_default(),
        match_duration_seconds: duration_seconds,
        kills,
        kills_by_second,
        player_stats: player_stats_sorted,
        positions_per_second: player_tracks,
        helicopter_positions_per_second: helicopter_tracks,
        vehicle_positions_per_second: vehicle_tracks,
        deployable_events: if deployable_events.is_empty() {
            bundle
                .actors
                .deployables
                .iter()
                .map(|actor| CompatDeployableEvent {
                    r#type: classify_deployable_event_type(
                        actor.class_name.as_deref().unwrap_or_default(),
                    )
                    .to_string(),
                    class_path: actor
                        .archetype_path
                        .clone()
                        .or_else(|| actor.class_name.clone())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    second: (actor.open_time_ms / 1000) as u32,
                    x: actor
                        .initial_location
                        .map(|location| round1(location.x))
                        .unwrap_or_default(),
                    y: actor
                        .initial_location
                        .map(|location| round1(location.y))
                        .unwrap_or_default(),
                    z: actor
                        .initial_location
                        .map(|location| round1(location.z))
                        .unwrap_or_default(),
                })
                .collect()
        } else {
            deployable_events
        },
        debug_stats: CompatDebugStats {
            frames_processed: bundle.diagnostics.frames_processed,
            packets_processed: bundle.diagnostics.packets_processed,
            actor_opens: bundle.diagnostics.actor_opens,
            prop_replications: bundle.diagnostics.property_replications,
            position_samples: bundle.diagnostics.position_samples,
            vehicle_position_samples: bundle.diagnostics.vehicle_position_samples,
            deployable_events: bundle.actors.deployables.len(),
            export_groups_discovered: bundle.diagnostics.export_groups_discovered,
            guid_to_path_size: bundle.diagnostics.guid_to_path_size,
        },
    }
}
