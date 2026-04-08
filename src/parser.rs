use crate::bundle::{
    ActorEntity, ActorGroups, Bundle, ComponentEntity, ComponentStateEvent, DecodedPropertyValue,
    DeploymentEvent, Diagnostics, EventGroups, KillEvent, Player, PropertyEvent, ProvenanceEntry,
    RepMovement, ReplayEngineInfo, ReplayInfoSection, ReplaySourceInfo, Rotator, SchemaInfo,
    SeatChangeEvent, Squad, StringInventory, Team, Track3, TrackGroups, TrackSample3, Vec3,
    VehicleStateEvent, WeaponStateEvent,
};
use crate::classify::{
    classify_deployable_event_type, infer_component_type, infer_group_leaf,
    is_deployable_primary_type, is_helicopter_type, is_soldier_type, is_vehicle_type,
    normalize_type,
};
use crate::error::{Error, Result};
use crate::unreal_names::unreal_name;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

const OUTER_MAGIC: u32 = 0x1CA2E27F;
const INNER_MAGIC: u32 = 0x2CF5A13D;

#[derive(Debug, Clone, Default)]
struct OuterInfo {
    file_version: u32,
    length_in_ms: u32,
    network_version: u32,
    changelist: u32,
    friendly_name: String,
    is_live: bool,
    is_compressed: bool,
    is_encrypted: bool,
    header_end: usize,
}

#[derive(Debug, Clone, Default)]
struct DemoHeader {
    network_version: u32,
    network_checksum: u32,
    engine_network_version: u32,
    game_network_protocol_version: u32,
    patch: u16,
    changelist: u32,
    branch: String,
    flags: u32,
    level_names_and_times: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Default)]
struct ReplayDataChunk {
    length: u32,
    start_pos: usize,
}

#[derive(Debug, Clone, Default)]
struct ExportField {
    handle: u32,
    name: String,
}

#[derive(Debug, Clone, Default)]
struct ExportGroup {
    path_name: String,
    net_field_exports_length: u32,
    net_field_exports: HashMap<u32, ExportField>,
}

#[derive(Debug, Clone, Copy, Default)]
struct NetworkGuid {
    value: u32,
}

impl NetworkGuid {
    fn is_valid(self) -> bool {
        self.value > 0
    }

    fn is_dynamic(self) -> bool {
        self.value > 0 && (self.value & 1) != 1
    }

    fn is_default(self) -> bool {
        self.value == 1
    }
}

#[derive(Debug, Clone, Default)]
struct OpenedActor {
    actor_net_guid: NetworkGuid,
    archetype: Option<NetworkGuid>,
    level: Option<NetworkGuid>,
    location: Option<Vec3>,
    rotation: Option<Rotator>,
    scale: Option<Vec3>,
    velocity: Option<Vec3>,
}

#[derive(Debug, Clone, Default)]
struct ChannelState {
    actor: Option<OpenedActor>,
}

#[derive(Debug, Clone, Default)]
struct ActorBuilder {
    actor_guid: u32,
    channel_index: u32,
    class_name: Option<String>,
    archetype_path: Option<String>,
    open_time_ms: u64,
    close_time_ms: Option<u64>,
    initial_location: Option<Vec3>,
    initial_rotation: Option<Rotator>,
    team: Option<i64>,
    build_state: Option<i64>,
    health: Option<f64>,
    owner: Option<u32>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PlayerBuilder {
    player_state_guid: u32,
    name: Option<String>,
    steam_id: Option<String>,
    eos_id: Option<String>,
    online_user_id: Option<String>,
    identity_raw: Option<String>,
    soldier_guid: Option<u32>,
    current_pawn_guid: Option<u32>,
    team_state_guid: Option<u32>,
    squad_state_guid: Option<u32>,
    current_role_id: Option<i32>,
    current_role_name: Option<String>,
    deploy_role_id: Option<i32>,
    deploy_role_name: Option<String>,
    player_type_name: Option<String>,
    start_time_ms: Option<u64>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct DeployableBuilder {
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct ComponentBuilder {
    component_guid: u32,
    owner_actor_guid: Option<u32>,
    class_name: Option<String>,
    component_class: Option<String>,
    path_hint: Option<String>,
    group_path: Option<String>,
    first_seen_ms: u64,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct RawSample {
    t_ms: u64,
    actor_guid: Option<u32>,
    player_state_guid: Option<u32>,
    key: Option<String>,
    class_name: Option<String>,
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Debug, Clone, Default)]
struct HelicopterMovementSample {
    t_ms: u64,
    actor_guid: u32,
    payload_bits: u32,
    movement: RepMovement,
}

#[derive(Debug, Clone, Copy)]
struct RepMovementDecodeConfig {
    skip_bits: usize,
    location_scale: f64,
    location_max_bits: u32,
    rotation_short: bool,
    velocity_scale: f64,
    velocity_max_bits: u32,
}

const STANDARD_REP_MOVEMENT_CONFIG: RepMovementDecodeConfig = RepMovementDecodeConfig {
    skip_bits: 0,
    location_scale: 100.0,
    location_max_bits: 30,
    rotation_short: false,
    velocity_scale: 1.0,
    velocity_max_bits: 24,
};

const PRIMARY_HELO_REP_MOVEMENT_CONFIG: RepMovementDecodeConfig = RepMovementDecodeConfig {
    skip_bits: 5,
    location_scale: 1.0,
    location_max_bits: 24,
    rotation_short: false,
    velocity_scale: 10.0,
    velocity_max_bits: 27,
};

const HELO_SPEED_THRESHOLD: f64 = 50_000.0;
const HELO_Z_JUMP_THRESHOLD: f64 = 8_000.0;
const HELO_WORLD_BOUND: f64 = 100_000.0;
const HELO_Z_BOUND: f64 = 30_000.0;
const HELO_DECODE_PADDING_BYTES: usize = 4;
const HELO_PRIMARY_PAYLOAD_BITS: u32 = 141;
const HELO_NON_PRIMARY_LOCAL_MIN_ABS: f64 = 1200.0;

#[derive(Debug, Clone, Default)]
struct PartialBunch {
    archive: BitReader,
    packet_id: u32,
    ch_index: u32,
    ch_sequence: u32,
    b_open: bool,
    b_reliable: bool,
    b_has_package_export_maps: bool,
    b_has_must_be_mapped_guids: bool,
    time_seconds: f32,
}

#[derive(Debug, Clone, Default)]
struct ParseState {
    groups_by_path: HashMap<String, ExportGroup>,
    groups_by_index: HashMap<u32, String>,
    guid_to_path: HashMap<u32, String>,
    channels: HashMap<u32, ChannelState>,
    ignored_channels: HashMap<u32, bool>,
    actor_to_channel: HashMap<u32, u32>,
    channel_to_actor: HashMap<u32, u32>,
    partial_bunch: Option<PartialBunch>,
    external_data: HashMap<u32, ExternalData>,
    actor_builders: HashMap<u32, ActorBuilder>,
    player_builders: HashMap<u32, PlayerBuilder>,
    deployables: HashMap<u32, DeployableBuilder>,
    component_builders: HashMap<u32, ComponentBuilder>,
    property_events: Vec<PropertyEvent>,
    component_state_events: Vec<ComponentStateEvent>,
    deployment_events: Vec<DeploymentEvent>,
    vehicle_state_events: Vec<VehicleStateEvent>,
    weapon_state_events: Vec<WeaponStateEvent>,
    raw_player_samples: Vec<RawSample>,
    raw_vehicle_samples: Vec<RawSample>,
    raw_helicopter_samples: Vec<HelicopterMovementSample>,
    in_packet_id: u32,
    in_reliable: u32,
    last_frame_time: f32,
    frames_processed: u64,
    packets_processed: u64,
    actor_opens: u64,
    property_replications: u64,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct ExternalData {
    handle: u8,
    payload: Vec<u8>,
}

type VehicleTrackEntry = (Option<u32>, Option<String>, Vec<TrackSample3>);
type SeenSeatKey = (u64, Option<u32>, Option<u32>, Option<u32>, String);

#[derive(Clone, Copy)]
struct PropertyContext<'a> {
    actor: Option<&'a OpenedActor>,
    group_path: &'a str,
    group_leaf: &'a str,
    property_name: &'a str,
    t_ms: u64,
    channel_index: u32,
    sub_object_net_guid: Option<u32>,
}

#[derive(Clone, Copy)]
struct ReplicationContext<'a> {
    actor: Option<&'a OpenedActor>,
    channel_index: u32,
    group_path: &'a str,
    time_seconds: f32,
    sub_object_net_guid: Option<u32>,
}

#[derive(Debug, Clone)]
struct ByteCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        if self.pos + len > self.data.len() {
            return Err(Error::InvalidReplay(
                "unexpected end of wrapper".to_string(),
            ));
        }
        let out = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_exact(2)?.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    fn read_string(&mut self) -> Result<String> {
        let length = self.read_i32()?;
        if length == 0 {
            return Ok(String::new());
        }
        if length < 0 {
            let chars = (-length) as usize;
            let bytes = self.read_exact(chars * 2)?;
            let payload = &bytes[..bytes.len().saturating_sub(2)];
            return Ok(String::from_utf16_lossy(
                &payload
                    .chunks_exact(2)
                    .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect::<Vec<_>>(),
            ));
        }
        let bytes = self.read_exact(length as usize)?;
        let payload = &bytes[..bytes.len().saturating_sub(1)];
        Ok(String::from_utf8_lossy(payload).into_owned())
    }

    fn read_bool32(&mut self) -> Result<bool> {
        let bytes = self.read_exact(4)?;
        Ok((bytes[0] & 1) == 1)
    }
}

#[derive(Debug, Clone, Default)]
struct BitReader {
    data: Arc<Vec<u8>>,
    offset: usize,
    last_bit: usize,
    offsets: Vec<Option<usize>>,
    is_error: bool,
    header: DemoHeader,
    outer: OuterInfo,
}

impl BitReader {
    fn new(data: impl Into<Arc<Vec<u8>>>) -> Self {
        let data = data.into();
        let last_bit = data.len() * 8;
        Self {
            data,
            offset: 0,
            last_bit,
            offsets: Vec::new(),
            is_error: false,
            header: DemoHeader::default(),
            outer: OuterInfo::default(),
        }
    }

    fn with_bounds(data: impl Into<Arc<Vec<u8>>>, bit_count: usize) -> Self {
        let mut reader = Self::new(data);
        reader.last_bit = bit_count;
        reader
    }

    fn clone_window(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            offset: self.offset,
            last_bit: self.last_bit,
            offsets: self.offsets.clone(),
            is_error: self.is_error,
            header: self.header.clone(),
            outer: self.outer.clone(),
        }
    }

    fn can_read(&self, bits: usize) -> bool {
        self.offset + bits <= self.last_bit
    }

    fn at_end(&self) -> bool {
        self.offset >= self.last_bit
    }

    fn add_offset(&mut self, index: usize, bits: usize) -> Result<()> {
        if !self.can_read(bits) {
            return Err(Error::InvalidReplay(
                "offset larger than buffer".to_string(),
            ));
        }
        if self.offsets.len() <= index {
            self.offsets.resize(index + 1, None);
        }
        self.offsets[index] = Some(self.last_bit);
        self.last_bit = self.offset + bits;
        Ok(())
    }

    fn add_offset_byte(&mut self, index: usize, bytes: usize) -> Result<()> {
        self.add_offset(index, bytes.saturating_mul(8))
    }

    fn pop_offset(&mut self, index: usize, ignore_error: bool) -> Result<()> {
        if self.is_error && !ignore_error {
            return Err(Error::InvalidReplay("too much was read".to_string()));
        }
        self.is_error = false;
        self.offset = self.last_bit;
        self.last_bit = self
            .offsets
            .get(index)
            .and_then(|value| *value)
            .ok_or_else(|| Error::InvalidReplay("offset stack underflow".to_string()))?;
        self.offsets.truncate(index);
        Ok(())
    }

    fn get_last_byte(&self) -> Option<u8> {
        let byte_index = (self.last_bit / 8).checked_sub(1)?;
        self.data.get(byte_index).copied()
    }

    fn read_bit(&mut self) -> bool {
        if self.at_end() || self.is_error {
            self.is_error = true;
            return false;
        }
        let byte_offset = self.offset / 8;
        let value = (self.data[byte_offset] >> (self.offset & 7)) & 1;
        self.offset += 1;
        value == 1
    }

    fn read_bits(&mut self, count: usize) -> Vec<u8> {
        let mut out = vec![0u8; count.div_ceil(8)];
        let mut read_bytes = 0usize;

        if (self.offset & 7) == 0 {
            read_bytes = count / 8;
            if !self.can_read(read_bytes * 8) {
                self.is_error = true;
                return Vec::new();
            }
            let start = self.offset / 8;
            out[..read_bytes].copy_from_slice(&self.data[start..start + read_bytes]);
            self.offset += read_bytes * 8;
        }

        let mut current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
        let mut current_byte_bit = 1u8 << (self.offset & 7);

        for i in (read_bytes * 8)..count {
            let bit_offset = self.offset & 7;
            let result_bit_offset = i & 7;
            let current_result_offset = i / 8;
            let current_bit = 1u8 << result_bit_offset;

            if bit_offset == 0 {
                current_byte_bit = 1;
                current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
            }

            if (current_byte & current_byte_bit) != 0 {
                out[current_result_offset] |= current_bit;
            } else {
                out[current_result_offset] &= !current_bit;
            }

            self.offset += 1;
            current_byte_bit = current_byte_bit.wrapping_shl(1);
        }

        out
    }

    fn read_bits_to_unsigned_int(&mut self, mut count: usize) -> u32 {
        let mut value = 0u32;
        let mut read_bits = 0usize;

        if (self.offset & 7) == 0 {
            let mut index = 0usize;
            while count >= 8 {
                if !self.can_read(8) {
                    self.is_error = true;
                    return 0;
                }
                value |= (self.data[self.offset / 8] as u32) << (index * 8);
                index += 1;
                count -= 8;
                read_bits += 8;
                self.offset += 8;
            }
            if count == 0 {
                return value;
            }
        }

        let mut current_bit = 1u32 << read_bits;
        let mut current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
        let mut current_byte_bit = 1u8 << (self.offset & 7);

        for _ in 0..count {
            let bit_offset = self.offset & 7;
            if bit_offset == 0 {
                current_byte_bit = 1;
                current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
            }
            if (current_byte & current_byte_bit) != 0 {
                value |= current_bit;
            }
            self.offset += 1;
            current_byte_bit = current_byte_bit.wrapping_shl(1);
            current_bit = current_bit.wrapping_shl(1);
        }

        value
    }

    fn read_serialized_int(&mut self, max_value: u32) -> u32 {
        let mut value = 0u32;
        let mut current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
        let mut current_byte_bit = 1u8 << (self.offset & 7);
        let mut mask = 1u32;

        while value.saturating_add(mask) < max_value {
            let bit_offset = self.offset & 7;
            if bit_offset == 0 {
                current_byte_bit = 1;
                current_byte = self.data.get(self.offset / 8).copied().unwrap_or(0);
            }
            if (current_byte & current_byte_bit) != 0 {
                value |= mask;
            }
            self.offset += 1;
            current_byte_bit = current_byte_bit.wrapping_shl(1);
            mask = mask.wrapping_shl(1);
        }

        value
    }

    fn read_int_packed(&mut self) -> u32 {
        let mut remaining = true;
        let mut value = 0u32;
        let mut index = 0u32;

        while remaining {
            let current_byte = self.read_byte();
            remaining = (current_byte & 1) == 1;
            let shift = 7 * index;
            let chunk = ((current_byte >> 1) as u32).wrapping_shl(shift);
            value = value.wrapping_add(chunk);
            index += 1;
        }

        value
    }

    fn read_bytes(&mut self, byte_count: usize) -> Vec<u8> {
        if (self.offset & 7) == 0 {
            let start = self.offset / 8;
            if !self.can_read(byte_count * 8) {
                self.is_error = true;
                return Vec::new();
            }
            let bytes = self.data[start..start + byte_count].to_vec();
            self.offset += byte_count * 8;
            bytes
        } else {
            let mut out = vec![0u8; byte_count];
            for byte in &mut out {
                *byte = self.read_byte();
            }
            out
        }
    }

    fn read_byte(&mut self) -> u8 {
        if (self.offset & 7) == 0 {
            if !self.can_read(8) {
                self.is_error = true;
                return 0;
            }
            let byte = self.data[self.offset / 8];
            self.offset += 8;
            byte
        } else {
            self.read_bits_to_unsigned_int(8) as u8
        }
    }

    fn read_u16(&mut self) -> u16 {
        if (self.offset & 7) == 0 {
            if !self.can_read(16) {
                self.is_error = true;
                return 0;
            }
            let start = self.offset / 8;
            self.offset += 16;
            u16::from_le_bytes(self.data[start..start + 2].try_into().unwrap())
        } else {
            self.read_bits_to_unsigned_int(16) as u16
        }
    }

    fn read_u32(&mut self) -> u32 {
        if (self.offset & 7) == 0 {
            if !self.can_read(32) {
                self.is_error = true;
                return 0;
            }
            let start = self.offset / 8;
            self.offset += 32;
            u32::from_le_bytes(self.data[start..start + 4].try_into().unwrap())
        } else {
            self.read_bits_to_unsigned_int(32)
        }
    }

    fn read_i32(&mut self) -> i32 {
        if (self.offset & 7) == 0 {
            if !self.can_read(32) {
                self.is_error = true;
                return 0;
            }
            let start = self.offset / 8;
            self.offset += 32;
            i32::from_le_bytes(self.data[start..start + 4].try_into().unwrap())
        } else {
            let mut bytes = [0u8; 4];
            for byte in &mut bytes {
                *byte = self.read_byte();
            }
            i32::from_le_bytes(bytes)
        }
    }

    fn read_u64(&mut self) -> u64 {
        if (self.offset & 7) == 0 {
            if !self.can_read(64) {
                self.is_error = true;
                return 0;
            }
            let start = self.offset / 8;
            self.offset += 64;
            u64::from_le_bytes(self.data[start..start + 8].try_into().unwrap())
        } else {
            let mut bytes = [0u8; 8];
            for byte in &mut bytes {
                *byte = self.read_byte();
            }
            u64::from_le_bytes(bytes)
        }
    }

    fn read_f32(&mut self) -> f32 {
        if (self.offset & 7) == 0 {
            if !self.can_read(32) {
                self.is_error = true;
                return 0.0;
            }
            let start = self.offset / 8;
            self.offset += 32;
            f32::from_le_bytes(self.data[start..start + 4].try_into().unwrap())
        } else {
            let mut bytes = [0u8; 4];
            for byte in &mut bytes {
                *byte = self.read_byte();
            }
            f32::from_le_bytes(bytes)
        }
    }

