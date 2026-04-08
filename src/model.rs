use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub name: String,
    pub version: u16,
    pub profile: String,
}

impl Default for SchemaInfo {
    fn default() -> Self {
        Self {
            name: "sqrj".to_string(),
            version: 1,
            profile: "canonical".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplaySourceInfo {
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplayEngineInfo {
    pub engine_version: Option<String>,
    pub net_version: Option<u32>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplayInfoSection {
    pub source: ReplaySourceInfo,
    pub engine: ReplayEngineInfo,
    pub map_name: Option<String>,
    pub layer_name: Option<String>,
    pub friendly_name: Option<String>,
    pub squad_version: Option<String>,
    pub duration_ms: u64,
    pub started_at: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Team {
    pub id: u32,
    pub name: Option<String>,
    pub faction: Option<String>,
    pub faction_setup_id: Option<String>,
    pub tickets: Option<u32>,
    pub commander_state_guid: Option<u32>,
    pub team_state_guid: Option<u32>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Squad {
    pub id: u32,
    pub raw_team_id: Option<u32>,
    pub team_id: Option<u32>,
    pub faction: Option<String>,
    pub squad_state_guid: Option<u32>,
    pub name: Option<String>,
    pub leader_player_state_guid: Option<u32>,
    pub leader_name: Option<String>,
    pub leader_steam_id: Option<String>,
    pub leader_eos_id: Option<String>,
    pub creator_name: Option<String>,
    pub creator_identity_raw: Option<String>,
    pub creator_steam_id: Option<String>,
    pub creator_eos_id: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Player {
    pub player_state_guid: u32,
    pub name: Option<String>,
    pub steam_id: Option<String>,
    pub eos_id: Option<String>,
    pub online_user_id: Option<String>,
    pub identity_raw: Option<String>,
    pub soldier_guid: Option<u32>,
    pub current_pawn_guid: Option<u32>,
    pub team_id: Option<u32>,
    pub faction: Option<String>,
    pub team_state_guid: Option<u32>,
    pub squad_id: Option<u32>,
    pub squad_state_guid: Option<u32>,
    pub current_role_id: Option<i32>,
    pub current_role_name: Option<String>,
    pub deploy_role_id: Option<i32>,
    pub deploy_role_name: Option<String>,
    pub player_type_name: Option<String>,
    pub squad_leader_name: Option<String>,
    pub squad_creator_name: Option<String>,
    pub squad_creator_steam_id: Option<String>,
    pub squad_creator_eos_id: Option<String>,
    pub start_time_ms: Option<u64>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Rotator {
    pub pitch: f64,
    pub yaw: f64,
    pub roll: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepMovement {
    pub location: Option<Vec3>,
    pub rotation: Option<Rotator>,
    pub linear_velocity: Option<Vec3>,
    pub angular_velocity: Option<Vec3>,
    pub server_frame: Option<u32>,
    pub server_handle: Option<u32>,
    pub rep_physics: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DecodedPropertyValue {
    pub bits: u32,
    pub int_packed: Option<u32>,
    pub int32: Option<i32>,
    pub float32: Option<f32>,
    pub boolean: Option<bool>,
    pub string: Option<String>,
    // Boxed because RepMovement is ~150 bytes but most events don't carry
    // movement data. Inlining it roughly doubled PropertyEvent for no gain.
    pub rep_movement: Option<Box<RepMovement>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PropertyEvent {
    pub t_ms: u64,
    pub second: u32,
    pub channel_index: u32,
    pub actor_guid: Option<u32>,
    // Very low cardinality across a replay (a few dozen unique values for
    // millions of events). Arc<str> lets them share backing storage via the
    // interner on ParseState.
    pub group_path: Arc<str>,
    pub property_name: Arc<str>,
    pub sub_object_net_guid: Option<u32>,
    pub decoded: DecodedPropertyValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActorEntity {
    pub actor_guid: u32,
    pub channel_index: u32,
    pub class_name: Option<String>,
    pub archetype_path: Option<String>,
    pub open_time_ms: u64,
    pub close_time_ms: Option<u64>,
    pub initial_location: Option<Vec3>,
    pub initial_rotation: Option<Rotator>,
    pub team: Option<i64>,
    pub build_state: Option<i64>,
    pub health: Option<f64>,
    pub owner: Option<u32>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentEntity {
    pub component_guid: u32,
    pub owner_actor_guid: Option<u32>,
    pub class_name: Option<String>,
    #[serde(default)]
    pub component_class: Option<String>,
    pub path_hint: Option<String>,
    #[serde(default)]
    pub group_path: Option<String>,
    pub first_seen_ms: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActorGroups {
    pub vehicles: Vec<ActorEntity>,
    pub helicopters: Vec<ActorEntity>,
    pub deployables: Vec<ActorEntity>,
    pub components: Vec<ComponentEntity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackSample3 {
    pub t_ms: u64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Track3 {
    pub key: String,
    pub actor_guid: Option<u32>,
    pub player_state_guid: Option<u32>,
    pub class_name: Option<String>,
    pub source: String,
    pub samples: Vec<TrackSample3>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackGroups {
    pub players: Vec<Track3>,
    pub vehicles: Vec<Track3>,
    pub helicopters: Vec<Track3>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KillEvent {
    pub t_ms: u64,
    pub second: u32,
    pub victim_name: Option<String>,
    pub killer_name: Option<String>,
    pub victim_guid: Option<u32>,
    pub killer_guid: Option<u32>,
    pub was_incap: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeploymentEvent {
    pub t_ms: u64,
    pub second: u32,
    pub actor_guid: Option<u32>,
    pub deployment_type: String,
    pub class_name: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeatChangeEvent {
    pub t_ms: u64,
    pub second: u32,
    pub actor_guid: Option<u32>,
    pub component_guid: Option<u32>,
    pub player_state_guid: Option<u32>,
    pub vehicle_class: Option<String>,
    pub seat_attach_socket: Option<String>,
    pub attach_socket_name: Option<String>,
    pub occupant_name: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentStateEvent {
    pub t_ms: u64,
    pub second: u32,
    pub component_guid: Option<u32>,
    pub owner_actor_guid: Option<u32>,
    pub component_type: String,
    #[serde(default)]
    pub component_name: Option<String>,
    #[serde(default)]
    pub component_class: Option<String>,
    #[serde(default)]
    pub group_path: String,
    pub property_name: String,
    #[serde(default)]
    pub decoded: DecodedPropertyValue,
    pub value_int: Option<i64>,
    pub value_float: Option<f64>,
    pub value_bool: Option<bool>,
    pub value_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VehicleStateEvent {
    pub t_ms: u64,
    pub second: u32,
    pub actor_guid: Option<u32>,
    #[serde(default)]
    pub actor_class: Option<String>,
    #[serde(default)]
    pub sub_object_net_guid: Option<u32>,
    #[serde(default)]
    pub group_path: String,
    pub property_name: String,
    #[serde(default)]
    pub decoded: DecodedPropertyValue,
    pub value_int: Option<i64>,
    pub value_float: Option<f64>,
    pub value_bool: Option<bool>,
    pub value_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeaponStateEvent {
    pub t_ms: u64,
    pub second: u32,
    pub actor_guid: Option<u32>,
    #[serde(default)]
    pub actor_class: Option<String>,
    #[serde(default)]
    pub sub_object_net_guid: Option<u32>,
    #[serde(default)]
    pub group_path: String,
    pub property_name: String,
    #[serde(default)]
    pub decoded: DecodedPropertyValue,
    pub value_int: Option<i64>,
    pub value_float: Option<f64>,
    pub value_bool: Option<bool>,
    pub value_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventGroups {
    pub kills: Vec<KillEvent>,
    pub deployments: Vec<DeploymentEvent>,
    pub seat_changes: Vec<SeatChangeEvent>,
    pub component_states: Vec<ComponentStateEvent>,
    pub vehicle_states: Vec<VehicleStateEvent>,
    pub weapon_states: Vec<WeaponStateEvent>,
    pub properties: Vec<PropertyEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StringInventory {
    pub ascii_strings: Vec<String>,
    pub utf16_strings: Vec<String>,
    pub class_paths: Vec<String>,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvenanceEntry {
    pub family: String,
    pub provenance: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Diagnostics {
    pub frames_processed: u64,
    pub packets_processed: u64,
    pub actor_opens: u64,
    pub export_groups_discovered: usize,
    pub guid_to_path_size: usize,
    pub property_replications: u64,
    pub position_samples: u64,
    pub vehicle_position_samples: u64,
    pub replay_data_chunks: usize,
    pub warnings: Vec<String>,
    pub string_inventory: StringInventory,
    #[serde(default)]
    pub provenance_report: Vec<ProvenanceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameStateInfo {
    // Match identity
    pub server_name: Option<String>,
    pub game_mode: Option<String>,
    pub match_state: Option<String>,
    pub match_id: Option<String>,
    pub map_name: Option<String>,

    // Server info
    pub max_players: Option<u32>,
    pub motd: Option<String>,
    pub server_tick_rate: Option<f32>,
    pub server_start_timestamp: Option<String>,
    pub startup_layer: Option<String>,

    // Match config
    pub is_ticket_based: Option<bool>,
    pub authority_num_teams: Option<u32>,
    pub num_reserved_slots: Option<u32>,
    pub public_queue_limit: Option<u32>,
    pub num_players_diff_for_team_changes: Option<u32>,
    pub low_player_count_threshold: Option<u32>,
    pub community_admin_access: Option<bool>,

    // Timing
    pub no_team_change_timer: Option<f32>,
    pub server_message_interval: Option<f32>,
    pub time_between_matches: Option<f32>,
    pub time_before_vote: Option<f32>,

    // Rotation & voting
    pub map_rotation_mode: Option<u32>,
    pub use_vote_level: Option<bool>,
    pub use_vote_layer: Option<bool>,
    pub layer_options_number: Option<u32>,
    pub faction_options_number: Option<u32>,
    pub map_skip_rounds: Option<u32>,
    pub layer_skip_rounds: Option<u32>,
    pub faction_skip_rounds: Option<u32>,
    pub faction_setup_skip_rounds: Option<u32>,
    pub display_votes: Option<bool>,
    pub unique_map_vote: Option<bool>,

    // Availability flags (mode-specific, all optional)
    pub vehicle_claiming_disabled: Option<bool>,
    pub commander_disabled: Option<bool>,
    pub force_all_role_availability: Option<bool>,
    pub helicopters_available: Option<bool>,
    pub boats_available: Option<bool>,
    pub tanks_available: Option<bool>,
    pub force_all_vehicle_availability: Option<bool>,
    pub force_all_deployable_availability: Option<bool>,
    pub force_all_action_availability: Option<bool>,
    pub force_allow_commander_actions: Option<bool>,
    pub force_no_commander_cooldowns: Option<bool>,
    pub no_respawn_timer: Option<bool>,
    pub vehicle_team_requirement_disabled: Option<bool>,
    pub vehicle_kit_requirement_disabled: Option<bool>,

    // Arrays (accumulated from repeated property events)
    pub server_tags: Vec<String>,
    pub level_rotation: Vec<String>,
    pub layer_rotation: Vec<String>,
    pub layer_rotation_low_players: Vec<String>,
    pub layer_vote_list: Vec<String>,
    pub excluded_levels: Vec<String>,
    pub excluded_layers: Vec<String>,

    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Bundle {
    pub schema: SchemaInfo,
    pub replay: ReplayInfoSection,
    pub game_state: GameStateInfo,
    pub teams: Vec<Team>,
    pub squads: Vec<Squad>,
    pub players: Vec<Player>,
    pub actors: ActorGroups,
    pub tracks: TrackGroups,
    pub events: EventGroups,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompatKillEvent {
    pub timestamp: u32,
    #[serde(rename = "victimName")]
    pub victim_name: String,
    #[serde(rename = "killerName")]
    pub killer_name: String,
    #[serde(rename = "victimGuidStr")]
    pub victim_guid_str: String,
    #[serde(rename = "killerGuidStr")]
    pub killer_guid_str: String,
    #[serde(rename = "wasIncap")]
    pub was_incap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompatPlayerStat {
    pub kills: u32,
    pub deaths: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompatDeployableEvent {
    pub r#type: String,
    #[serde(rename = "classPath")]
    pub class_path: String,
    pub second: u32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompatDebugStats {
    #[serde(rename = "framesProcessed")]
    pub frames_processed: u64,
    #[serde(rename = "packetsProcessed")]
    pub packets_processed: u64,
    #[serde(rename = "actorOpens")]
    pub actor_opens: u64,
    #[serde(rename = "propReplications")]
    pub prop_replications: u64,
    #[serde(rename = "positionSamples")]
    pub position_samples: u64,
    #[serde(rename = "vehiclePositionSamples")]
    pub vehicle_position_samples: u64,
    #[serde(rename = "deployableEvents")]
    pub deployable_events: usize,
    #[serde(rename = "exportGroupsDiscovered")]
    pub export_groups_discovered: usize,
    #[serde(rename = "guidToPathSize")]
    pub guid_to_path_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompatMatch {
    #[serde(rename = "mapName")]
    pub map_name: String,
    #[serde(rename = "squadVersion")]
    pub squad_version: String,
    #[serde(rename = "matchDurationSeconds")]
    pub match_duration_seconds: u32,
    pub kills: Vec<CompatKillEvent>,
    #[serde(rename = "killsBySecond")]
    pub kills_by_second: BTreeMap<String, Vec<CompatKillEvent>>,
    #[serde(rename = "playerStats")]
    pub player_stats: BTreeMap<String, CompatPlayerStat>,
    #[serde(rename = "positionsPerSecond")]
    pub positions_per_second: BTreeMap<String, BTreeMap<String, [f64; 3]>>,
    #[serde(rename = "helicopterPositionsPerSecond")]
    pub helicopter_positions_per_second: BTreeMap<String, BTreeMap<String, [f64; 3]>>,
    #[serde(rename = "vehiclePositionsPerSecond")]
    pub vehicle_positions_per_second: BTreeMap<String, BTreeMap<String, [f64; 3]>>,
    #[serde(rename = "deployableEvents")]
    pub deployable_events: Vec<CompatDeployableEvent>,
    #[serde(rename = "debugStats")]
    pub debug_stats: CompatDebugStats,
}

#[derive(Debug, Clone)]
pub struct ParseOptions {
    pub include_property_events: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            include_property_events: true,
        }
    }
}