    fn read_f64(&mut self) -> f64 {
        if (self.offset & 7) == 0 {
            if !self.can_read(64) {
                self.is_error = true;
                return 0.0;
            }
            let start = self.offset / 8;
            self.offset += 64;
            f64::from_le_bytes(self.data[start..start + 8].try_into().unwrap())
        } else {
            let mut bytes = [0u8; 8];
            for byte in &mut bytes {
                *byte = self.read_byte();
            }
            f64::from_le_bytes(bytes)
        }
    }

    fn read_string(&mut self) -> String {
        let length = self.read_i32();
        if length == 0 {
            return String::new();
        }
        if length < 0 {
            let chars = (-length) as usize;
            let byte_len = chars * 2;
            let bytes = if (self.offset & 7) == 0 {
                if !self.can_read(byte_len * 8) {
                    self.is_error = true;
                    return String::new();
                }
                let start = self.offset / 8;
                self.offset += byte_len * 8;
                &self.data[start..start + byte_len]
            } else {
                let bytes = self.read_bytes(byte_len);
                if bytes.len() < byte_len {
                    self.is_error = true;
                    return String::new();
                }
                return String::from_utf16_lossy(
                    &bytes[..bytes.len().saturating_sub(2)]
                        .chunks_exact(2)
                        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                        .collect::<Vec<_>>(),
                );
            };
            let payload = &bytes[..bytes.len().saturating_sub(2)];
            let utf16 = payload
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&utf16)
        } else {
            let byte_len = length as usize;
            if (self.offset & 7) == 0 {
                if !self.can_read(byte_len * 8) {
                    self.is_error = true;
                    return String::new();
                }
                let start = self.offset / 8;
                self.offset += byte_len * 8;
                let payload = &self.data[start..start + byte_len.saturating_sub(1)];
                String::from_utf8_lossy(payload).into_owned()
            } else {
                let bytes = self.read_bytes(byte_len);
                if bytes.len() < byte_len {
                    self.is_error = true;
                    return String::new();
                }
                let payload = &bytes[..bytes.len().saturating_sub(1)];
                String::from_utf8_lossy(payload).into_owned()
            }
        }
    }

    fn skip_string(&mut self) {
        let length = self.read_i32();
        if length < 0 {
            self.skip_bytes((-length) as usize * 2);
        } else if length > 0 {
            self.skip_bytes(length as usize);
        }
    }

    fn read_fname(&mut self) -> String {
        let is_hardcoded = self.read_bit();
        if is_hardcoded {
            let index = if self.header.engine_network_version < 6 {
                self.read_u32()
            } else {
                self.read_int_packed()
            };
            return unreal_name(index).unwrap_or("UnknownFName").to_string();
        }
        let value = self.read_string();
        self.skip_bytes(4);
        value
    }

    fn skip_fname(&mut self) {
        let is_hardcoded = self.read_bit();
        if is_hardcoded {
            if self.header.engine_network_version < 6 {
                self.read_u32();
            } else {
                self.read_int_packed();
            }
            return;
        }
        self.skip_string();
        self.skip_bytes(4);
    }

    fn read_fname_byte(&mut self) -> String {
        let is_hardcoded = self.read_byte();
        if is_hardcoded != 0 {
            let index = if self.header.engine_network_version < 6 {
                self.read_u32()
            } else {
                self.read_int_packed()
            };
            return unreal_name(index).unwrap_or("UnknownFName").to_string();
        }
        let value = self.read_string();
        self.skip_bytes(4);
        value
    }

    fn read_vector3d(&mut self) -> Vec3 {
        if self.header.engine_network_version < 23 {
            Vec3 {
                x: self.read_f32() as f64,
                y: self.read_f32() as f64,
                z: self.read_f32() as f64,
            }
        } else {
            Vec3 {
                x: self.read_f64(),
                y: self.read_f64(),
                z: self.read_f64(),
            }
        }
    }

    fn read_quantized_vector(&mut self, scale_factor: f64) -> Vec3 {
        let bits_and_info = self.read_serialized_int(1 << 7);
        if self.is_error {
            return Vec3::default();
        }
        let component_bits = bits_and_info & 63;
        let extra_info = bits_and_info >> 6;
        if component_bits > 0 {
            let x = self.read_bits_to_unsigned_int(component_bits as usize);
            let y = self.read_bits_to_unsigned_int(component_bits as usize);
            let z = self.read_bits_to_unsigned_int(component_bits as usize);
            let sign_bit = 1u32.wrapping_shl(component_bits - 1);
            let x_sign = ((x ^ sign_bit) as i64 - sign_bit as i64) as f64;
            let y_sign = ((y ^ sign_bit) as i64 - sign_bit as i64) as f64;
            let z_sign = ((z ^ sign_bit) as i64 - sign_bit as i64) as f64;
            if extra_info != 0 {
                return Vec3 {
                    x: x_sign / scale_factor,
                    y: y_sign / scale_factor,
                    z: z_sign / scale_factor,
                };
            }
            return Vec3 {
                x: x_sign,
                y: y_sign,
                z: z_sign,
            };
        }
        let size = if extra_info != 0 { 8 } else { 4 };
        if size == 8 {
            Vec3 {
                x: self.read_f64(),
                y: self.read_f64(),
                z: self.read_f64(),
            }
        } else {
            Vec3 {
                x: self.read_f32() as f64,
                y: self.read_f32() as f64,
                z: self.read_f32() as f64,
            }
        }
    }

    fn read_packed_vector_legacy(&mut self, scale_factor: f64, max_bits: u32) -> Vec3 {
        let bits = self.read_serialized_int(max_bits);
        if self.is_error {
            return Vec3::default();
        }
        let bias = 1u32.wrapping_shl(bits + 1);
        let max = 1u32.wrapping_shl(bits + 2);
        let dx = self.read_serialized_int(max);
        let dy = self.read_serialized_int(max);
        let dz = self.read_serialized_int(max);
        if self.is_error {
            return Vec3::default();
        }
        Vec3 {
            x: (dx as f64 - bias as f64) / scale_factor,
            y: (dy as f64 - bias as f64) / scale_factor,
            z: (dz as f64 - bias as f64) / scale_factor,
        }
    }

    fn read_packed_vector(&mut self, scale_factor: f64, max_bits: u32) -> Vec3 {
        if self.header.engine_network_version >= 23 {
            self.read_quantized_vector(scale_factor)
        } else {
            self.read_packed_vector_legacy(scale_factor, max_bits)
        }
    }

    fn read_rotation(&mut self) -> Rotator {
        let mut pitch = 0.0;
        let mut yaw = 0.0;
        let mut roll = 0.0;
        if self.read_bit() {
            pitch = self.read_byte() as f64 * 360.0 / 256.0;
        }
        if self.read_bit() {
            yaw = self.read_byte() as f64 * 360.0 / 256.0;
        }
        if self.read_bit() {
            roll = self.read_byte() as f64 * 360.0 / 256.0;
        }
        Rotator { pitch, yaw, roll }
    }

    fn read_rotation_short(&mut self) -> Rotator {
        let mut pitch = 0.0;
        let mut yaw = 0.0;
        let mut roll = 0.0;
        if self.read_bit() {
            pitch = self.read_u16() as f64 * 360.0 / 65536.0;
        }
        if self.read_bit() {
            yaw = self.read_u16() as f64 * 360.0 / 65536.0;
        }
        if self.read_bit() {
            roll = self.read_u16() as f64 * 360.0 / 65536.0;
        }
        Rotator { pitch, yaw, roll }
    }

    fn skip_bits(&mut self, bits: usize) {
        self.offset += bits;
    }

    fn skip_bytes(&mut self, bytes: usize) {
        self.offset += bytes * 8;
    }

    fn go_to_byte(&mut self, byte_offset: usize) {
        self.offset = byte_offset * 8;
    }

    fn get_bits_left(&self) -> usize {
        self.last_bit.saturating_sub(self.offset)
    }

    fn has_level_streaming_fixes(&self) -> bool {
        (self.header.flags & 2) == 2
    }

    fn has_game_specific_frame_data(&self) -> bool {
        (self.header.flags & 8) == 8
    }

    fn append_data_from_checked(&mut self, data: &[u8], bit_count: usize) {
        Arc::make_mut(&mut self.data).extend_from_slice(data);
        self.last_bit += bit_count;
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn remove_path_prefix(path: &str, prefix: &str) -> String {
    if !prefix.is_empty() {
        if let Some(stripped) = path.strip_prefix(prefix) {
            return stripped.to_string();
        }
        return path.to_string();
    }
    for (index, ch) in path.char_indices().rev() {
        if ch == '.' {
            return path[index + 1..].to_string();
        }
        if ch == '/' {
            return path.to_string();
        }
    }
    remove_path_prefix(path, "Default__")
}

fn cleaned_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed
        .chars()
        .any(|ch| ch == '\u{fffd}' || (ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn ignored_fname_value(value: &str) -> bool {
    matches!(
        value,
        "None"
            | "BoolProperty"
            | "Vector"
            | "VectorProperty"
            | "ArrayProperty"
            | "NameProperty"
            | "InterfaceProperty"
            | "MulticastDelegateProperty"
            | "Box"
            | "Color"
    )
}

fn canonical_group_leaf(path: &str) -> String {
    let leaf = remove_path_prefix(path, "");
    let leaf = leaf.split(':').next().unwrap_or(&leaf);
    let leaf = leaf.trim_end_matches("_ClassNetCache");
    let leaf = leaf.strip_prefix("Default__").unwrap_or(leaf);
    leaf.to_string()
}

fn push_group_candidate(candidates: &mut Vec<String>, candidate: impl Into<String>) {
    let candidate = candidate.into();
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn canonical_script_group_candidates(hint: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let hint = hint.trim();
    if hint.is_empty() {
        return candidates;
    }

    push_group_candidate(&mut candidates, hint.to_string());

    let normalized = canonical_group_leaf(hint);
    push_group_candidate(&mut candidates, normalized.clone());

    if hint.starts_with("/Script/") {
        let trimmed = hint
            .split(':')
            .next()
            .unwrap_or(hint)
            .trim_end_matches("_ClassNetCache");
        push_group_candidate(&mut candidates, trimmed.to_string());
    }

    match normalized.as_str() {
        "SQPlayerState" => {
            push_group_candidate(&mut candidates, "/Script/Squad.SQPlayerState".to_string());
        }
        "SQSquadState" => {
            push_group_candidate(&mut candidates, "/Script/Squad.SQSquadState".to_string());
        }
        "SQSquadStatePrivateToTeam" => {
            push_group_candidate(
                &mut candidates,
                "/Script/Squad.SQSquadStatePrivateToTeam".to_string(),
            );
        }
        "SQTeamState" => {
            push_group_candidate(&mut candidates, "/Script/Squad.SQTeamState".to_string());
        }
        "SQTeamStatePrivate" => {
            push_group_candidate(
                &mut candidates,
                "/Script/Squad.SQTeamStatePrivate".to_string(),
            );
        }
        _ => {}
    }

    if !normalized.contains('/') && !normalized.is_empty() {
        push_group_candidate(&mut candidates, format!("/Script/Squad.{normalized}"));
    }

    candidates
}

fn group_for_hint(state: &ParseState, hint: &str) -> Option<ExportGroup> {
    for candidate in canonical_script_group_candidates(hint) {
        if let Some(group) = state.groups_by_path.get(&candidate) {
            return Some(group.clone());
        }
    }

    let normalized = canonical_group_leaf(hint);
    if normalized.is_empty() {
        return None;
    }

    state.groups_by_path.iter().find_map(|(group_path, group)| {
        (canonical_group_leaf(group_path) == normalized).then(|| group.clone())
    })
}

fn resolve_rep_group(
    state: &ParseState,
    actor: Option<&OpenedActor>,
    rep_object: Option<&str>,
    sub_object_net_guid: Option<u32>,
) -> Option<ExportGroup> {
    let mut hints = Vec::new();

    if let Some(raw) = rep_object {
        hints.push(raw.to_string());
        if let Ok(net_guid) = raw.parse::<u32>() {
            if let Some(path) = state.guid_to_path.get(&net_guid) {
                hints.push(path.clone());
            }
        }
    }

    if let Some(sub_guid) = sub_object_net_guid {
        if let Some(path) = state.guid_to_path.get(&sub_guid) {
            hints.push(path.clone());
        }
    }

    if let Some(actor) = actor {
        if let Some(archetype) = actor.archetype {
            if let Some(path) = state.guid_to_path.get(&archetype.value) {
                hints.push(path.clone());
            }
        }
        if let Some(path) = state.guid_to_path.get(&actor.actor_net_guid.value) {
            hints.push(path.clone());
        }
    }

    for hint in hints {
        if let Some(group) = group_for_hint(state, &hint) {
            return Some(group);
        }
    }

    None
}

fn identity_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\bEOS\b|\bEpic(?:\s*ID)?\b)\s*:\s*([0-9a-f]{32})|\bsteam\b\s*:\s*(\d{17})",
        )
        .expect("identity regex must compile")
    })
}

fn faction_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[^a-z0-9])(AFU|ADF|BAF|CAF|CRF|GFI|IMF|MEI|PLANMC|PLAAGF|PLA|RGF|TLF|USA|USMC|VDV|WPMC|CIV|INS|MEA)(?:[^a-z0-9]|$)",
        )
        .expect("faction regex must compile")
    })
}

fn canonical_faction_token(token: &str) -> String {
    let token = token.to_ascii_uppercase();
    match token.as_str() {
        "INS" | "MEI" => "MEI".to_string(),
        "MEA" | "GFI" => "GFI".to_string(),
        _ => token,
    }
}

#[derive(Debug, Clone, Default)]
struct ParsedIdentity {
    raw: Option<String>,
    steam_id: Option<String>,
    eos_id: Option<String>,
}

fn parse_identity_blob(value: &str) -> ParsedIdentity {
    let Some(cleaned) = cleaned_text(value) else {
        return ParsedIdentity::default();
    };

    let mut parsed = ParsedIdentity {
        raw: Some(cleaned.clone()),
        ..ParsedIdentity::default()
    };

    for captures in identity_regex().captures_iter(&cleaned) {
        if let Some(eos) = captures.get(1) {
            parsed.eos_id = Some(eos.as_str().to_ascii_lowercase());
        }
        if let Some(steam) = captures.get(2) {
            parsed.steam_id = Some(steam.as_str().to_string());
        }
    }

    parsed
}

fn asset_faction_token(value: Option<&str>) -> Option<String> {
    let value = value?;
    faction_regex()
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|value| canonical_faction_token(value.as_str()))
}

fn meaningful_decoded_string(decoded: &DecodedPropertyValue) -> Option<String> {
    decoded.string.as_deref().and_then(cleaned_text)
}

fn decoded_scalar_u32(decoded: &DecodedPropertyValue) -> Option<u32> {
    decoded.int_packed.or_else(|| {
        decoded
            .int32
            .and_then(|value| (value >= 0).then_some(value as u32))
    })
}

fn decoded_preferred_u32(decoded: &DecodedPropertyValue) -> Option<u32> {
    decoded
        .int32
        .and_then(|value| (value >= 0).then_some(value as u32))
        .or(decoded.int_packed)
}

fn decoded_scalar_string(decoded: &DecodedPropertyValue) -> Option<String> {
    if let Some(value) = meaningful_decoded_string(decoded) {
        return Some(value);
    }
    if let Some(value) = decoded.int32 {
        return Some(value.to_string());
    }
    if let Some(value) = decoded.int_packed {
        return Some(value.to_string());
    }
    decoded.boolean.map(|value| value.to_string())
}

fn normalized_state_values(
    property_name: &str,
    decoded: &DecodedPropertyValue,
) -> (Option<i64>, Option<f64>, Option<bool>, Option<String>) {
    let value_string = meaningful_decoded_string(decoded);
    match property_name {
        "Health" | "Throttle" | "Brake" => (
            None,
            decoded
                .float32
                .map(|value| value as f64)
                .or_else(|| decoded.int32.map(|value| value as f64))
                .or_else(|| decoded.int_packed.map(|value| value as f64)),
            None,
            value_string,
        ),
        "bIsEngineActive" | "bIsFiring" => (None, None, decoded.boolean, value_string),
        "CurrentGear" | "ReloadState" | "CurrentAmmo" | "RemainingAmmo" => (
            decoded
                .int32
                .map(|value| value as i64)
                .or(decoded.int_packed.map(|value| value as i64)),
            None,
            None,
            value_string,
        ),
        _ => (
            decoded
                .int32
                .map(|value| value as i64)
                .or(decoded.int_packed.map(|value| value as i64)),
            decoded.float32.map(|value| value as f64),
            decoded.boolean,
            value_string,
        ),
    }
}

fn state_event_actor_class(state: &ParseState, actor_guid: Option<u32>) -> Option<String> {
    actor_guid
        .and_then(|guid| state.actor_builders.get(&guid))
        .and_then(|builder| {
            builder
                .class_name
                .clone()
                .or_else(|| builder.archetype_path.as_deref().and_then(normalize_type))
        })
}

fn canonical_provenance_report() -> Vec<ProvenanceEntry> {
    vec![
        ProvenanceEntry {
            family: "replay".to_string(),
            provenance: "direct_replay_metadata".to_string(),
            notes: vec![
                "File identity, engine versions, map, branch, and duration come directly from the replay wrapper/header."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "actors.*".to_string(),
            provenance: "direct_actor_open_and_property_replication".to_string(),
            notes: vec![
                "Actor and component inventories are built from actor opens, NetGUID paths, and direct property replication."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "events.properties".to_string(),
            provenance: "direct_replay_property_stream".to_string(),
            notes: vec![
                "This is the least-derived event family and should be treated as the replay-grounded source of truth for replicated properties."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "events.component_states|vehicle_states|weapon_states".to_string(),
            provenance: "grouped_projection_with_raw_payload_preserved".to_string(),
            notes: vec![
                "Grouped state events are filtered views of the raw property stream."
                    .to_string(),
                "The `decoded` field preserves the original scalar decode; `value_*` fields are property-aware convenience projections only."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "teams|squads|players".to_string(),
            provenance: "normalized_and_joined_from_property_events".to_string(),
            notes: vec![
                "Roster entities are assembled by joining direct replay-backed player, squad, and team property events."
                    .to_string(),
                "Some fields are normalized or backfilled, such as team-id normalization and squad-creator identity backfill."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "teams.faction|players.faction".to_string(),
            provenance: "heuristic_asset_or_role_hint_with_state_fallback".to_string(),
            notes: vec![
                "Faction labels may come from actor asset prefixes or role-name hints when explicit replay state is incomplete."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "tracks.players|tracks.vehicles|tracks.helicopters".to_string(),
            provenance: "mixed_direct_and_derived_track_samples".to_string(),
            notes: vec![
                "Consult each track's `source` field for exact provenance.".to_string(),
                "Helicopter tracks may be direct movement-component reconstructions or fall back to other replay-backed movement paths."
                    .to_string(),
            ],
        },
        ProvenanceEntry {
            family: "compat-match.json".to_string(),
            provenance: "derived_from_canonical_bundle".to_string(),
            notes: vec![
                "Compatibility JSON is generated entirely from the canonical bundle."
                    .to_string(),
                "Per-second position maps are fill-forward projections of canonical track samples."
                    .to_string(),
                "Deployable events may fall back to deployable actor opens when explicit deployment events are absent."
                    .to_string(),
            ],
        },
    ]
}

fn normalize_team_id(raw_team_id: u32, known_team_ids: &HashSet<u32>) -> u32 {
    if known_team_ids.contains(&raw_team_id) {
        raw_team_id
    } else if raw_team_id > 0 && known_team_ids.contains(&(raw_team_id - 1)) {
        raw_team_id - 1
    } else {
        raw_team_id
    }
}

fn parse_online_identity(value: &str, player: &mut PlayerBuilder) {
    if let Some(cleaned) = cleaned_text(value) {
        player.online_user_id = Some(cleaned.clone());
        let parsed = parse_identity_blob(&cleaned);
        if player.steam_id.is_none() {
            player.steam_id = parsed.steam_id;
        }
        if player.eos_id.is_none() {
            player.eos_id = parsed.eos_id;
        }
    }
}

fn decode_rep_movement_with_config(
    mut reader: BitReader,
    header: &DemoHeader,
    config: RepMovementDecodeConfig,
) -> Option<RepMovement> {
    reader.header = header.clone();
    if reader.get_bits_left() < config.skip_bits {
        return None;
    }
    if config.skip_bits > 0 {
        reader.skip_bits(config.skip_bits);
    }
    let mut movement = RepMovement::default();
    let b_simulated_physic_sleep = reader.read_bit();
    let b_rep_physics = reader.read_bit();
    let mut b_rep_server_frame = false;
    let mut b_rep_server_handle = false;
    if reader.header.engine_network_version >= 25 && reader.header.engine_network_version != 26 {
        b_rep_server_frame = reader.read_bit();
        b_rep_server_handle = reader.read_bit();
    }
    let location = reader.read_packed_vector(config.location_scale, config.location_max_bits);
    let rotation = if config.rotation_short {
        reader.read_rotation_short()
    } else {
        reader.read_rotation()
    };
    let linear_velocity =
        reader.read_packed_vector(config.velocity_scale, config.velocity_max_bits);
    let angular_velocity = if b_rep_physics {
        Some(reader.read_packed_vector(config.velocity_scale, config.velocity_max_bits))
    } else {
        None
    };
    let server_frame = if b_rep_server_frame {
        Some(reader.read_int_packed())
    } else {
        None
    };
    let server_handle = if b_rep_server_handle {
        Some(reader.read_int_packed())
    } else {
        None
    };
    if reader.is_error {
        return None;
    }
    movement.location = Some(location);
    movement.rotation = Some(rotation);
    movement.linear_velocity = Some(linear_velocity);
    movement.angular_velocity = angular_velocity;
    movement.server_frame = server_frame;
    movement.server_handle = server_handle;
    movement.rep_physics = b_rep_physics;
    let _ = b_simulated_physic_sleep;
    Some(movement)
}

fn decode_rep_movement(reader: BitReader, header: &DemoHeader) -> Option<RepMovement> {
    decode_rep_movement_with_config(reader, header, STANDARD_REP_MOVEMENT_CONFIG)
}

fn decode_helicopter_component_rep_movement(
    reader: BitReader,
    header: &DemoHeader,
) -> Option<RepMovement> {
    let payload_bits = reader.get_bits_left();
    let mut payload_reader = reader.clone_window();
    let mut payload = payload_reader.read_bits(payload_bits);
    payload.resize(payload.len() + HELO_DECODE_PADDING_BYTES, 0);

    let mut lenient_reader =
        BitReader::with_bounds(payload, payload_bits + HELO_DECODE_PADDING_BYTES * 8);
    lenient_reader.header = header.clone();
    decode_rep_movement_with_config(lenient_reader, header, PRIMARY_HELO_REP_MOVEMENT_CONFIG)
}

fn is_helicopter_movement_component_group(group_path: &str) -> bool {
    canonical_group_leaf(group_path) == "SQHelicopterMovementComponent"
}

fn should_attempt_string_decode(property_name: &str) -> bool {
    property_name.contains("Name")
        || property_name.contains("Socket")
        || property_name.contains("Text")
        || matches!(
            property_name,
            "OnlineUserId"
                | "Faction"
                | "FactionId"
                | "FactionName"
                | "FactionSetupId"
                | "CurrentRoleId"
                | "DeployRoleId"
                | "Type"
                | "UniqueID"
                | "SquadCreatorSteamID"
                | "CreatorSteamId"
                | "CreatorEOSId"
                | "CreatorEosId"
                | "CreatorEpicId"
        )
}

fn decode_textual_scalar(reader: &BitReader, property_name: &str) -> Option<String> {
    if !should_attempt_string_decode(property_name) {
        return None;
    }

    if reader.get_bits_left() >= 32 {
        let mut string_reader = reader.clone_window();
        let value = string_reader.read_string();
        if !string_reader.is_error {
            if let Some(cleaned) = cleaned_text(&value) {
                return Some(cleaned);
            }
        }
    }

    let mut fname_reader = reader.clone_window();
    let value = fname_reader.read_fname();
    if fname_reader.is_error {
        return None;
    }

    let cleaned = cleaned_text(&value)?;
    if ignored_fname_value(&cleaned) {
        return None;
    }
    Some(cleaned)
}

fn decode_generic_value(
    reader: &BitReader,
    header: &DemoHeader,
    property_name: &str,
) -> DecodedPropertyValue {
    let bits = reader.get_bits_left();
    let byte_len = bits.div_ceil(8);

    let int32 = if byte_len >= 4 {
        let mut value_reader = reader.clone_window();
        Some(value_reader.read_i32()).filter(|_| !value_reader.is_error)
    } else {
        None
    };
    let float32 = if byte_len >= 4 {
        let mut value_reader = reader.clone_window();
        Some(value_reader.read_f32()).filter(|_| !value_reader.is_error)
    } else {
        None
    };
    let boolean = if bits > 0 {
        let mut value_reader = reader.clone_window();
        Some(value_reader.read_bit()).filter(|_| !value_reader.is_error)
    } else {
        None
    };

    let mut int_packed_reader = reader.clone_window();
    let int_packed =
        Some(int_packed_reader.read_int_packed()).filter(|_| !int_packed_reader.is_error);

    let string = decode_textual_scalar(reader, property_name);

    let rep_movement = if property_name == "ReplicatedMovement" {
        decode_rep_movement(reader.clone_window(), header)
    } else {
        None
    };

    DecodedPropertyValue {
        bits: bits as u32,
        int_packed,
        int32,
        float32,
        boolean,
        string,
        rep_movement,
    }
}

fn decode_net_guid(
    replay: &mut BitReader,
    is_exporting: bool,
    state: &mut ParseState,
    recursion: usize,
) -> NetworkGuid {
    if recursion > 16 {
        return NetworkGuid::default();
    }
    let net_guid = NetworkGuid {
        value: replay.read_int_packed(),
    };
    if !net_guid.is_valid() {
        return net_guid;
    }
    if net_guid.is_default() || is_exporting {
        let flags = replay.read_byte();
        if (flags & 1) == 1 {
            let _outer = decode_net_guid(replay, true, state, recursion + 1);
            let path_name = if is_exporting {
                Some(replay.read_string())
            } else {
                replay.skip_string();
                None
            };
            if (flags & 4) == 4 {
                let _checksum = replay.read_u32();
            }
            if is_exporting {
                state.guid_to_path.insert(
                    net_guid.value,
                    remove_path_prefix(path_name.as_deref().unwrap_or_default(), ""),
                );
            }
            return net_guid;
        }
    }
    net_guid
}

fn parse_wrapper(data: &[u8]) -> Result<(OuterInfo, DemoHeader, Vec<ReplayDataChunk>)> {
    let mut outer = ByteCursor::new(data);
    let magic = outer.read_u32()?;
    if magic != OUTER_MAGIC {
        return Err(Error::InvalidReplay("bad outer magic".to_string()));
    }
    let file_version = outer.read_u32()?;
    let mut outer_info = OuterInfo {
        file_version,
        ..OuterInfo::default()
    };
    if outer_info.file_version >= 7 {
        let custom_version_count = outer.read_i32()? as usize;
        outer.read_exact(custom_version_count * 20)?;
    }
    outer_info.length_in_ms = outer.read_u32()?;
    outer_info.network_version = outer.read_u32()?;
    outer_info.changelist = outer.read_u32()?;
    outer_info.friendly_name = outer.read_string()?;
    outer_info.is_live = outer.read_bool32()?;
    if outer_info.file_version >= 3 {
        let _timestamp_ticks = outer.read_u64()?;
    }
    if outer_info.file_version >= 2 {
        outer_info.is_compressed = outer.read_bool32()?;
    }
    if outer_info.file_version >= 6 {
        outer_info.is_encrypted = outer.read_bool32()?;
        let key_len = outer.read_u32()? as usize;
        outer.read_exact(key_len)?;
    }
    outer_info.header_end = outer.pos;

    let mut demo_header = DemoHeader::default();
    let mut replay_data_chunks = Vec::new();

    while outer.remaining() >= 8 {
        let chunk_type = outer.read_u32()?;
        let chunk_size = outer.read_i32()? as usize;
        let start = outer.pos;
        let end = start.saturating_add(chunk_size);
        if end > data.len() {
            return Err(Error::InvalidReplay(
                "chunk extends past end of file".to_string(),
            ));
        }
        let mut chunk_reader = ByteCursor::new(&data[start..end]);

        match chunk_type {
            0 => {
                let magic = chunk_reader.read_u32()?;
                if magic != INNER_MAGIC {
                    return Err(Error::InvalidReplay("bad inner header magic".to_string()));
                }
                demo_header.network_version = chunk_reader.read_u32()?;
                if demo_header.network_version >= 19 {
                    let custom_version_count = chunk_reader.read_i32()? as usize;
                    chunk_reader.read_exact(custom_version_count * 20)?;
                }
                demo_header.network_checksum = chunk_reader.read_u32()?;
                demo_header.engine_network_version = chunk_reader.read_u32()?;
                demo_header.game_network_protocol_version = chunk_reader.read_u32()?;
                if demo_header.network_version >= 12 {
                    let _guid = chunk_reader.read_exact(16)?;
                }
                if demo_header.network_version >= 11 {
                    chunk_reader.read_exact(4)?;
                    demo_header.patch = chunk_reader.read_u16()?;
                    demo_header.changelist = chunk_reader.read_u32()?;
                    demo_header.branch = chunk_reader.read_string()?;
                } else {
                    demo_header.changelist = chunk_reader.read_u32()?;
                }
                if demo_header.network_version >= 18 {
                    chunk_reader.read_exact(12)?;
                }
                let level_count = chunk_reader.read_u32()? as usize;
                for _ in 0..level_count {
                    let level_name = chunk_reader.read_string()?;
                    let level_time = chunk_reader.read_u32()?;
                    demo_header
                        .level_names_and_times
                        .insert(level_name, level_time);
                }
                demo_header.flags = if demo_header.network_version >= 9 {
                    chunk_reader.read_u32()?
                } else {
                    0
                };
                let game_specific_count = chunk_reader.read_u32()? as usize;
                for _ in 0..game_specific_count {
                    let _item = chunk_reader.read_string()?;
                }
            }
            1 => {
                if outer_info.file_version >= 4 {
                    chunk_reader.read_u32()?;
                    chunk_reader.read_u32()?;
                }
                let length = chunk_reader.read_u32()?;
                if outer_info.file_version >= 6 {
                    chunk_reader.read_exact(4)?;
                }
                replay_data_chunks.push(ReplayDataChunk {
                    length,
                    start_pos: start + chunk_reader.pos,
                });
            }
            _ => {}
        }

        outer.pos = end;
    }

    Ok((outer_info, demo_header, replay_data_chunks))
}

fn scan_printable_strings(data: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = Vec::new();

    for byte in data {
        if byte.is_ascii_graphic()
            || *byte == b' '
            || *byte == b'/'
            || *byte == b'_'
            || *byte == b'-'
        {
            current.push(*byte);
        } else {
            if current.len() >= 4 {
                strings.push(String::from_utf8_lossy(&current).into_owned());
            }
            current.clear();
        }
    }

    if current.len() >= 4 {
        strings.push(String::from_utf8_lossy(&current).into_owned());
    }

    strings.sort();
    strings.dedup();
    strings
}

fn scan_utf16_strings(data: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let mut j = i;
        let mut utf16 = Vec::new();
        while j + 1 < data.len() {
            let value = u16::from_le_bytes([data[j], data[j + 1]]);
            if value == 0 {
                break;
            }
            if !(value as u32 >= 0x20 && value as u32 <= 0x7e) {
                utf16.clear();
                break;
            }
            utf16.push(value);
            j += 2;
        }
        if utf16.len() >= 4 {
            strings.push(String::from_utf16_lossy(&utf16));
            i = j + 2;
        } else {
            i += 2;
        }
    }
    strings.sort();
    strings.dedup();
    strings
}

fn build_string_inventory(data: &[u8]) -> StringInventory {
    let ascii_strings = scan_printable_strings(data);
    let utf16_strings = scan_utf16_strings(data);

    let class_paths = ascii_strings
        .iter()
        .chain(utf16_strings.iter())
        .filter(|value| {
            value.contains("/Game/") || value.contains("/Script/") || value.starts_with("BP_")
        })
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let ids = ascii_strings
        .iter()
        .chain(utf16_strings.iter())
        .filter(|value| {
            value.contains("steam:")
                || value.contains("EOS:")
                || (value.len() == 32 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
                || (value.len() == 17 && value.chars().all(|ch| ch.is_ascii_digit()))
        })
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    StringInventory {
        ascii_strings,
        utf16_strings,
        class_paths,
        ids,
    }
}

fn decode_export_field(replay: &mut BitReader) -> Option<ExportField> {
    let is_exported = replay.read_byte();
    if is_exported == 0 {
        return None;
    }
    let handle = replay.read_int_packed();
    replay.read_u32();
    let name = if replay.header.engine_network_version < 10 {
        replay.read_string()
    } else {
        replay.read_fname_byte()
    };

    Some(ExportField { handle, name })
}

fn read_net_field_exports(replay: &mut BitReader, state: &mut ParseState) {
    let num_layout_cmd_exports = replay.read_int_packed();
    for _ in 0..num_layout_cmd_exports {
        let path_name_index = replay.read_int_packed();
        let is_exported = replay.read_int_packed() == 1;
        let group_path = if is_exported {
            let pathname = replay.read_string();
            let num_exports = replay.read_int_packed();
            state
                .groups_by_path
                .entry(pathname.clone())
                .or_insert_with(|| ExportGroup {
                    path_name: pathname.clone(),
                    net_field_exports_length: num_exports,
                    net_field_exports: HashMap::new(),
                });
            state
                .groups_by_index
                .insert(path_name_index, pathname.clone());
            pathname
        } else {
            state
                .groups_by_index
                .get(&path_name_index)
                .cloned()
                .unwrap_or_default()
        };

        if let Some(export) = decode_export_field(replay) {
            if let Some(group) = state.groups_by_path.get_mut(&group_path) {
                group.net_field_exports.insert(export.handle, export);
            }
        }
    }
}

fn read_net_export_guids(replay: &mut BitReader, state: &mut ParseState) {
    let num_guids = replay.read_int_packed();
    for _ in 0..num_guids {
        let size = replay.read_i32().max(0) as usize;
        if replay.add_offset_byte(2, size).is_err() {
            break;
        }
        let _ = decode_net_guid(replay, true, state, 0);
        let _ = replay.pop_offset(2, true);
    }
}

fn read_export_data(replay: &mut BitReader, state: &mut ParseState) {
    read_net_field_exports(replay, state);
    read_net_export_guids(replay, state);
}

fn read_external_data(replay: &mut BitReader, state: &mut ParseState) {
    loop {
        let external_bits = replay.read_int_packed();
        if external_bits == 0 {
            return;
        }
        let net_guid = replay.read_int_packed();
        let external_bytes = (external_bits / 8) as usize;
        let handle = replay.read_byte();
        let _something1 = replay.read_byte();
        let _something2 = replay.read_byte();
        let payload = replay.read_bytes(external_bytes.saturating_sub(3));

        // Some replicated properties arrive out-of-band and are matched back to
        // the owning net guid once the regular layout decode has identified them.
        state
            .external_data
            .insert(net_guid, ExternalData { handle, payload });
    }
}

fn read_packet_prefix(replay: &mut BitReader) -> Option<(i32, u32)> {
    if replay.has_level_streaming_fixes() {
        let _streaming_fix = replay.read_int_packed();
    }
    let buffer_size = replay.read_i32();
    let state = if buffer_size == 0 {
        1
    } else if buffer_size < 0 {
        2
    } else {
        0
    };
    Some((buffer_size, state))
}

fn conditionally_serialize_quantized_vector(archive: &mut BitReader, default_vector: Vec3) -> Vec3 {
    let was_serialized = archive.read_bit();
    if was_serialized {
        let should_quantize = archive.header.engine_network_version < 13 || archive.read_bit();
        if should_quantize {
            archive.read_packed_vector(10.0, 24)
        } else {
            archive.read_vector3d()
        }
    } else {
        default_vector
    }
}

fn ensure_component_builder(
    state: &mut ParseState,
    component_guid: u32,
    owner_actor_guid: Option<u32>,
    path_hint: Option<String>,
    group_path: &str,
    t_ms: u64,
) {
    let builder = state
        .component_builders
        .entry(component_guid)
        .or_insert_with(|| ComponentBuilder {
            component_guid,
            owner_actor_guid,
            class_name: Some(infer_component_type(group_path, path_hint.as_deref())),
            component_class: Some(canonical_group_leaf(group_path)),
            path_hint: path_hint.clone(),
            group_path: Some(group_path.to_string()),
            first_seen_ms: t_ms,
            notes: Vec::new(),
        });

    if builder.owner_actor_guid.is_none() {
        builder.owner_actor_guid = owner_actor_guid;
    }
    if builder.class_name.is_none() {
        builder.class_name = Some(infer_component_type(group_path, path_hint.as_deref()));
    }
    if builder.component_class.is_none() {
        builder.component_class = Some(canonical_group_leaf(group_path));
    }
    if builder.path_hint.is_none() {
        builder.path_hint = path_hint;
    }
    if builder.group_path.is_none() {
        builder.group_path = Some(group_path.to_string());
    }
}

fn apply_property_event(
    state: &mut ParseState,
    context: PropertyContext<'_>,
    decoded: &DecodedPropertyValue,
) {
    // Keep the raw property event, but also project the pieces we care about into
    // actor, player, component, and movement views for later passes.
    let actor_guid = context.actor.map(|value| value.actor_net_guid.value);
    let is_helicopter_movement_component =
        is_helicopter_movement_component_group(context.group_path);

    if let Some(channel_actor_guid) = actor_guid {
        let builder = state
            .actor_builders
            .entry(channel_actor_guid)
            .or_insert_with(|| ActorBuilder {
                actor_guid: channel_actor_guid,
                channel_index: context.channel_index,
                class_name: Some(context.group_leaf.to_string()),
                archetype_path: context
                    .actor
                    .and_then(|opened| opened.archetype)
                    .and_then(|guid| state.guid_to_path.get(&guid.value))
                    .cloned(),
                open_time_ms: context.t_ms,
                close_time_ms: None,
                initial_location: context.actor.and_then(|opened| opened.location),
                initial_rotation: context.actor.and_then(|opened| opened.rotation),
                team: None,
                build_state: None,
                health: None,
                owner: None,
                notes: Vec::new(),
            });

        if builder.class_name.is_none() {
            builder.class_name = Some(context.group_leaf.to_string());
        }
        if builder.archetype_path.is_none() {
            builder.archetype_path = context
                .actor
                .and_then(|opened| opened.archetype)
                .and_then(|guid| state.guid_to_path.get(&guid.value))
                .cloned();
        }
        match context.property_name {
            "Team" => {
                if let Some(value) = decoded.int_packed {
                    builder.team = Some(value as i64);
                }
            }
            "BuildState" => {
                if let Some(value) = decoded.int32 {
                    builder.build_state = Some(value as i64);
                }
            }
            "Health" => {
                if let Some(value) = decoded.float32 {
                    builder.health = Some(value as f64);
                } else if let Some(value) = decoded.int32 {
                    builder.health = Some(value as f64);
                }
            }
            "Owner" => {
                if let Some(value) = decoded.int_packed {
                    builder.owner = Some(value);
                }
            }
            _ => {}
        }

        if is_deployable_primary_type(context.group_leaf) {
            let deployment = state
                .deployables
                .entry(channel_actor_guid)
                .or_insert_with(|| DeployableBuilder {
                    x: builder.initial_location.map(|value| value.x),
                    y: builder.initial_location.map(|value| value.y),
                    z: builder.initial_location.map(|value| value.z),
                });

            if state
                .deployment_events
                .iter()
                .all(|event| event.actor_guid != Some(channel_actor_guid))
            {
                state.deployment_events.push(DeploymentEvent {
                    t_ms: context.t_ms,
                    second: (context.t_ms / 1000) as u32,
                    actor_guid: Some(channel_actor_guid),
                    deployment_type: classify_deployable_event_type(context.group_leaf).to_string(),
                    class_name: builder
                        .archetype_path
                        .clone()
                        .or_else(|| Some(context.group_leaf.to_string())),
                    x: deployment.x,
                    y: deployment.y,
                    z: deployment.z,
                });
            }
        }
    }

    if context.group_leaf == "SQPlayerState" || context.group_path == "/Script/Squad.SQPlayerState"
    {
        if let Some(player_state_guid) = actor_guid {
            let player = state
                .player_builders
                .entry(player_state_guid)
                .or_insert_with(|| PlayerBuilder {
                    player_state_guid,
                    ..PlayerBuilder::default()
                });

            match context.property_name {
                "PlayerNamePrivate" => {
                    if let Some(value) = &decoded.string {
                        if !value.is_empty() {
                            player.name = Some(value.clone());
                        }
                    }
                }
                "OnlineUserId" => {
                    if let Some(value) = &decoded.string {
                        parse_online_identity(value, player);
                    }
                }
                "UniqueID" => {
                    if let Some(value) = meaningful_decoded_string(decoded) {
                        let parsed = parse_identity_blob(&value);
                        if parsed.steam_id.is_some() || parsed.eos_id.is_some() {
                            if player.identity_raw.is_none() {
                                player.identity_raw = parsed.raw.clone();
                            }
                            if player.steam_id.is_none() {
                                player.steam_id = parsed.steam_id;
                            }
                            if player.eos_id.is_none() {
                                player.eos_id = parsed.eos_id;
                            }
                        }
                    }
                }
                "Soldier" => player.soldier_guid = decoded.int_packed,
                "CurrentPawn" => player.current_pawn_guid = decoded.int_packed,
                "TeamState" => player.team_state_guid = decoded.int_packed,
                "SquadState" => player.squad_state_guid = decoded.int_packed,
                "CurrentRoleId" => {
                    player.current_role_id = decoded.int32;
                    if let Some(value) = meaningful_decoded_string(decoded) {
                        player.current_role_name = Some(value);
                    }
                }
                "DeployRoleId" => {
                    player.deploy_role_id = decoded.int32;
                    if let Some(value) = meaningful_decoded_string(decoded) {
                        player.deploy_role_name = Some(value);
                    }
                }
                "Type" => {
                    if let Some(value) = meaningful_decoded_string(decoded) {
                        player.player_type_name = Some(value);
                    }
                }
                "StartTime" => {
                    if let Some(value) = decoded.float32.filter(|value| value.abs() > 0.001) {
                        player.start_time_ms = Some((value.max(0.0) * 1000.0).round() as u64);
                    } else if let Some(value) = decoded.int32.filter(|value| *value >= 0) {
                        player.start_time_ms = Some(value as u64);
                    }
                }
                _ => {}
            }
        }
    }

    if is_soldier_type(context.group_leaf) && context.property_name == "ReplicatedMovement" {
        if let Some(movement) = &decoded.rep_movement {
            if let Some(location) = movement.location {
                state.raw_player_samples.push(RawSample {
                    t_ms: context.t_ms,
                    actor_guid,
                    player_state_guid: None,
                    key: None,
                    class_name: Some(context.group_leaf.to_string()),
                    x: location.x,
                    y: location.y,
                    z: location.z,
                });
            }
        }
    }

    if is_helicopter_movement_component && context.property_name == "ReplicatedMovement" {
        if let (Some(owner_actor_guid), Some(component_guid), Some(movement)) = (
            actor_guid,
            context.sub_object_net_guid,
            decoded.rep_movement.clone(),
        ) {
            if component_guid != owner_actor_guid && movement.location.is_some() {
                // Helicopter movement components report local-space transforms.
                // We anchor them back onto the owning actor once the full track is known.
                state.raw_helicopter_samples.push(HelicopterMovementSample {
                    t_ms: context.t_ms,
                    actor_guid: owner_actor_guid,
                    payload_bits: decoded.bits,
                    movement,
                });
            }
        }
    }

    if !is_helicopter_movement_component
        && is_vehicle_type(context.group_leaf)
        && context.property_name == "ReplicatedMovement"
    {
        if let Some(movement) = &decoded.rep_movement {
            if let Some(location) = movement.location {
                let key = actor_guid
                    .map(|guid| format!("{}_{guid}", context.group_leaf))
                    .unwrap_or_else(|| context.group_leaf.to_string());
                state.raw_vehicle_samples.push(RawSample {
                    t_ms: context.t_ms,
                    actor_guid,
                    player_state_guid: None,
                    key: Some(key),
                    class_name: Some(context.group_leaf.to_string()),
                    x: location.x,
                    y: location.y,
                    z: location.z,
                });
            }
        }
    }

    if let Some(sub_guid) = context.sub_object_net_guid {
        if Some(sub_guid) != actor_guid {
            let path_hint = state.guid_to_path.get(&sub_guid).cloned();
            ensure_component_builder(
                state,
                sub_guid,
                actor_guid,
                path_hint.clone(),
                context.group_path,
                context.t_ms,
            );

            let component_type = infer_component_type(context.group_path, path_hint.as_deref());
            let retain_component_state = matches!(
                context.property_name,
                "Health" | "bIsEngineActive" | "Owner" | "Occupant" | "PlayerState" | "CurrentSeat"
            ) || (component_type == "seat"
                && matches!(
                    context.property_name,
                    "AttachParent"
                        | "AttachSocketName"
                        | "SeatAttachSocket"
                        | "SoldierAttachSocket"
                        | "SeatPawn"
                        | "SeatedPlayer"
                        | "SeatedSoldier"
                        | "Occupant"
                        | "PlayerState"
                        | "SuppressionMultiplier"
                        | "EnterSeatDuration"
                ));
            if retain_component_state {
                let component_builder = state.component_builders.get(&sub_guid);
                let (value_int, value_float, value_bool, value_string) =
                    normalized_state_values(context.property_name, decoded);
                state.component_state_events.push(ComponentStateEvent {
                    t_ms: context.t_ms,
                    second: (context.t_ms / 1000) as u32,
                    component_guid: Some(sub_guid),
                    owner_actor_guid: actor_guid,
                    component_type,
                    component_name: component_builder.and_then(|builder| builder.path_hint.clone()),
                    component_class: component_builder
                        .and_then(|builder| builder.component_class.clone())
                        .or_else(|| Some(canonical_group_leaf(context.group_path))),
                    group_path: context.group_path.to_string(),
                    property_name: context.property_name.to_string(),
                    decoded: decoded.clone(),
                    value_int,
                    value_float,
                    value_bool,
                    value_string,
                });
            }
        }
    }

    if is_vehicle_type(context.group_leaf)
        && matches!(
            context.property_name,
            "Health" | "bIsEngineActive" | "CurrentGear" | "Throttle" | "Brake"
        )
    {
        let (value_int, value_float, value_bool, value_string) =
            normalized_state_values(context.property_name, decoded);
        state.vehicle_state_events.push(VehicleStateEvent {
            t_ms: context.t_ms,
            second: (context.t_ms / 1000) as u32,
            actor_guid,
            actor_class: state_event_actor_class(state, actor_guid),
            sub_object_net_guid: context.sub_object_net_guid,
            group_path: context.group_path.to_string(),
            property_name: context.property_name.to_string(),
            decoded: decoded.clone(),
            value_int,
            value_float,
            value_bool,
            value_string,
        });
    }

    if matches!(
        context.property_name,
        "bIsFiring" | "ReloadState" | "CurrentAmmo" | "RemainingAmmo"
    ) {
        let (value_int, value_float, value_bool, value_string) =
            normalized_state_values(context.property_name, decoded);
        state.weapon_state_events.push(WeaponStateEvent {
            t_ms: context.t_ms,
            second: (context.t_ms / 1000) as u32,
            actor_guid,
            actor_class: state_event_actor_class(state, actor_guid),
            sub_object_net_guid: context.sub_object_net_guid,
            group_path: context.group_path.to_string(),
            property_name: context.property_name.to_string(),
            decoded: decoded.clone(),
            value_int,
            value_float,
            value_bool,
            value_string,
        });
    }
}

fn record_property_event(
    state: &mut ParseState,
    context: PropertyContext<'_>,
    decoded: DecodedPropertyValue,
) {
    state.property_replications += 1;
    apply_property_event(state, context, &decoded);
    state.property_events.push(PropertyEvent {
        t_ms: context.t_ms,
        second: (context.t_ms / 1000) as u32,
        channel_index: context.channel_index,
        actor_guid: context.actor.map(|value| value.actor_net_guid.value),
        group_path: context.group_path.to_string(),
        property_name: context.property_name.to_string(),
        sub_object_net_guid: context.sub_object_net_guid,
        decoded,
    });
}

fn read_rep_layout_properties(
    archive: &mut BitReader,
    group: &ExportGroup,
    context: ReplicationContext<'_>,
    state: &mut ParseState,
) {
    let group_leaf = infer_group_leaf(context.group_path);
    let actor_guid = context.actor.map(|value| value.actor_net_guid.value);
    archive.skip_bits(1);
    let mut had_property_data = false;
    loop {
        let handle = archive.read_int_packed();
        if handle == 0 {
            break;
        }
        let handle = handle - 1;
        if handle >= group.net_field_exports_length {
            break;
        }
        let num_bits = archive.read_int_packed() as usize;
        if num_bits == 0 {
            continue;
        }
        let Some(export) = group.net_field_exports.get(&handle) else {
            archive.skip_bits(num_bits);
            continue;
        };
        if archive.add_offset(5, num_bits).is_err() {
            break;
        }
        let payload_reader = archive.clone_window();
        had_property_data = true;
        let mut decoded =
            decode_generic_value(&payload_reader, &archive.header, export.name.as_str());
        if export.name == "ReplicatedMovement"
            && is_helicopter_movement_component_group(context.group_path)
        {
            decoded.rep_movement =
                decode_helicopter_component_rep_movement(payload_reader, &archive.header);
        }
        let t_ms = (context.time_seconds.max(0.0) as f64 * 1000.0).round() as u64;
        let _ = archive.pop_offset(5, true);

        record_property_event(
            state,
            PropertyContext {
                actor: context.actor,
                group_path: context.group_path,
                group_leaf,
                property_name: export.name.as_str(),
                t_ms,
                channel_index: context.channel_index,
                sub_object_net_guid: context.sub_object_net_guid,
            },
            decoded,
        );
    }

    if had_property_data {
        if let Some(owner_actor_guid) = actor_guid {
            if let Some(external) = state.external_data.remove(&owner_actor_guid) {
                if let Some(export) = group.net_field_exports.get(&(external.handle as u32)) {
                    let mut payload_reader = BitReader::with_bounds(
                        external.payload.clone(),
                        external.payload.len() * 8,
                    );
                    payload_reader.header = archive.header.clone();
                    payload_reader.outer = archive.outer.clone();

                    let mut decoded = decode_generic_value(
                        &payload_reader,
                        &archive.header,
                        export.name.as_str(),
                    );
                    if export.name == "ReplicatedMovement"
                        && is_helicopter_movement_component_group(context.group_path)
                    {
                        decoded.rep_movement = decode_helicopter_component_rep_movement(
                            payload_reader,
                            &archive.header,
                        );
                    }

                    let t_ms = (context.time_seconds.max(0.0) as f64 * 1000.0).round() as u64;
                    // Re-run side-channel payloads through the same projection path so
                    // derived state stays consistent with inline property replication.
                    record_property_event(
                        state,
                        PropertyContext {
                            actor: context.actor,
                            group_path: context.group_path,
                            group_leaf,
                            property_name: export.name.as_str(),
                            t_ms,
                            channel_index: context.channel_index,
                            sub_object_net_guid: context.sub_object_net_guid,
                        },
                        decoded,
                    );
                }
            }
        }
    }
}

fn guess_stable_subobject_rep_object(
    leaf_path: Option<&str>,
    actor_type: Option<&str>,
) -> Option<String> {
    let leaf = leaf_path.unwrap_or_default().to_ascii_lowercase();
    let actor_type = actor_type.unwrap_or_default().to_ascii_lowercase();

    if leaf.contains("mainrotorcomponent") || leaf.contains("tailrotorcomponent") {
        Some("/Script/Squad.SQRotorComponent".to_string())
    } else if leaf.contains("mainrotor_bladescollision")
        || leaf.contains("tailrotor_bladescollision")
    {
        Some("/Script/Squad.SQRotorBladesComponent".to_string())
    } else if leaf.contains("trackleftcomponent") || leaf.contains("trackrightcomponent") {
        Some("/Script/Squad.SQVehicleTrack".to_string())
    } else if leaf.starts_with("wheel_") || leaf.contains("wheelcomponent") {
        Some("/Script/Squad.SQVehicleWheel".to_string())
    } else if leaf.contains("ammorackcomponent") {
        Some("/Script/Squad.SQVehicleAmmoBox".to_string())
    } else if leaf == "movementcomponentmanager" {
        Some("/Script/Squad.SQMovementComponentManager".to_string())
    } else if leaf == "movementcomponent" {
        if is_helicopter_type(&actor_type) {
            Some("/Script/Squad.SQHelicopterMovementComponent".to_string())
        } else if actor_type.contains("m60")
            || actor_type.contains("m1a1")
            || actor_type.contains("t72")
            || actor_type.contains("t62")
            || actor_type.contains("bmp")
            || actor_type.contains("tracked")
            || actor_type.contains("tank")
        {
            Some("/Script/Squad.SQTrackedVehicleMovementComponent".to_string())
        } else {
            Some("/Script/Squad.SQWheeledVehicleMovementComponent".to_string())
        }
    } else {
        None
    }
}

fn read_content_block_header(
    bunch_archive: &mut BitReader,
    actor: Option<&OpenedActor>,
    state: &mut ParseState,
) -> (bool, bool, Option<String>, Option<u32>) {
    let mut object_deleted = false;
    let out_has_rep_layout = bunch_archive.read_bit();
    let is_actor = bunch_archive.read_bit();

    if is_actor {
        let rep_object = actor
            .and_then(|opened| opened.archetype)
            .map(|guid| guid.value.to_string())
            .or_else(|| actor.map(|opened| opened.actor_net_guid.value.to_string()));
        let sub_object = actor.map(|opened| opened.actor_net_guid.value);
        return (object_deleted, out_has_rep_layout, rep_object, sub_object);
    }

    let net_guid = decode_net_guid(bunch_archive, false, state, 0);
    let stably_named = bunch_archive.read_bit();
    if stably_named {
        let leaf_path = state.guid_to_path.get(&net_guid.value).cloned();
        let actor_path = actor
            .and_then(|opened| opened.archetype)
            .and_then(|guid| state.guid_to_path.get(&guid.value))
            .cloned();
        let guessed =
            guess_stable_subobject_rep_object(leaf_path.as_deref(), actor_path.as_deref());
        let rep_object = guessed.or_else(|| Some(net_guid.value.to_string()));
        return (
            object_deleted,
            out_has_rep_layout,
            rep_object,
            Some(net_guid.value),
        );
    }

    let mut delete_sub_object = false;
    let mut serialize_class = true;

    if bunch_archive.header.engine_network_version >= 30 {
        let is_destroy_message = bunch_archive.read_bit();
        if is_destroy_message {
            delete_sub_object = true;
            serialize_class = false;
            bunch_archive.skip_bits(8);
        }
    }

    let mut class_net_guid = None;
    if serialize_class {
        class_net_guid = Some(decode_net_guid(bunch_archive, false, state, 0));
        delete_sub_object = !class_net_guid.unwrap_or_default().is_valid();
    }

    if delete_sub_object {
        object_deleted = true;
        return (
            object_deleted,
            out_has_rep_layout,
            Some(net_guid.value.to_string()),
            Some(net_guid.value),
        );
    }

    if bunch_archive.header.engine_network_version >= 18 {
        let actor_is_outer = bunch_archive.read_bit();
        if !actor_is_outer {
            let _outer = decode_net_guid(bunch_archive, false, state, 0);
        }
    }

    (
        object_deleted,
        out_has_rep_layout,
        class_net_guid.map(|guid| guid.value.to_string()),
        Some(net_guid.value),
    )
}

fn process_bunch(bunch: &mut Bunch, state: &mut ParseState) {
    let needs_actor_open =
        matches!(state.channels.get(&bunch.ch_index), Some(channel) if channel.actor.is_none());

    if needs_actor_open {
        if !bunch.b_open {
            return;
        }

        let mut actor = OpenedActor {
            actor_net_guid: decode_net_guid(&mut bunch.archive, false, state, 0),
            ..OpenedActor::default()
        };

        if !bunch.archive.at_end() && actor.actor_net_guid.is_dynamic() {
            actor.archetype = Some(decode_net_guid(&mut bunch.archive, false, state, 0));
            if bunch.archive.header.engine_network_version >= 5 {
                actor.level = Some(decode_net_guid(&mut bunch.archive, false, state, 0));
            }
            actor.location = Some(conditionally_serialize_quantized_vector(
                &mut bunch.archive,
                Vec3::default(),
            ));
            actor.rotation = Some(if bunch.archive.read_bit() {
                bunch.archive.read_rotation_short()
            } else {
                Rotator::default()
            });
            actor.scale = Some(conditionally_serialize_quantized_vector(
                &mut bunch.archive,
                Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            ));
            actor.velocity = Some(conditionally_serialize_quantized_vector(
                &mut bunch.archive,
                Vec3::default(),
            ));
        }

        let actor_guid = actor.actor_net_guid.value;
        let archetype_path = actor
            .archetype
            .and_then(|guid| state.guid_to_path.get(&guid.value))
            .cloned();

        let t_ms = (bunch.time_seconds.max(0.0) as f64 * 1000.0).round() as u64;
        state.actor_builders.insert(
            actor_guid,
            ActorBuilder {
                actor_guid,
                channel_index: bunch.ch_index,
                class_name: archetype_path
                    .as_deref()
                    .and_then(normalize_type)
                    .or_else(|| Some("Unknown".to_string())),
                archetype_path,
                open_time_ms: t_ms,
                close_time_ms: None,
                initial_location: actor.location,
                initial_rotation: actor.rotation,
                team: None,
                build_state: None,
                health: None,
                owner: None,
                notes: Vec::new(),
            },
        );
        state.actor_opens += 1;
        state.actor_to_channel.insert(actor_guid, bunch.ch_index);
        state.channel_to_actor.insert(bunch.ch_index, actor_guid);

        if let Some(channel) = state.channels.get_mut(&bunch.ch_index) {
            channel.actor = Some(actor);
        }
    }

    let actor = state
        .channels
        .get(&bunch.ch_index)
        .and_then(|channel| channel.actor.clone());

    while !bunch.archive.at_end() {
        let (object_deleted, out_has_rep_layout, rep_object, sub_object_net_guid) =
            read_content_block_header(&mut bunch.archive, actor.as_ref(), state);

        if object_deleted {
            continue;
        }

        let num_payload_bits = bunch.archive.read_int_packed() as usize;
        if num_payload_bits == 0 {
            continue;
        }

        let _ = bunch.archive.add_offset(4, num_payload_bits);
        let group = resolve_rep_group(
            state,
            actor.as_ref(),
            rep_object.as_deref(),
            sub_object_net_guid,
        );

        if let Some(group) = group {
            if out_has_rep_layout {
                read_rep_layout_properties(
                    &mut bunch.archive,
                    &group,
                    ReplicationContext {
                        actor: actor.as_ref(),
                        channel_index: bunch.ch_index,
                        group_path: &group.path_name,
                        time_seconds: bunch.time_seconds,
                        sub_object_net_guid,
                    },
                    state,
                );
            }
        } else {
            bunch.archive.skip_bits(num_payload_bits);
        }

        let _ = bunch.archive.pop_offset(4, true);
    }
}

#[derive(Debug, Clone)]
struct Bunch {
    time_seconds: f32,
    packet_id: u32,
    b_open: bool,
    b_close: bool,
    b_dormant: bool,
    close_reason: u32,
    b_is_replication_paused: bool,
    b_reliable: bool,
    ch_index: u32,
    b_has_package_export_maps: bool,
    b_has_must_be_mapped_guids: bool,
    b_partial: bool,
    b_partial_initial: bool,
    b_partial_final: bool,
    ch_sequence: u32,
    archive: BitReader,
}

fn received_sequence_bunch(bunch: &mut Bunch, state: &mut ParseState) {
    process_bunch(bunch, state);

    if bunch.b_close {
        if let Some(actor_guid) = state.channel_to_actor.get(&bunch.ch_index).copied() {
            if let Some(builder) = state.actor_builders.get_mut(&actor_guid) {
                builder.close_time_ms =
                    Some((bunch.time_seconds.max(0.0) as f64 * 1000.0).round() as u64);
            }
        }
        state.ignored_channels.remove(&bunch.ch_index);
        if let Some(actor_guid) = state.channel_to_actor.remove(&bunch.ch_index) {
            state.actor_to_channel.remove(&actor_guid);
        }
        state.channels.remove(&bunch.ch_index);
    }
}

fn received_next_bunch(mut bunch: Bunch, state: &mut ParseState) {
    if bunch.b_reliable {
        state.in_reliable = bunch.ch_sequence;
    }

    if bunch.b_partial {
        if bunch.b_partial_initial {
            state.partial_bunch = Some(PartialBunch {
                archive: bunch.archive.clone_window(),
                packet_id: bunch.packet_id,
                ch_index: bunch.ch_index,
                ch_sequence: bunch.ch_sequence,
                b_open: bunch.b_open,
                b_reliable: bunch.b_reliable,
                b_has_package_export_maps: bunch.b_has_package_export_maps,
                b_has_must_be_mapped_guids: bunch.b_has_must_be_mapped_guids,
                time_seconds: bunch.time_seconds,
            });
            return;
        }

        if let Some(partial) = state.partial_bunch.as_mut() {
            let reliable_matches = bunch.ch_sequence == partial.ch_sequence + 1;
            let unreliable_matches = reliable_matches || bunch.ch_sequence == partial.ch_sequence;
            let sequence_matches = if partial.b_reliable {
                reliable_matches
            } else {
                unreliable_matches
            };

            if sequence_matches && partial.b_reliable == bunch.b_reliable {
                let bits_left = bunch.archive.get_bits_left();
                if !bunch.b_has_package_export_maps && bits_left > 0 {
                    let payload = bunch.archive.read_bits(bits_left);
                    partial
                        .archive
                        .append_data_from_checked(&payload, bits_left);
                }
                partial.ch_sequence = bunch.ch_sequence;
                if bunch.b_partial_final {
                    let mut merged = Bunch {
                        time_seconds: partial.time_seconds,
                        packet_id: partial.packet_id,
                        b_open: partial.b_open,
                        b_close: bunch.b_close,
                        b_dormant: bunch.b_dormant,
                        close_reason: bunch.close_reason,
                        b_is_replication_paused: bunch.b_is_replication_paused,
                        b_reliable: partial.b_reliable,
                        ch_index: partial.ch_index,
                        b_has_package_export_maps: partial.b_has_package_export_maps,
                        b_has_must_be_mapped_guids: partial.b_has_must_be_mapped_guids,
                        b_partial: false,
                        b_partial_initial: false,
                        b_partial_final: true,
                        ch_sequence: partial.ch_sequence,
                        archive: partial.archive.clone_window(),
                    };
                    received_sequence_bunch(&mut merged, state);
                    state.partial_bunch = None;
                }
            }
            return;
        }
    }

    received_sequence_bunch(&mut bunch, state);
}

fn received_packet(packet_archive: &mut BitReader, time_seconds: f32, state: &mut ParseState) {
    state.in_packet_id += 1;

    while !packet_archive.at_end() {
        if packet_archive.header.engine_network_version < 8 {
            packet_archive.skip_bits(1);
        }

        let b_control = packet_archive.read_bit();
        let b_open = if b_control {
            packet_archive.read_bit()
        } else {
            false
        };
        let b_close = if b_control {
            packet_archive.read_bit()
        } else {
            false
        };

        let (b_dormant, close_reason) = if packet_archive.header.engine_network_version < 7 {
            let dormant = if b_close {
                packet_archive.read_bit()
            } else {
                false
            };
            (dormant, if dormant { 1 } else { 0 })
        } else {
            let reason = if b_close {
                packet_archive.read_serialized_int(15)
            } else {
                0
            };
            (reason == 1, reason)
        };

        let b_is_replication_paused = packet_archive.read_bit();
        let b_reliable = packet_archive.read_bit();
        let ch_index = if packet_archive.header.engine_network_version < 3 {
            packet_archive.read_serialized_int(u32::MAX)
        } else {
            packet_archive.read_int_packed()
        };
        let b_has_package_export_maps = packet_archive.read_bit();
        let b_has_must_be_mapped_guids = packet_archive.read_bit();
        let b_partial = packet_archive.read_bit();

        let ch_sequence = if b_reliable {
            state.in_reliable + 1
        } else if b_partial {
            state.in_packet_id
        } else {
            0
        };

        let b_partial_initial = if b_partial {
            packet_archive.read_bit()
        } else {
            false
        };
        let b_partial_final = if b_partial {
            packet_archive.read_bit()
        } else {
            false
        };

        let _ch_name =
            if packet_archive.header.engine_network_version >= 6 && (b_reliable || b_open) {
                packet_archive.skip_fname();
                "Actor".to_string()
            } else {
                "Actor".to_string()
            };

        let bunch_data_bits = packet_archive.read_serialized_int(1024 * 2 * 8) as usize;
        let ignore_channel = state
            .ignored_channels
            .get(&ch_index)
            .copied()
            .unwrap_or(false);

        let archive = if ignore_channel {
            packet_archive.skip_bits(bunch_data_bits);
            None
        } else if b_partial {
            Some(BitReader::with_bounds(
                packet_archive.read_bits(bunch_data_bits),
                bunch_data_bits,
            ))
        } else {
            let _ = packet_archive.add_offset(3, bunch_data_bits);
            Some(packet_archive.clone_window())
        };

        let mut archive = match archive {
            Some(mut value) => {
                value.header = packet_archive.header.clone();
                value.outer = packet_archive.outer.clone();
                value
            }
            None => {
                continue;
            }
        };

        if b_has_package_export_maps {
            let b_has_rep_layout_export = archive.read_bit();
            if !b_has_rep_layout_export {
                let num_guids_in_bunch = archive.read_i32().max(0) as usize;
                if num_guids_in_bunch <= 2048 {
                    for _ in 0..num_guids_in_bunch {
                        let _ = decode_net_guid(&mut archive, true, state, 0);
                    }
                }
            }
        }

        if b_reliable && ch_sequence <= state.in_reliable {
            let _ = packet_archive.pop_offset(3, true);
            continue;
        }

        state.channels.entry(ch_index).or_default();

        let bunch = Bunch {
            time_seconds,
            packet_id: state.in_packet_id,
            b_open,
            b_close,
            b_dormant,
            close_reason,
            b_is_replication_paused,
            b_reliable,
            ch_index,
            b_has_package_export_maps,
            b_has_must_be_mapped_guids,
            b_partial,
            b_partial_initial,
            b_partial_final,
            ch_sequence,
            archive,
        };

        received_next_bunch(bunch, state);
        let _ = packet_archive.pop_offset(3, true);
    }
}

fn received_raw_packet(
    packet_size: usize,
    replay: &mut BitReader,
    time_seconds: f32,
    state: &mut ParseState,
) {
    let Some(mut last_byte) = replay.get_last_byte() else {
        return;
    };

    let mut bit_size = packet_size.saturating_mul(8).saturating_sub(1);
    while (last_byte & 0x80) < 1 && bit_size > 0 {
        last_byte = last_byte.wrapping_shl(1);
        bit_size = bit_size.saturating_sub(1);
    }

    if replay.add_offset(2, bit_size).is_err() {
        return;
    }
    received_packet(replay, time_seconds, state);
    let _ = replay.pop_offset(2, true);
}

fn parse_playback_frame(replay: &mut BitReader, state: &mut ParseState) {
    if replay.header.network_version >= 6 {
        let _current_level_index = replay.read_i32();
    }

    let time_seconds = replay.read_f32();
    if (state.last_frame_time - time_seconds).abs() > f32::EPSILON {
        state.last_frame_time = time_seconds;
        state.frames_processed += 1;
    }

    if replay.header.network_version >= 10 {
        read_export_data(replay, state);
    }

    if replay.has_level_streaming_fixes() {
        let num_streaming_levels = replay.read_int_packed() as usize;
        for _ in 0..num_streaming_levels {
            let _ = replay.read_string();
        }
        replay.skip_bytes(8);
    }

    read_external_data(replay, state);

    if !replay.has_level_streaming_fixes() {
        replay.skip_bytes(1);
    }

    if replay.has_game_specific_frame_data() {
        let skip_external_offset = replay.read_u64() as usize;
        if skip_external_offset > 0 {
            replay.skip_bytes(skip_external_offset);
        }
    }

    loop {
        let Some((packet_size, packet_state)) = read_packet_prefix(replay) else {
            break;
        };
        let packet_size = packet_size.max(0) as usize;
        let _ = replay.add_offset_byte(1, packet_size);
        if packet_state == 0 {
            received_raw_packet(packet_size, replay, time_seconds, state);
            state.packets_processed += 1;
        } else {
            let _ = replay.pop_offset(1, true);
            break;
        }
        let _ = replay.pop_offset(1, true);
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, Default)]
struct HelicopterTrackReconstruction {
    accepted_samples: Vec<TrackSample3>,
}

fn helicopter_track_prefix(class_name: Option<&str>, archetype_path: Option<&str>) -> &'static str {
    let hint = class_name
        .or(archetype_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if hint.contains("loach") {
        "LOACH"
    } else if hint.contains("uh1y") || hint.contains("uh-1y") {
        "UH1Y"
    } else if hint.contains("uh1h") || hint.contains("uh-1h") {
        "UH1H"
    } else if hint.contains("uh60") || hint.contains("uh-60") || hint.contains("blackhawk") {
        "UH60"
    } else if hint.contains("ch146")
        || hint.contains("ch-146")
        || hint.contains("griffon")
        || hint.contains("raven")
    {
        "CH146"
    } else if hint.contains("ch178")
        || hint.contains("ch-178")
        || hint.contains("mi17")
        || hint.contains("mi-17")
    {
        "MI17"
    } else if hint.contains("mi8") || hint.contains("mi-8") {
        "MI8"
    } else if hint.contains("mrh90") || hint.contains("mrh-90") {
        "MRH90"
    } else if hint.contains("sa330") || hint.contains("sa-330") || hint.contains("puma") {
        "SA330"
    } else if hint.contains("z8") || hint.contains("z-8") {
        "Z8"
    } else if hint.contains("z9") || hint.contains("z-9") {
        "Z9"
    } else {
        "HELICOPTER"
    }
}

fn helicopter_track_key(
    actor_guid: u32,
    class_name: Option<&str>,
    archetype_path: Option<&str>,
) -> String {
    format!(
        "{}_{}",
        helicopter_track_prefix(class_name, archetype_path),
        actor_guid
    )
}

fn reconstruct_anchored_helicopter_track(
    actor: &ActorBuilder,
    samples: &[HelicopterMovementSample],
) -> Option<HelicopterTrackReconstruction> {
    let anchor = actor.initial_location?;
    if samples.is_empty() {
        return None;
    }

    let mut ordered = samples.to_vec();
    ordered.sort_by_key(|sample| sample.t_ms);

    let primary_count = ordered
        .iter()
        .filter(|sample| sample.payload_bits == HELO_PRIMARY_PAYLOAD_BITS)
        .count();
    let non_primary_count = ordered.len().saturating_sub(primary_count);
    let keep_non_primary = !(primary_count > 0 && non_primary_count <= 2);

    ordered.retain(|sample| {
        let Some(local) = sample.movement.location else {
            return false;
        };
        sample.payload_bits == HELO_PRIMARY_PAYLOAD_BITS
            || (keep_non_primary
                && local.x.abs().max(local.y.abs()).max(local.z.abs())
                    >= HELO_NON_PRIMARY_LOCAL_MIN_ABS)
    });
    if ordered.is_empty() {
        return None;
    }

    let first_local = ordered.iter().find_map(|sample| sample.movement.location)?;

    let mut accepted_samples = Vec::new();
    let mut last_world: Option<(u64, f64, f64, f64)> = None;

    // Treat the actor open transform as the world-space anchor and offset later
    // local samples relative to the first decoded movement payload.
    for sample in ordered {
        let Some(local) = sample.movement.location else {
            continue;
        };

        let world_x = anchor.x + local.x - first_local.x;
        let world_y = anchor.y + local.y - first_local.y;
        let world_z = anchor.z + local.z - first_local.z;

        if let Some((last_t_ms, last_x, last_y, last_z)) = last_world {
            let dt_ms = sample.t_ms.saturating_sub(last_t_ms);
            if dt_ms > 0 {
                let dt_seconds = dt_ms as f64 / 1000.0;
                let dx = world_x - last_x;
                let dy = world_y - last_y;
                let dz = world_z - last_z;
                let speed = (dx * dx + dy * dy + dz * dz).sqrt() / dt_seconds;
                if speed > HELO_SPEED_THRESHOLD
                    || dz.abs() > HELO_Z_JUMP_THRESHOLD
                    || world_x.abs() > HELO_WORLD_BOUND
                    || world_y.abs() > HELO_WORLD_BOUND
                    || world_z.abs() > HELO_Z_BOUND
                {
                    continue;
                }
            }
        }

        accepted_samples.push(TrackSample3 {
            t_ms: sample.t_ms,
            x: round2(world_x),
            y: round2(world_y),
            z: round2(world_z),
        });
        last_world = Some((sample.t_ms, world_x, world_y, world_z));
    }

    if accepted_samples.is_empty() {
        return None;
    }

    accepted_samples.insert(
        0,
        TrackSample3 {
            t_ms: actor.open_time_ms,
            x: round2(anchor.x),
            y: round2(anchor.y),
            z: round2(anchor.z),
        },
    );

    Some(HelicopterTrackReconstruction { accepted_samples })
}

fn finalize_tracks(state: &mut ParseState) -> TrackGroups {
    let mut player_state_to_name = HashMap::new();
    let mut actor_to_player_state = HashMap::new();

    for player in state.player_builders.values() {
        if let Some(name) = &player.name {
            player_state_to_name.insert(player.player_state_guid, name.clone());
        }
        if let Some(guid) = player.soldier_guid {
            actor_to_player_state.insert(guid, player.player_state_guid);
        }
        if let Some(guid) = player.current_pawn_guid {
            actor_to_player_state.insert(guid, player.player_state_guid);
        }
    }

    let mut player_tracks_map: HashMap<String, Vec<TrackSample3>> = HashMap::new();
    for sample in &state.raw_player_samples {
        let player_state_guid = sample.player_state_guid.or_else(|| {
            sample
                .actor_guid
                .and_then(|guid| actor_to_player_state.get(&guid).copied())
        });
        let Some(player_state_guid) = player_state_guid else {
            continue;
        };
        let Some(name) = player_state_to_name.get(&player_state_guid) else {
            continue;
        };
        player_tracks_map
            .entry(name.clone())
            .or_default()
            .push(TrackSample3 {
                t_ms: sample.t_ms,
                x: round2(sample.x),
                y: round2(sample.y),
                z: round2(sample.z),
            });
    }

    let mut vehicle_tracks_map: HashMap<String, VehicleTrackEntry> = HashMap::new();
    for sample in &state.raw_vehicle_samples {
        let key = sample.key.clone().unwrap_or_else(|| "vehicle".to_string());
        let entry = vehicle_tracks_map
            .entry(key.clone())
            .or_insert_with(|| (sample.actor_guid, sample.class_name.clone(), Vec::new()));
        entry.2.push(TrackSample3 {
            t_ms: sample.t_ms,
            x: round2(sample.x),
            y: round2(sample.y),
            z: round2(sample.z),
        });
    }

    let mut players = player_tracks_map
        .into_iter()
        .map(|(key, mut samples)| {
            samples.sort_by_key(|sample| sample.t_ms);
            Track3 {
                key,
                actor_guid: None,
                player_state_guid: None,
                class_name: Some("Player".to_string()),
                source: "replicated_movement".to_string(),
                samples,
            }
        })
        .collect::<Vec<_>>();

    let mut helicopter_component_samples: HashMap<u32, Vec<HelicopterMovementSample>> =
        HashMap::new();
    for sample in &state.raw_helicopter_samples {
        helicopter_component_samples
            .entry(sample.actor_guid)
            .or_default()
            .push(sample.clone());
    }

    let mut direct_helicopter_actor_guids = HashSet::new();
    let mut vehicles = Vec::new();
    let mut helicopters = Vec::new();

    for actor in state.actor_builders.values() {
        let type_name = actor
            .class_name
            .as_deref()
            .or(actor.archetype_path.as_deref())
            .unwrap_or_default();
        if !is_vehicle_type(type_name) || !is_helicopter_type(type_name) {
            continue;
        }

        let Some(samples) = helicopter_component_samples.get(&actor.actor_guid) else {
            continue;
        };
        let Some(reconstruction) = reconstruct_anchored_helicopter_track(actor, samples) else {
            continue;
        };

        direct_helicopter_actor_guids.insert(actor.actor_guid);
        helicopters.push(Track3 {
            key: helicopter_track_key(
                actor.actor_guid,
                actor.class_name.as_deref(),
                actor.archetype_path.as_deref(),
            ),
            actor_guid: Some(actor.actor_guid),
            player_state_guid: None,
            class_name: actor.class_name.clone(),
            source: "movement_component_anchored".to_string(),
            samples: reconstruction.accepted_samples,
        });
    }

    let mut fallback_helicopter_actor_guids = HashSet::new();
    for (key, (actor_guid, class_name, mut samples)) in vehicle_tracks_map {
        samples.sort_by_key(|sample| sample.t_ms);
        let is_helicopter = class_name
            .as_deref()
            .map(is_helicopter_type)
            .unwrap_or(false);
        if is_helicopter
            && actor_guid.is_some_and(|guid| direct_helicopter_actor_guids.contains(&guid))
        {
            continue;
        }
        let track = Track3 {
            key: if is_helicopter {
                actor_guid
                    .map(|guid| helicopter_track_key(guid, class_name.as_deref(), None))
                    .unwrap_or(key.clone())
            } else {
                key.clone()
            },
            actor_guid,
            player_state_guid: None,
            class_name: class_name.clone(),
            source: if is_helicopter {
                "helicopter_rep_movement".to_string()
            } else {
                "vehicle_rep_movement".to_string()
            },
            samples,
        };
        if is_helicopter {
            if let Some(guid) = track.actor_guid {
                fallback_helicopter_actor_guids.insert(guid);
            }
            helicopters.push(track);
        } else {
            vehicles.push(track);
        }
    }

    for actor in state.actor_builders.values() {
        let type_name = actor
            .class_name
            .as_deref()
            .or(actor.archetype_path.as_deref())
            .unwrap_or_default();
        if !is_vehicle_type(type_name) || !is_helicopter_type(type_name) {
            continue;
        }
        if direct_helicopter_actor_guids.contains(&actor.actor_guid)
            || fallback_helicopter_actor_guids.contains(&actor.actor_guid)
        {
            continue;
        }
        state.warnings.push(format!(
            "no helicopter track samples recovered for actor {} ({})",
            actor.actor_guid, type_name
        ));
    }

    players.sort_by(|a, b| a.key.cmp(&b.key));
    vehicles.sort_by(|a, b| a.key.cmp(&b.key));
    helicopters.sort_by(|a, b| a.key.cmp(&b.key));

    TrackGroups {
        players,
        vehicles,
        helicopters,
    }
}

fn finalize_roster_and_seats(
    state: &ParseState,
    players: &mut [Player],
) -> (Vec<Team>, Vec<Squad>, Vec<SeatChangeEvent>) {
    #[derive(Debug, Clone, Default)]
    struct TeamTemp {
        id: Option<u32>,
        team_state_guid: Option<u32>,
        name: Option<String>,
        faction_from_state: Option<String>,
        faction_setup_id: Option<String>,
        tickets: Option<u32>,
        commander_state_guid: Option<u32>,
    }

    #[derive(Debug, Clone, Default)]
    struct SquadTemp {
        id: Option<u32>,
        raw_team_id: Option<u32>,
        squad_state_guid: Option<u32>,
        leader_player_state_guid: Option<u32>,
        creator_player_state_guid: Option<u32>,
        name: Option<String>,
        leader_name: Option<String>,
        leader_steam_id: Option<String>,
        leader_eos_id: Option<String>,
        creator_name: Option<String>,
        creator_identity_raw: Option<String>,
        creator_steam_id: Option<String>,
        creator_eos_id: Option<String>,
    }

    #[derive(Debug, Clone, Default)]
    struct SeatMeta {
        vehicle_actor_guid: Option<u32>,
        vehicle_class: Option<String>,
        seat_attach_socket: Option<String>,
        attach_socket_name: Option<String>,
    }

    let mut team_faction_counts: BTreeMap<u32, HashMap<String, usize>> = BTreeMap::new();
    for actor in state.actor_builders.values() {
        let Some(team_id) = actor
            .team
            .and_then(|value| (value >= 0).then_some(value as u32))
        else {
            continue;
        };
        for source in [actor.class_name.as_deref(), actor.archetype_path.as_deref()] {
            if let Some(token) = asset_faction_token(source) {
                *team_faction_counts
                    .entry(team_id)
                    .or_default()
                    .entry(token)
                    .or_insert(0) += 1;
            }
        }
    }

    let mut faction_hint_by_team_id: BTreeMap<u32, String> = BTreeMap::new();
    for (team_id, counts) in &team_faction_counts {
        if let Some((hint, _)) = counts
            .iter()
            .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        {
            faction_hint_by_team_id.insert(*team_id, hint.clone());
        }
    }

    let mut teams_by_actor_guid: BTreeMap<u32, TeamTemp> = BTreeMap::new();
    let mut private_to_public_squad_guid: HashMap<u32, u32> = HashMap::new();

    for event in &state.property_events {
        let Some(actor_guid) = event.actor_guid else {
            continue;
        };
        let group_leaf = canonical_group_leaf(&event.group_path);

        if group_leaf == "SQSquadStatePrivateToTeam" && event.property_name == "SquadState" {
            if let Some(public_guid) = decoded_scalar_u32(&event.decoded) {
                private_to_public_squad_guid.insert(actor_guid, public_guid);
            }
        }

        if group_leaf == "SQTeamState" || group_leaf == "SQTeamStatePrivate" {
            let team = teams_by_actor_guid.entry(actor_guid).or_default();
            team.team_state_guid.get_or_insert(actor_guid);

            match event.property_name.as_str() {
                "ID" => team.id = decoded_preferred_u32(&event.decoded),
                "Tickets" => team.tickets = decoded_preferred_u32(&event.decoded),
                "CommanderState" => team.commander_state_guid = decoded_scalar_u32(&event.decoded),
                "Name" | "ShortName" | "DisplayName" => {
                    if let Some(value) = meaningful_decoded_string(&event.decoded) {
                        team.name = Some(value);
                    }
                }
                "FactionSetupId" => {
                    if let Some(value) = decoded_scalar_string(&event.decoded) {
                        team.faction_setup_id = Some(value.clone());
                        team.faction_from_state = Some(value);
                    }
                }
                "Faction" | "FactionId" | "FactionName" => {
                    if let Some(value) = decoded_scalar_string(&event.decoded) {
                        team.faction_from_state = Some(value);
                    }
                }
                _ => {}
            }
        }
    }

    let mut known_team_ids: HashSet<u32> = faction_hint_by_team_id.keys().copied().collect();
    for team in teams_by_actor_guid.values() {
        if let Some(team_id) = team.id {
            known_team_ids.insert(team_id);
        }
    }

    let mut squads_by_state_guid: BTreeMap<u32, SquadTemp> = BTreeMap::new();
    for player in players.iter() {
        if let Some(squad_state_guid) = player.squad_state_guid {
            squads_by_state_guid
                .entry(squad_state_guid)
                .or_insert_with(|| SquadTemp {
                    squad_state_guid: Some(squad_state_guid),
                    ..SquadTemp::default()
                });
        }
    }

    for event in &state.property_events {
        let Some(actor_guid) = event.actor_guid else {
            continue;
        };
        let group_leaf = canonical_group_leaf(&event.group_path);
        if group_leaf != "SQSquadState" && group_leaf != "SQSquadStatePrivateToTeam" {
            continue;
        }

        let squad_state_guid = private_to_public_squad_guid
            .get(&actor_guid)
            .copied()
            .or_else(|| (group_leaf == "SQSquadState").then_some(actor_guid))
            .or_else(|| {
                (event.property_name == "SquadState").then(|| decoded_scalar_u32(&event.decoded))?
            });
        let Some(squad_state_guid) = squad_state_guid else {
            continue;
        };

        let squad = squads_by_state_guid
            .entry(squad_state_guid)
            .or_insert_with(|| SquadTemp {
                squad_state_guid: Some(squad_state_guid),
                ..SquadTemp::default()
            });

        match event.property_name.as_str() {
            "ID" => squad.id = decoded_preferred_u32(&event.decoded),
            "Team" | "TeamId" | "TeamState" => {
                squad.raw_team_id = decoded_preferred_u32(&event.decoded);
            }
            "Name" | "SquadName" | "DisplayName" => {
                if let Some(value) = decoded_scalar_string(&event.decoded) {
                    squad.name = Some(value);
                }
            }
            "Leader" | "LeaderState" | "LeaderPlayerState" => {
                squad.leader_player_state_guid = decoded_scalar_u32(&event.decoded);
            }
            "Creator" | "CreatorPlayerState" => {
                squad.creator_player_state_guid = decoded_scalar_u32(&event.decoded);
            }
            "LeaderName" => {
                if let Some(value) = meaningful_decoded_string(&event.decoded) {
                    squad.leader_name = Some(value);
                }
            }
            "CreatorName" | "SquadCreatorName" => {
                if let Some(value) = meaningful_decoded_string(&event.decoded) {
                    squad.creator_name = Some(value);
                }
            }
            "SquadCreatorSteamID" => {
                if let Some(raw) = meaningful_decoded_string(&event.decoded) {
                    squad.creator_identity_raw = Some(raw.clone());
                    let parsed = parse_identity_blob(&raw);
                    if squad.creator_steam_id.is_none() {
                        squad.creator_steam_id = parsed.steam_id;
                    }
                    if squad.creator_eos_id.is_none() {
                        squad.creator_eos_id = parsed.eos_id;
                    }
                }
            }
            "CreatorSteamId" => {
                if let Some(value) = meaningful_decoded_string(&event.decoded) {
                    squad.creator_steam_id = Some(value);
                }
            }
            "CreatorEOSId" | "CreatorEosId" | "CreatorEpicId" => {
                if let Some(value) = meaningful_decoded_string(&event.decoded) {
                    squad.creator_eos_id = Some(value);
                }
            }
            _ => {}
        }
    }

    let player_index: HashMap<u32, usize> = players
        .iter()
        .enumerate()
        .map(|(index, player)| (player.player_state_guid, index))
        .collect();

    for squad in squads_by_state_guid.values_mut() {
        if squad.leader_name.is_none() {
            if let Some(player) = squad
                .leader_player_state_guid
                .and_then(|guid| player_index.get(&guid).copied())
                .and_then(|index| players.get(index))
            {
                squad.leader_name = player.name.clone();
                if squad.leader_steam_id.is_none() {
                    squad.leader_steam_id = player.steam_id.clone();
                }
                if squad.leader_eos_id.is_none() {
                    squad.leader_eos_id = player.eos_id.clone();
                }
            }
        }

        if let Some(player) = squad
            .leader_player_state_guid
            .and_then(|guid| player_index.get(&guid).copied())
            .and_then(|index| players.get(index))
        {
            if squad.leader_steam_id.is_none() {
                squad.leader_steam_id = player.steam_id.clone();
            }
            if squad.leader_eos_id.is_none() {
                squad.leader_eos_id = player.eos_id.clone();
            }
        }

        if squad.creator_name.is_none() {
            if let Some(player) = squad
                .creator_player_state_guid
                .and_then(|guid| player_index.get(&guid).copied())
                .and_then(|index| players.get(index))
            {
                squad.creator_name = player.name.clone();
                if squad.creator_steam_id.is_none() {
                    squad.creator_steam_id = player.steam_id.clone();
                }
                if squad.creator_eos_id.is_none() {
                    squad.creator_eos_id = player.eos_id.clone();
                }
            }
        }

        if squad.leader_name.is_some()
            && squad.creator_name.is_some()
            && squad.leader_name == squad.creator_name
        {
            if squad.leader_steam_id.is_none() {
                squad.leader_steam_id = squad.creator_steam_id.clone();
            }
            if squad.leader_eos_id.is_none() {
                squad.leader_eos_id = squad.creator_eos_id.clone();
            }
        }
    }

    let mut teams_by_id: BTreeMap<u32, Team> = BTreeMap::new();
    for (team_id, faction_hint) in &faction_hint_by_team_id {
        teams_by_id.insert(
            *team_id,
            Team {
                id: *team_id,
                name: None,
                faction: Some(faction_hint.clone()),
                faction_setup_id: None,
                tickets: None,
                commander_state_guid: None,
                team_state_guid: None,
                notes: vec!["faction inferred from actor asset/class hints".to_string()],
            },
        );
    }

    for team in teams_by_actor_guid.values() {
        let Some(team_id) = team.id else {
            continue;
        };
        let entry = teams_by_id.entry(team_id).or_insert_with(|| Team {
            id: team_id,
            name: None,
            faction: None,
            faction_setup_id: None,
            tickets: None,
            commander_state_guid: None,
            team_state_guid: team.team_state_guid,
            notes: Vec::new(),
        });
        if entry.team_state_guid.is_none() {
            entry.team_state_guid = team.team_state_guid;
        }
        if entry.name.is_none() {
            entry.name = team.name.clone();
        }
        if entry.faction.is_none() {
            entry.faction = team.faction_from_state.clone();
            if entry.faction.is_some() {
                entry
                    .notes
                    .push("faction sourced from team-state textual scalar".to_string());
            }
        }
        if entry.faction_setup_id.is_none() {
            entry.faction_setup_id = team.faction_setup_id.clone();
        }
        if entry.tickets.is_none() {
            entry.tickets = team.tickets;
        }
        if entry.commander_state_guid.is_none() {
            entry.commander_state_guid = team.commander_state_guid;
        }
    }

    let mut squads = squads_by_state_guid
        .into_values()
        .map(|temp| {
            let raw_team_id = temp.raw_team_id;
            let team_id = raw_team_id.map(|value| normalize_team_id(value, &known_team_ids));
            if let Some(team_id) = team_id {
                teams_by_id.entry(team_id).or_insert_with(|| Team {
                    id: team_id,
                    name: None,
                    faction: faction_hint_by_team_id.get(&team_id).cloned(),
                    faction_setup_id: None,
                    tickets: None,
                    commander_state_guid: None,
                    team_state_guid: None,
                    notes: if faction_hint_by_team_id.contains_key(&team_id) {
                        vec!["faction inferred from actor asset/class hints".to_string()]
                    } else {
                        Vec::new()
                    },
                });
            }
            let faction = team_id.and_then(|value| {
                teams_by_id
                    .get(&value)
                    .and_then(|team| team.faction.clone())
            });
            Squad {
                id: temp
                    .id
                    .unwrap_or_else(|| temp.squad_state_guid.unwrap_or_default()),
                raw_team_id,
                team_id,
                faction,
                squad_state_guid: temp.squad_state_guid,
                name: temp.name,
                leader_player_state_guid: temp.leader_player_state_guid,
                leader_name: temp.leader_name,
                leader_steam_id: temp.leader_steam_id,
                leader_eos_id: temp.leader_eos_id,
                creator_name: temp.creator_name,
                creator_identity_raw: temp.creator_identity_raw,
                creator_steam_id: temp.creator_steam_id,
                creator_eos_id: temp.creator_eos_id,
                notes: match (raw_team_id, team_id) {
                    (Some(raw), Some(normalized)) if raw != normalized => vec![format!(
                        "team_id normalized from raw team id {raw} to {normalized}"
                    )],
                    _ => Vec::new(),
                },
            }
        })
        .collect::<Vec<_>>();

    let squad_index_by_state_guid: HashMap<u32, usize> = squads
        .iter()
        .enumerate()
        .filter_map(|(index, squad)| squad.squad_state_guid.map(|guid| (guid, index)))
        .collect();

    for player in players.iter_mut() {
        let Some(squad_index) = player
            .squad_state_guid
            .and_then(|guid| squad_index_by_state_guid.get(&guid).copied())
        else {
            continue;
        };
        let squad = &squads[squad_index];
        let is_leader = squad.leader_name.is_some()
            && player.name.is_some()
            && player.name == squad.leader_name;
        let is_creator = squad.creator_name.is_some()
            && player.name.is_some()
            && player.name == squad.creator_name;
        player.team_id = squad.team_id;
        player.faction = squad.faction.clone();
        player.squad_id = Some(squad.id);
        player.squad_leader_name = squad.leader_name.clone();
        player.squad_creator_name = squad.creator_name.clone();
        player.squad_creator_steam_id = squad.creator_steam_id.clone();
        player.squad_creator_eos_id = squad.creator_eos_id.clone();
        if is_leader || is_creator {
            if player.steam_id.is_none() {
                player.steam_id = squad.creator_steam_id.clone();
                if player.steam_id.is_some() {
                    player
                        .notes
                        .push("steam_id backfilled from squad creator identity".to_string());
                }
            }
            if player.eos_id.is_none() {
                player.eos_id = squad.creator_eos_id.clone();
                if player.eos_id.is_some() {
                    player
                        .notes
                        .push("eos_id backfilled from squad creator identity".to_string());
                }
            }
            if player.identity_raw.is_none() {
                player.identity_raw = squad.creator_identity_raw.clone();
                if player.identity_raw.is_some() {
                    player
                        .notes
                        .push("identity_raw backfilled from squad creator identity".to_string());
                }
            }
        }
    }

    for player in players.iter_mut() {
        if player.faction.is_none() {
            for source in [
                player.current_role_name.as_deref(),
                player.deploy_role_name.as_deref(),
                player.player_type_name.as_deref(),
                player.name.as_deref(),
            ] {
                if let Some(token) = asset_faction_token(source) {
                    player.faction = Some(token);
                    player
                        .notes
                        .push("faction inferred from asset/role/name hints".to_string());
                    break;
                }
            }
        }
    }

    let mut seen_seat_keys: HashSet<SeenSeatKey> = HashSet::new();
    let mut seat_changes: Vec<SeatChangeEvent> = Vec::new();
    let player_name_by_state = players
        .iter()
        .filter_map(|player| {
            player
                .name
                .as_ref()
                .map(|name| (player.player_state_guid, name.clone()))
        })
        .collect::<HashMap<_, _>>();
    let current_seat_guids = state
        .property_events
        .iter()
        .filter(|event| {
            canonical_group_leaf(&event.group_path) == "SQPlayerState"
                && event.property_name == "CurrentSeat"
        })
        .filter_map(|event| decoded_scalar_u32(&event.decoded).filter(|guid| *guid != 0))
        .collect::<HashSet<_>>();
    let mut seat_meta_by_guid: HashMap<u32, SeatMeta> = HashMap::new();
    for event in &state.property_events {
        if let Some(sub_guid) = event.sub_object_net_guid {
            if Some(sub_guid) != event.actor_guid {
                let path_hint = state.guid_to_path.get(&sub_guid).map(String::as_str);
                let is_seat_component = infer_component_type(&event.group_path, path_hint)
                    == "seat"
                    || current_seat_guids.contains(&sub_guid);
                if is_seat_component {
                    let seat = seat_meta_by_guid.entry(sub_guid).or_default();
                    seat.vehicle_actor_guid = event.actor_guid;
                    if seat.vehicle_class.is_none() {
                        seat.vehicle_class = event.actor_guid.and_then(|actor_guid| {
                            state.actor_builders.get(&actor_guid).and_then(|builder| {
                                builder.class_name.clone().or_else(|| {
                                    builder.archetype_path.as_deref().and_then(normalize_type)
                                })
                            })
                        });
                    }
                    match event.property_name.as_str() {
                        "AttachSocketName" => {
                            if let Some(value) = meaningful_decoded_string(&event.decoded) {
                                seat.attach_socket_name = Some(value);
                            }
                        }
                        "SeatAttachSocket" => {
                            if let Some(value) = meaningful_decoded_string(&event.decoded) {
                                seat.seat_attach_socket = Some(value);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if canonical_group_leaf(&event.group_path) != "SQPlayerState"
            || event.property_name != "CurrentSeat"
        {
            continue;
        }

        let player_state_guid = event.actor_guid.filter(|guid| *guid != 0);
        let component_guid = decoded_scalar_u32(&event.decoded).filter(|guid| *guid != 0);
        let seat = component_guid.and_then(|guid| seat_meta_by_guid.get(&guid).cloned());
        let dedup_key = (
            event.t_ms,
            seat.as_ref().and_then(|value| value.vehicle_actor_guid),
            component_guid,
            player_state_guid,
            event.property_name.clone(),
        );
        if seen_seat_keys.insert(dedup_key) {
            seat_changes.push(SeatChangeEvent {
                t_ms: event.t_ms,
                second: event.second,
                actor_guid: seat.as_ref().and_then(|value| value.vehicle_actor_guid),
                component_guid,
                player_state_guid,
                vehicle_class: seat.as_ref().and_then(|value| value.vehicle_class.clone()),
                seat_attach_socket: seat.as_ref().and_then(|value| {
                    value
                        .seat_attach_socket
                        .clone()
                        .or_else(|| value.attach_socket_name.clone())
                }),
                attach_socket_name: seat
                    .as_ref()
                    .and_then(|value| value.attach_socket_name.clone()),
                occupant_name: player_state_guid
                    .and_then(|guid| player_name_by_state.get(&guid).cloned()),
                value: Some(
                    component_guid
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
            });
        }
    }

    let mut teams = teams_by_id.into_values().collect::<Vec<_>>();
    teams.sort_by_key(|team| team.id);
    squads.sort_by_key(|squad| (squad.team_id.unwrap_or(u32::MAX), squad.id));
    seat_changes.sort_by_key(|event| (event.t_ms, event.component_guid.unwrap_or_default()));

    (teams, squads, seat_changes)
}

fn finalize_kills(state: &ParseState, players: &[Player]) -> Vec<KillEvent> {
    #[derive(Debug, Clone, Default)]
    struct DeathState {
        dead: Option<bool>,
        incap: Option<bool>,
        health: Option<f64>,
    }

    let mut player_name_by_state: HashMap<u32, String> = HashMap::new();
    let mut actor_to_player_state: HashMap<u32, u32> = HashMap::new();
    for player in players {
        if let Some(name) = &player.name {
            player_name_by_state.insert(player.player_state_guid, name.clone());
        }
        if let Some(guid) = player.soldier_guid {
            actor_to_player_state.insert(guid, player.player_state_guid);
        }
        if let Some(guid) = player.current_pawn_guid {
            actor_to_player_state.insert(guid, player.player_state_guid);
        }
    }

    let mut events: Vec<KillEvent> = Vec::new();
    let mut states: HashMap<u32, DeathState> = HashMap::new();
    let mut dedup: HashSet<(u64, u32, bool)> = HashSet::new();
    for event in &state.property_events {
        let leaf = infer_group_leaf(&event.group_path);
        let Some(raw_actor_guid) = event.actor_guid else {
            continue;
        };
        let player_state_guid =
            if leaf == "SQPlayerState" || event.group_path == "/Script/Squad.SQPlayerState" {
                raw_actor_guid
            } else if let Some(mapped) = actor_to_player_state.get(&raw_actor_guid).copied() {
                mapped
            } else {
                continue;
            };

        let mut emit = |was_incap: bool| {
            let key = (event.t_ms / 1000, player_state_guid, was_incap);
            if dedup.insert(key) {
                events.push(KillEvent {
                    t_ms: event.t_ms,
                    second: event.second,
                    victim_name: player_name_by_state.get(&player_state_guid).cloned(),
                    killer_name: None,
                    victim_guid: Some(player_state_guid),
                    killer_guid: None,
                    was_incap: Some(was_incap),
                });
            }
        };

        let death_state = states.entry(player_state_guid).or_default();
        match event.property_name.as_str() {
            "bIsIncapacitated" | "bIsUnconscious" | "bWounded" => {
                if event.decoded.boolean == Some(true) && death_state.incap != Some(true) {
                    emit(true);
                }
                death_state.incap = event.decoded.boolean;
            }
            "bIsDead" | "bIsKilled" => {
                if event.decoded.boolean == Some(true) && death_state.dead != Some(true) {
                    emit(false);
                }
                death_state.dead = event.decoded.boolean;
            }
            "Health" | "CurrentHealth" => {
                let next_health = event
                    .decoded
                    .float32
                    .map(|value| value as f64)
                    .or_else(|| event.decoded.int32.map(|value| value as f64))
                    .or_else(|| event.decoded.int_packed.map(|value| value as f64));
                if let Some(next_health) = next_health {
                    if death_state.health.unwrap_or(1.0) > 0.0 && next_health <= 0.0 {
                        emit(false);
                    }
                    death_state.health = Some(next_health);
                }
            }
            "LifeState" => {
                let next_state = event
                    .decoded
                    .int32
                    .map(|value| value as i64)
                    .or_else(|| event.decoded.int_packed.map(|value| value as i64));
                if let Some(next_state) = next_state {
                    if next_state >= 2 {
                        emit(false);
                    } else if next_state == 1 {
                        emit(true);
                    }
                }
            }
            _ => {}
        }
    }

    events.sort_by_key(|event| (event.t_ms, event.victim_guid.unwrap_or_default()));
    events
}

pub(crate) fn parse_file(path: impl AsRef<Path>) -> Result<Bundle> {
    let path = path.as_ref();
    let data = Arc::new(fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?);
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown.replay".to_string());
    parse_data(data, file_name)
}

pub(crate) fn parse_bytes(bytes: &[u8], file_name: Option<String>) -> Result<Bundle> {
    parse_data(
        Arc::new(bytes.to_vec()),
        file_name.unwrap_or_else(|| "unknown.replay".to_string()),
    )
}

fn parse_data(data: Arc<Vec<u8>>, file_name: String) -> Result<Bundle> {
    let (outer, header, replay_data_chunks) = parse_wrapper(data.as_ref().as_slice())?;

    if outer.is_encrypted {
        return Err(Error::Unsupported(
            "encrypted replays are not supported in this pure-Rust parser".to_string(),
        ));
    }

    if outer.is_compressed {
        return Err(Error::Unsupported(
            "compressed replays are not supported in this pure-Rust parser".to_string(),
        ));
    }

    let mut state = ParseState::default();
    let mut string_inventory = build_string_inventory(data.as_ref().as_slice());

    for chunk in &replay_data_chunks {
        // Replay data chunks are bounded byte ranges inside the wrapper. Each chunk
        // gets its own reader and contributes to the same parser state.
        let mut replay = BitReader::new(Arc::clone(&data));
        replay.header = header.clone();
        replay.outer = outer.clone();
        replay.go_to_byte(chunk.start_pos);
        let _ = replay.add_offset_byte(1, chunk.length as usize);

        while !replay.at_end() {
            parse_playback_frame(&mut replay, &mut state);
        }

        let _ = replay.pop_offset(1, true);
    }

    let tracks = finalize_tracks(&mut state);

    let mut players = state
        .player_builders
        .values()
        .cloned()
        .map(|builder| Player {
            player_state_guid: builder.player_state_guid,
            name: builder.name,
            steam_id: builder.steam_id,
            eos_id: builder.eos_id,
            online_user_id: builder.online_user_id,
            identity_raw: builder.identity_raw,
            soldier_guid: builder.soldier_guid,
            current_pawn_guid: builder.current_pawn_guid,
            team_id: None,
            faction: None,
            team_state_guid: builder.team_state_guid,
            squad_id: None,
            squad_state_guid: builder.squad_state_guid,
            current_role_id: builder.current_role_id,
            current_role_name: builder.current_role_name,
            deploy_role_id: builder.deploy_role_id,
            deploy_role_name: builder.deploy_role_name,
            player_type_name: builder.player_type_name,
            squad_leader_name: None,
            squad_creator_name: None,
            squad_creator_steam_id: None,
            squad_creator_eos_id: None,
            start_time_ms: builder.start_time_ms,
            notes: builder.notes,
        })
        .collect::<Vec<_>>();
    players.sort_by(|a, b| a.name.cmp(&b.name));

    let mut actor_entities = state
        .actor_builders
        .values()
        .map(|builder| ActorEntity {
            actor_guid: builder.actor_guid,
            channel_index: builder.channel_index,
            class_name: builder.class_name.clone(),
            archetype_path: builder.archetype_path.clone(),
            open_time_ms: builder.open_time_ms,
            close_time_ms: builder.close_time_ms,
            initial_location: builder.initial_location,
            initial_rotation: builder.initial_rotation,
            team: builder.team,
            build_state: builder.build_state,
            health: builder.health,
            owner: builder.owner,
            notes: builder.notes.clone(),
        })
        .collect::<Vec<_>>();
    actor_entities.sort_by_key(|actor| actor.open_time_ms);

    let mut vehicles = Vec::new();
    let mut helicopters = Vec::new();
    let mut deployables = Vec::new();

    for actor in actor_entities {
        let type_name = actor
            .class_name
            .as_deref()
            .or(actor.archetype_path.as_deref())
            .unwrap_or_default()
            .to_string();

        if is_deployable_primary_type(&type_name) {
            deployables.push(actor);
        } else if is_vehicle_type(&type_name) && is_helicopter_type(&type_name) {
            helicopters.push(actor);
        } else if is_vehicle_type(&type_name) {
            vehicles.push(actor);
        }
    }

    let mut components = state
        .component_builders
        .values()
        .map(|builder| ComponentEntity {
            component_guid: builder.component_guid,
            owner_actor_guid: builder.owner_actor_guid,
            class_name: builder.class_name.clone(),
            component_class: builder.component_class.clone(),
            path_hint: builder.path_hint.clone(),
            group_path: builder.group_path.clone(),
            first_seen_ms: builder.first_seen_ms,
            notes: builder.notes.clone(),
        })
        .collect::<Vec<_>>();
    components.sort_by_key(|value| value.first_seen_ms);

    let map_name = header.level_names_and_times.keys().next().cloned();

    let kills = finalize_kills(&state, &players);
    let (teams, squads, seat_changes) = finalize_roster_and_seats(&state, &mut players);

    string_inventory.class_paths.sort();
    string_inventory.ids.sort();

    let bundle = Bundle {
        schema: SchemaInfo::default(),
        replay: ReplayInfoSection {
            source: ReplaySourceInfo {
                file_name,
                size_bytes: data.len() as u64,
                sha256: sha256_hex(data.as_ref().as_slice()),
            },
            engine: ReplayEngineInfo {
                engine_version: Some(format!(
                    "{}.{}",
                    header.engine_network_version, header.network_version
                )),
                net_version: Some(header.network_version),
                notes: Vec::new(),
            },
            map_name,
            squad_version: Some(header.branch.clone()),
            duration_ms: outer.length_in_ms as u64,
            started_at: None,
            notes: vec![
                "Canonical bundle produced directly from a single replay ingest.".to_string(),
                "Compatibility JSON is derived from this canonical representation.".to_string(),
            ],
        },
        teams,
        squads,
        players,
        actors: ActorGroups {
            vehicles,
            helicopters,
            deployables,
            components,
        },
        tracks,
        events: EventGroups {
            kills,
            deployments: state.deployment_events,
            seat_changes,
            component_states: state.component_state_events,
            vehicle_states: state.vehicle_state_events,
            weapon_states: state.weapon_state_events,
            properties: state.property_events,
        },
        diagnostics: Diagnostics {
            frames_processed: state.frames_processed,
            packets_processed: state.packets_processed,
            actor_opens: state.actor_opens,
            export_groups_discovered: state.groups_by_path.len(),
            guid_to_path_size: state.guid_to_path.len(),
            property_replications: state.property_replications,
            position_samples: state.raw_player_samples.len() as u64,
            vehicle_position_samples: state.raw_vehicle_samples.len() as u64,
            replay_data_chunks: replay_data_chunks.len(),
            warnings: state.warnings,
            string_inventory,
            provenance_report: canonical_provenance_report(),
        },
    };

    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaned_text_rejects_replacement_and_control_chars() {
        assert_eq!(
            cleaned_text(" Insidious Fiddler "),
            Some("Insidious Fiddler".to_string())
        );
        assert_eq!(cleaned_text("�"), None);
        assert_eq!(cleaned_text("bad\u{0000}value"), None);
    }

    #[test]
    fn parses_identity_blob() {
        let parsed =
            parse_identity_blob("EOS: 00028d9ce5804bd193376b5a9b482ad2 steam: 76561199047801300");
        assert_eq!(
            parsed.eos_id.as_deref(),
            Some("00028d9ce5804bd193376b5a9b482ad2")
        );
        assert_eq!(parsed.steam_id.as_deref(), Some("76561199047801300"));
    }

    #[test]
    fn asset_faction_token_matches_current_and_legacy_codes() {
        assert_eq!(
            asset_faction_token(Some("TLF_SLPilot_01")).as_deref(),
            Some("TLF")
        );
        assert_eq!(
            asset_faction_token(Some("USA_Pilot_01")).as_deref(),
            Some("USA")
        );
        assert_eq!(
            asset_faction_token(Some("PLANMC_Rifleman_01")).as_deref(),
            Some("PLANMC")
        );
        assert_eq!(
            asset_faction_token(Some("PLAAGF_Rifleman_01")).as_deref(),
            Some("PLAAGF")
        );
        assert_eq!(
            asset_faction_token(Some("AFU_Pilot_01")).as_deref(),
            Some("AFU")
        );
        assert_eq!(
            asset_faction_token(Some("CRF_Scout_01")).as_deref(),
            Some("CRF")
        );
        assert_eq!(
            asset_faction_token(Some("/Game/Vehicles/Loach_WPMC/BP_Loach.BP_Loach_C")).as_deref(),
            Some("WPMC")
        );
        assert_eq!(
            asset_faction_token(Some("INS_Rifleman_01")).as_deref(),
            Some("MEI")
        );
        assert_eq!(
            asset_faction_token(Some("MEA_Pilot_01")).as_deref(),
            Some("GFI")
        );
        assert_eq!(asset_faction_token(Some("Role_Pilot")).as_deref(), None);
    }

    #[test]
    fn canonical_group_candidates_cover_default_state_names() {
        let candidates = canonical_script_group_candidates("Default__SQPlayerState");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate == "/Script/Squad.SQPlayerState")
        );
    }

    #[test]
    fn resolve_rep_group_prefers_explicit_helicopter_movement_component_hint() {
        let mut state = ParseState::default();
        let actor_path = "/Game/Vehicles/Loach_WPMC/BP_Loach.BP_Loach_C".to_string();
        let movement_group = "/Script/Squad.SQHelicopterMovementComponent".to_string();
        state.groups_by_path.insert(
            actor_path.clone(),
            ExportGroup {
                path_name: actor_path.clone(),
                ..ExportGroup::default()
            },
        );
        state.groups_by_path.insert(
            movement_group.clone(),
            ExportGroup {
                path_name: movement_group.clone(),
                ..ExportGroup::default()
            },
        );
        state.guid_to_path.insert(966, actor_path.clone());
        state
            .guid_to_path
            .insert(1586, "MovementComponent".to_string());

        let actor = OpenedActor {
            actor_net_guid: NetworkGuid { value: 966 },
            archetype: Some(NetworkGuid { value: 966 }),
            ..OpenedActor::default()
        };

        let resolved = resolve_rep_group(
            &state,
            Some(&actor),
            Some("/Script/Squad.SQHelicopterMovementComponent"),
            Some(1586),
        )
        .expect("movement component rep group should resolve");

        assert_eq!(resolved.path_name, movement_group);
    }

    #[test]
    fn stable_movement_component_guess_covers_current_helicopter_families() {
        for actor_type in [
            "BP_CH146_Utility_C",
            "BP_CH178_Transport_C",
            "BP_Mi8MTV5_C",
            "BP_MRH90_C",
            "BP_SA330_C",
            "BP_UH60M_C",
            "BP_Z8G_C",
            "BP_Z9A_C",
        ] {
            assert_eq!(
                guess_stable_subobject_rep_object(Some("MovementComponent"), Some(actor_type))
                    .as_deref(),
                Some("/Script/Squad.SQHelicopterMovementComponent"),
                "{actor_type} should resolve as a helicopter movement component"
            );
        }
    }

    #[test]
    fn anchored_helicopter_track_seeds_open_transform_and_rejects_outlier_jump() {
        let actor = ActorBuilder {
            actor_guid: 754,
            open_time_ms: 16,
            initial_location: Some(Vec3 {
                x: 100.0,
                y: 200.0,
                z: 300.0,
            }),
            class_name: Some("BP_Loach_CAS_Small_C".to_string()),
            ..ActorBuilder::default()
        };

        let make_sample = |t_ms, x, y, z| HelicopterMovementSample {
            t_ms,
            actor_guid: 754,
            payload_bits: HELO_PRIMARY_PAYLOAD_BITS,
            movement: RepMovement {
                location: Some(Vec3 { x, y, z }),
                ..RepMovement::default()
            },
        };

        let reconstruction = reconstruct_anchored_helicopter_track(
            &actor,
            &[
                make_sample(16, 10.0, 20.0, 30.0),
                make_sample(1016, 15.0, 25.0, 35.0),
                make_sample(2016, 500_000.0, 25.0, 35.0),
            ],
        )
        .expect("anchored reconstruction should accept the plausible samples");

        assert_eq!(reconstruction.accepted_samples.len(), 3);
        assert_eq!(reconstruction.accepted_samples[0].t_ms, 16);
        assert_eq!(reconstruction.accepted_samples[0].x, 100.0);
        assert_eq!(reconstruction.accepted_samples[1].x, 100.0);
        assert_eq!(reconstruction.accepted_samples[2].x, 105.0);
        assert_eq!(reconstruction.accepted_samples[2].y, 205.0);
        assert_eq!(reconstruction.accepted_samples[2].z, 305.0);
    }

    #[test]
    fn helicopter_track_prefix_covers_current_families() {
        assert_eq!(
            helicopter_track_prefix(Some("BP_CH146_Utility_C"), None),
            "CH146"
        );
        assert_eq!(
            helicopter_track_prefix(Some("BP_CH178_Transport_C"), None),
            "MI17"
        );
        assert_eq!(helicopter_track_prefix(Some("BP_Mi8MTV5_C"), None), "MI8");
        assert_eq!(helicopter_track_prefix(Some("BP_MRH90_C"), None), "MRH90");
        assert_eq!(helicopter_track_prefix(Some("BP_SA330_C"), None), "SA330");
        assert_eq!(helicopter_track_prefix(Some("BP_UH60M_C"), None), "UH60");
        assert_eq!(helicopter_track_prefix(Some("BP_Z8G_C"), None), "Z8");
        assert_eq!(helicopter_track_prefix(Some("BP_Z9A_C"), None), "Z9");
    }

    fn decode_test_hex(input: &str) -> Vec<u8> {
        fn nybble(value: u8) -> u8 {
            match value {
                b'0'..=b'9' => value - b'0',
                b'a'..=b'f' => value - b'a' + 10,
                b'A'..=b'F' => value - b'A' + 10,
                _ => panic!("invalid hex input"),
            }
        }

        input
            .as_bytes()
            .chunks_exact(2)
            .map(|chunk| (nybble(chunk[0]) << 4) | nybble(chunk[1]))
            .collect()
    }

    #[test]
    fn primary_helicopter_rep_movement_decodes_known_loach_payload() {
        let payload = decode_test_hex("d51a48473c3683f97f104198fa8f8abdea08");
        let header = DemoHeader {
            engine_network_version: 36,
            network_version: 19,
            ..DemoHeader::default()
        };

        let movement =
            decode_helicopter_component_rep_movement(BitReader::with_bounds(payload, 141), &header)
                .expect("known Loach payload should decode");

        let location = movement.location.expect("location should decode");
        assert_eq!(location.x, 1864.0);
        assert_eq!(location.y, -3614.0);
        assert_eq!(location.z, 205.0);

        let rotation = movement.rotation.expect("rotation should decode");
        assert_eq!(rotation.pitch, 350.15625);
        assert_eq!(rotation.yaw, 88.59375);
        assert_eq!(rotation.roll, 0.0);

        let linear_velocity = movement
            .linear_velocity
            .expect("linear velocity should decode");
        assert_eq!(linear_velocity.x, 0.0);
        assert_eq!(linear_velocity.y, 0.2);
        assert_eq!(linear_velocity.z, -0.4);
        assert!(movement.rep_physics);
    }

    #[test]
    fn state_value_projection_prefers_typed_float_and_bool_fields() {
        let health = DecodedPropertyValue {
            int32: Some(1137180672),
            float32: Some(400.0),
            ..DecodedPropertyValue::default()
        };
        let (value_int, value_float, value_bool, value_string) =
            normalized_state_values("Health", &health);
        assert_eq!(value_int, None);
        assert_eq!(value_float, Some(400.0));
        assert_eq!(value_bool, None);
        assert_eq!(value_string, None);

        let engine_active = DecodedPropertyValue {
            int32: Some(1920),
            boolean: Some(true),
            ..DecodedPropertyValue::default()
        };
        let (value_int, value_float, value_bool, value_string) =
            normalized_state_values("bIsEngineActive", &engine_active);
        assert_eq!(value_int, None);
        assert_eq!(value_float, None);
        assert_eq!(value_bool, Some(true));
        assert_eq!(value_string, None);

        let packed_health = DecodedPropertyValue {
            int_packed: Some(46),
            ..DecodedPropertyValue::default()
        };
        let (value_int, value_float, value_bool, value_string) =
            normalized_state_values("Health", &packed_health);
        assert_eq!(value_int, None);
        assert_eq!(value_float, Some(46.0));
        assert_eq!(value_bool, None);
        assert_eq!(value_string, None);
    }

    #[test]
    fn decoded_scalar_string_falls_back_to_int32() {
        let decoded = DecodedPropertyValue {
            int32: Some(2),
            ..DecodedPropertyValue::default()
        };
        assert_eq!(decoded_scalar_string(&decoded).as_deref(), Some("2"));
    }

    #[test]
    fn normalizes_team_id_with_raw_minus_one_fallback() {
        let known = HashSet::from([0_u32, 1_u32]);
        assert_eq!(normalize_team_id(2, &known), 1);
        assert_eq!(normalize_team_id(1, &known), 1);
    }

    #[test]
    fn attempts_text_decode_for_role_and_socket_fields() {
        assert!(should_attempt_string_decode("CurrentRoleId"));
        assert!(should_attempt_string_decode("DeployRoleId"));
        assert!(should_attempt_string_decode("Type"));
        assert!(should_attempt_string_decode("SeatAttachSocket"));
    }

    #[test]
    #[ignore = "fixture replay regression; run explicitly when validating parser output"]
    fn sample_replay_roster_regression_if_fixture_present() {
        let Some(fixture_dir) = std::env::var_os("SQUADREPLAY_TEST_FIXTURE_DIR") else {
            return;
        };
        let fixture =
            Path::new(&fixture_dir).join("rtb-jensens-range-wpmc-vs-turkey-20260407.replay");
        if !fixture.exists() {
            return;
        }

        let bundle = parse_file(&fixture).expect("fixture replay should parse");
        assert_eq!(bundle.schema.version, 1);
        assert_eq!(bundle.teams.len(), 2);
        assert_eq!(bundle.squads.len(), 1);
        assert_eq!(bundle.players.len(), 2);

        let factions = bundle
            .teams
            .iter()
            .map(|team| team.faction.clone().unwrap_or_default())
            .collect::<HashSet<_>>();
        assert!(factions.contains("WPMC"));
        assert!(factions.contains("TLF"));

        let team = bundle
            .teams
            .iter()
            .find(|team| team.id == 0)
            .expect("expected team 0");
        assert_eq!(team.faction.as_deref(), Some("WPMC"));
        assert_eq!(team.faction_setup_id.as_deref(), Some("CIV_Motorized"));
        assert_eq!(team.tickets, Some(150));
        assert_eq!(team.commander_state_guid, Some(10));
        assert_eq!(team.team_state_guid, Some(18));

        let squad = bundle
            .squads
            .iter()
            .find(|squad| squad.squad_state_guid == Some(3730))
            .expect("expected squad state 3730");
        assert_eq!(squad.id, 1);
        assert_eq!(squad.raw_team_id, Some(2));
        assert_eq!(squad.team_id, Some(1));
        assert_eq!(squad.faction.as_deref(), Some("TLF"));
        assert_eq!(squad.name.as_deref(), Some("2"));
        assert_eq!(squad.leader_player_state_guid, Some(26));
        assert_eq!(squad.leader_name.as_deref(), Some("Insidious Fiddler"));
        assert_eq!(squad.leader_steam_id.as_deref(), Some("76561199047801300"));
        assert_eq!(
            squad.leader_eos_id.as_deref(),
            Some("00028d9ce5804bd193376b5a9b482ad2")
        );
        assert_eq!(squad.creator_name.as_deref(), Some("Insidious Fiddler"));
        assert_eq!(
            squad.creator_identity_raw.as_deref(),
            Some("EOS: 00028d9ce5804bd193376b5a9b482ad2 steam: 76561199047801300")
        );
        assert_eq!(squad.creator_steam_id.as_deref(), Some("76561199047801300"));
        assert_eq!(
            squad.creator_eos_id.as_deref(),
            Some("00028d9ce5804bd193376b5a9b482ad2")
        );

        let player = bundle
            .players
            .iter()
            .find(|player| player.player_state_guid == 26)
            .expect("expected player state 26");
        assert_eq!(player.name.as_deref(), Some("Insidious Fiddler"));
        assert_eq!(
            player.online_user_id.as_deref(),
            Some("81be8f2a-bbc2-421d-b2df-dabe9503dbf3")
        );
        assert_eq!(player.steam_id.as_deref(), Some("76561199047801300"));
        assert_eq!(
            player.eos_id.as_deref(),
            Some("00028d9ce5804bd193376b5a9b482ad2")
        );
        assert_eq!(
            player.identity_raw.as_deref(),
            Some("EOS: 00028d9ce5804bd193376b5a9b482ad2 steam: 76561199047801300")
        );
        assert_eq!(player.team_id, Some(1));
        assert_eq!(player.faction.as_deref(), Some("TLF"));
        assert_eq!(player.squad_state_guid, Some(3730));
        assert_eq!(player.squad_id, Some(1));
        assert_eq!(player.current_role_name.as_deref(), Some("TLF_SLPilot_01"));
        assert_eq!(player.deploy_role_name.as_deref(), Some("TLF_SLPilot_01"));
        assert_eq!(player.player_type_name.as_deref(), Some("Role_Pilot"));
        assert_eq!(
            player.squad_leader_name.as_deref(),
            Some("Insidious Fiddler")
        );
        assert_eq!(
            player.squad_creator_name.as_deref(),
            Some("Insidious Fiddler")
        );
        assert_eq!(
            player.squad_creator_steam_id.as_deref(),
            Some("76561199047801300")
        );
        assert_eq!(
            player.squad_creator_eos_id.as_deref(),
            Some("00028d9ce5804bd193376b5a9b482ad2")
        );

        assert!(
            bundle
                .events
                .component_states
                .iter()
                .any(|event| event.component_type == "seat")
        );
        assert!(bundle.actors.components.iter().any(|component| {
            component.owner_actor_guid == Some(754)
                && component.component_guid == 3334
                && component.path_hint.as_deref() == Some("MainRotorComponent")
                && component.component_class.as_deref() == Some("SQRotorComponent")
                && component.group_path.as_deref() == Some("/Script/Squad.SQRotorComponent")
        }));
        let main_rotor_open = bundle
            .events
            .component_states
            .iter()
            .find(|event| {
                event.owner_actor_guid == Some(754)
                    && event.component_guid == Some(3334)
                    && event.property_name == "Health"
                    && event.t_ms == 16
            })
            .expect("expected main rotor health open event");
        assert_eq!(
            main_rotor_open.component_name.as_deref(),
            Some("MainRotorComponent")
        );
        assert_eq!(
            main_rotor_open.component_class.as_deref(),
            Some("SQRotorComponent")
        );
        assert_eq!(main_rotor_open.group_path, "/Script/Squad.SQRotorComponent");
        assert_eq!(main_rotor_open.value_float, Some(400.0));
        assert_eq!(main_rotor_open.value_int, None);

        let tail_rotor_failure = bundle
            .events
            .component_states
            .iter()
            .find(|event| {
                event.owner_actor_guid == Some(754)
                    && event.component_guid == Some(3338)
                    && event.property_name == "Health"
                    && event.t_ms == 152653
            })
            .expect("expected tail rotor failure event");
        assert_eq!(
            tail_rotor_failure.component_name.as_deref(),
            Some("TailRotorComponent")
        );
        assert_eq!(tail_rotor_failure.value_float, Some(0.0));

        let ammo_rack_damage = bundle
            .events
            .component_states
            .iter()
            .find(|event| {
                event.owner_actor_guid == Some(760)
                    && event.component_guid == Some(2144)
                    && event.property_name == "Health"
                    && event.t_ms == 534601
            })
            .expect("expected ammo rack damage event");
        assert_eq!(
            ammo_rack_damage.component_name.as_deref(),
            Some("AmmoRackComponent")
        );
        assert_eq!(
            ammo_rack_damage.component_class.as_deref(),
            Some("SQVehicleAmmoBox")
        );
        assert_eq!(ammo_rack_damage.value_float, Some(866.0));

        assert!(bundle.events.component_states.iter().any(|event| {
            event.owner_actor_guid == Some(760)
                && event.component_guid == Some(2146)
                && event.component_name.as_deref() == Some("TrackLeftComponent")
                && event.component_class.as_deref() == Some("SQVehicleTrack")
                && event.property_name == "Health"
                && event.t_ms == 525535
                && event.value_float == Some(0.0)
        }));
        assert!(bundle.events.component_states.iter().any(|event| {
            event.owner_actor_guid == Some(760)
                && event.component_guid == Some(2148)
                && event.component_name.as_deref() == Some("TrackRightComponent")
                && event.component_class.as_deref() == Some("SQVehicleTrack")
                && event.property_name == "Health"
                && event.t_ms == 525535
                && event.value_float == Some(0.0)
        }));
        assert!(bundle.events.seat_changes.iter().any(|event| {
            event.player_state_guid == Some(26)
                && event.component_guid == Some(3318)
                && event.vehicle_class.as_deref() == Some("BP_Loach_CAS_Small_C")
        }));
        assert!(
            bundle
                .events
                .seat_changes
                .iter()
                .filter(|event| event.player_state_guid == Some(26))
                .count()
                >= 9
        );

        assert!(bundle.events.properties.iter().any(|event| {
            event.group_path == "/Script/Squad.SQHelicopterMovementComponent"
                && event.property_name == "ReplicatedMovement"
                && matches!(event.actor_guid, Some(754 | 966 | 3764))
        }));

        let helicopter_tracks = bundle
            .tracks
            .helicopters
            .iter()
            .map(|track| (track.key.clone(), track))
            .collect::<HashMap<_, _>>();
        assert_eq!(helicopter_tracks.len(), 3);

        let loach_754 = helicopter_tracks
            .get("LOACH_754")
            .expect("expected direct track for LOACH_754");
        assert_eq!(loach_754.source, "movement_component_anchored");
        assert_eq!(loach_754.samples.len(), 215);
        assert_eq!(
            loach_754.samples.first().map(|sample| sample.t_ms),
            Some(16)
        );
        assert_eq!(
            loach_754.samples.last().map(|sample| sample.t_ms),
            Some(153013)
        );

        let loach_966 = helicopter_tracks
            .get("LOACH_966")
            .expect("expected direct track for LOACH_966");
        assert_eq!(loach_966.source, "movement_component_anchored");
        assert_eq!(loach_966.samples.len(), 1160);
        assert_eq!(
            loach_966.samples.first().map(|sample| sample.t_ms),
            Some(16)
        );
        assert_eq!(
            loach_966.samples.last().map(|sample| sample.t_ms),
            Some(144924)
        );

        let loach_3764 = helicopter_tracks
            .get("LOACH_3764")
            .expect("expected direct track for LOACH_3764");
        assert_eq!(loach_3764.source, "movement_component_anchored");
        assert_eq!(loach_3764.samples.len(), 1336);
        assert_eq!(
            loach_3764.samples.first().map(|sample| sample.t_ms),
            Some(55867)
        );
        assert_eq!(
            loach_3764.samples.last().map(|sample| sample.t_ms),
            Some(222871)
        );

        let compat = crate::compat::from_bundle(&bundle);
        let helicopter_keys = compat
            .helicopter_positions_per_second
            .values()
            .flat_map(|by_name| by_name.keys().cloned())
            .collect::<HashSet<_>>();
        assert!(helicopter_keys.contains("LOACH_754"));
        assert!(helicopter_keys.contains("LOACH_966"));
        assert!(helicopter_keys.contains("LOACH_3764"));
        assert!(!helicopter_keys.contains("HELICOPTER_972"));
        assert!(!helicopter_keys.contains("HELICOPTER_978"));
    }
}
