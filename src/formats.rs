use crate::bundle::{Bundle, ReplayInfoSection, SchemaInfo};
use crate::compat;
use crate::error::{Error, Result};
use crc32fast::Hasher as Crc32;
use rmp_serde::{decode::from_slice as from_msgpack_slice, encode::to_vec_named as to_msgpack_vec};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};
use std::path::Path;

const SQRB_MAGIC: &[u8; 4] = b"SQRB";
const SQRB_MAJOR: u16 = 1;
const SQRB_MINOR: u16 = 0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    schema: SchemaInfo,
    replay: ReplayInfoSection,
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
enum SectionId {
    Manifest = 1,
    Teams = 2,
    Squads = 3,
    Players = 4,
    Vehicles = 5,
    Helicopters = 6,
    Deployables = 7,
    Components = 8,
    PlayerTracks = 9,
    VehicleTracks = 10,
    HelicopterTracks = 11,
    Kills = 12,
    Deployments = 13,
    SeatChanges = 14,
    ComponentStates = 15,
    VehicleStates = 16,
    WeaponStates = 17,
    Properties = 18,
    Diagnostics = 19,
}

#[derive(Debug, Clone)]
struct EncodedSection {
    id: SectionId,
    flags: u16,
    stored_len: u64,
    raw_len: u64,
    crc32: u32,
    item_count: u32,
    stored: Vec<u8>,
}

#[derive(Debug, Clone)]
struct SectionDirectoryEntry {
    id: SectionId,
    flags: u16,
    offset: u64,
    stored_len: u64,
    raw_len: u64,
    crc32: u32,
    item_count: u32,
}

fn io_err(path: impl AsRef<Path>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

fn zstd_encode(data: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::encode_all(Cursor::new(data), 10)
        .map_err(|source| Error::Message(format!("zstd encode failed: {source}")))
}

fn zstd_decode(data: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::decode_all(Cursor::new(data))
        .map_err(|source| Error::Message(format!("zstd decode failed: {source}")))
}

pub(crate) fn write_sqrj(bundle: &Bundle, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let file = File::create(path).map_err(|source| io_err(path, source))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, bundle)?;
    // Flush explicitly so write errors do not get lost on drop.
    writer.flush().map_err(|source| io_err(path, source))?;
    Ok(())
}

pub(crate) fn read_sqrj(path: impl AsRef<Path>) -> Result<Bundle> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|source| io_err(path, source))?;
    let bundle = serde_json::from_slice(&bytes)?;
    Ok(bundle)
}

fn encode_section(
    id: SectionId,
    json_payload: bool,
    compress: bool,
    item_count: u32,
    raw: Vec<u8>,
) -> Result<EncodedSection> {
    let raw_len = raw.len() as u64;
    let mut crc = Crc32::new();
    crc.update(&raw);
    let mut flags: u16 = if json_payload { 0x0001 } else { 0x0002 };
    let stored = if compress && raw.len() > 1024 {
        flags |= 0x0004;
        zstd_encode(&raw)?
    } else {
        raw
    };
    Ok(EncodedSection {
        id,
        flags,
        stored_len: stored.len() as u64,
        raw_len,
        crc32: crc.finalize(),
        item_count,
        stored,
    })
}

fn write_encoded_section(
    writer: &mut BufWriter<File>,
    directory: &mut Vec<SectionDirectoryEntry>,
    offset: &mut u64,
    section: EncodedSection,
) -> Result<()> {
    writer
        .write_all(&section.stored)
        .map_err(|source| io_err("<sqrb-stream>", source))?;
    directory.push(SectionDirectoryEntry {
        id: section.id,
        flags: section.flags,
        offset: *offset,
        stored_len: section.stored_len,
        raw_len: section.raw_len,
        crc32: section.crc32,
        item_count: section.item_count,
    });
    *offset += section.stored_len;
    Ok(())
}

pub(crate) fn write_sqrb(bundle: &Bundle, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();

    let manifest = Manifest {
        schema: bundle.schema.clone(),
        replay: bundle.replay.clone(),
    };

    let header_len = 4 + 2 + 2 + 4 + 8 + 4 + 8;
    let mut offset = header_len as u64;
    let file = File::create(path).map_err(|source| io_err(path, source))?;
    let mut writer = BufWriter::new(file);
    let mut directory = Vec::with_capacity(19);

    writer
        .write_all(SQRB_MAGIC)
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&SQRB_MAJOR.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&SQRB_MINOR.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&19u32.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&0u64.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&0u32.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    writer
        .write_all(&0u64.to_le_bytes())
        .map_err(|source| io_err(path, source))?;

    macro_rules! stream_section {
        ($id:expr, $json:expr, $compress:expr, $count:expr, $raw:expr) => {{
            let section = encode_section($id, $json, $compress, $count, $raw)?;
            write_encoded_section(&mut writer, &mut directory, &mut offset, section)?;
        }};
    }

    stream_section!(
        SectionId::Manifest,
        true,
        false,
        1,
        serde_json::to_vec_pretty(&manifest)?
    );
    stream_section!(
        SectionId::Teams,
        false,
        false,
        bundle.teams.len() as u32,
        to_msgpack_vec(&bundle.teams)?
    );
    stream_section!(
        SectionId::Squads,
        false,
        false,
        bundle.squads.len() as u32,
        to_msgpack_vec(&bundle.squads)?
    );
    stream_section!(
        SectionId::Players,
        false,
        false,
        bundle.players.len() as u32,
        to_msgpack_vec(&bundle.players)?
    );
    stream_section!(
        SectionId::Vehicles,
        false,
        false,
        bundle.actors.vehicles.len() as u32,
        to_msgpack_vec(&bundle.actors.vehicles)?
    );
    stream_section!(
        SectionId::Helicopters,
        false,
        false,
        bundle.actors.helicopters.len() as u32,
        to_msgpack_vec(&bundle.actors.helicopters)?
    );
    stream_section!(
        SectionId::Deployables,
        false,
        false,
        bundle.actors.deployables.len() as u32,
        to_msgpack_vec(&bundle.actors.deployables)?
    );
    stream_section!(
        SectionId::Components,
        false,
        false,
        bundle.actors.components.len() as u32,
        to_msgpack_vec(&bundle.actors.components)?
    );
    stream_section!(
        SectionId::PlayerTracks,
        false,
        true,
        bundle.tracks.players.len() as u32,
        to_msgpack_vec(&bundle.tracks.players)?
    );
    stream_section!(
        SectionId::VehicleTracks,
        false,
        true,
        bundle.tracks.vehicles.len() as u32,
        to_msgpack_vec(&bundle.tracks.vehicles)?
    );
    stream_section!(
        SectionId::HelicopterTracks,
        false,
        true,
        bundle.tracks.helicopters.len() as u32,
        to_msgpack_vec(&bundle.tracks.helicopters)?
    );
    stream_section!(
        SectionId::Kills,
        false,
        false,
        bundle.events.kills.len() as u32,
        to_msgpack_vec(&bundle.events.kills)?
    );
    stream_section!(
        SectionId::Deployments,
        false,
        false,
        bundle.events.deployments.len() as u32,
        to_msgpack_vec(&bundle.events.deployments)?
    );
    stream_section!(
        SectionId::SeatChanges,
        false,
        false,
        bundle.events.seat_changes.len() as u32,
        to_msgpack_vec(&bundle.events.seat_changes)?
    );
    stream_section!(
        SectionId::ComponentStates,
        false,
        true,
        bundle.events.component_states.len() as u32,
        to_msgpack_vec(&bundle.events.component_states)?
    );
    stream_section!(
        SectionId::VehicleStates,
        false,
        true,
        bundle.events.vehicle_states.len() as u32,
        to_msgpack_vec(&bundle.events.vehicle_states)?
    );
    stream_section!(
        SectionId::WeaponStates,
        false,
        true,
        bundle.events.weapon_states.len() as u32,
        to_msgpack_vec(&bundle.events.weapon_states)?
    );
    stream_section!(
        SectionId::Properties,
        false,
        true,
        bundle.events.properties.len() as u32,
        to_msgpack_vec(&bundle.events.properties)?
    );
    stream_section!(
        SectionId::Diagnostics,
        false,
        false,
        1,
        to_msgpack_vec(&bundle.diagnostics)?
    );

    let directory_offset = offset;
    for section in &directory {
        writer
            .write_all(&(section.id as u16).to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.flags.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&0u32.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.offset.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.stored_len.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.raw_len.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.crc32.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
        writer
            .write_all(&section.item_count.to_le_bytes())
            .map_err(|source| io_err(path, source))?;
    }
    writer.flush().map_err(|source| io_err(path, source))?;

    let mut file = writer
        .into_inner()
        .map_err(|source| io_err(path, source.into_error()))?;
    file.seek(SeekFrom::Start(12))
        .map_err(|source| io_err(path, source))?;
    file.write_all(&directory_offset.to_le_bytes())
        .map_err(|source| io_err(path, source))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct DirectoryEntry {
    id: u16,
    flags: u16,
    offset: u64,
    stored_len: u64,
    crc32: u32,
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16> {
    if *offset + 2 > bytes.len() {
        return Err(Error::InvalidSqrb("unexpected end of file".to_string()));
    }
    let value = u16::from_le_bytes(bytes[*offset..*offset + 2].try_into().unwrap());
    *offset += 2;
    Ok(value)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    if *offset + 4 > bytes.len() {
        return Err(Error::InvalidSqrb("unexpected end of file".to_string()));
    }
    let value = u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap());
    *offset += 4;
    Ok(value)
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    if *offset + 8 > bytes.len() {
        return Err(Error::InvalidSqrb("unexpected end of file".to_string()));
    }
    let value = u64::from_le_bytes(bytes[*offset..*offset + 8].try_into().unwrap());
    *offset += 8;
    Ok(value)
}

pub(crate) fn read_sqrb(path: impl AsRef<Path>) -> Result<Bundle> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|source| io_err(path, source))?;
    let mut cursor = 0usize;

    if bytes.len() < 32 || &bytes[..4] != SQRB_MAGIC {
        return Err(Error::InvalidSqrb("bad magic".to_string()));
    }
    cursor += 4;
    let major = read_u16(&bytes, &mut cursor)?;
    let _minor = read_u16(&bytes, &mut cursor)?;
    if major != SQRB_MAJOR {
        return Err(Error::InvalidSqrb(format!(
            "unsupported major version {major}"
        )));
    }
    let section_count = read_u32(&bytes, &mut cursor)? as usize;
    let directory_offset = read_u64(&bytes, &mut cursor)? as usize;
    let _flags = read_u32(&bytes, &mut cursor)?;
    let _reserved = read_u64(&bytes, &mut cursor)?;

    // `directory_offset` starts as a placeholder and gets patched at the end.
    // If it is still zero or out of range, the file is probably truncated.
    if directory_offset < 32 || directory_offset > bytes.len() {
        return Err(Error::InvalidSqrb(format!(
            "directory offset {directory_offset} is out of range (file size {}) — \
             the bundle is likely truncated or the writer did not patch the header",
            bytes.len()
        )));
    }

    let mut directory = Vec::with_capacity(section_count);
    let mut dir_cursor = directory_offset;
    for _ in 0..section_count {
        let id = read_u16(&bytes, &mut dir_cursor)?;
        let flags = read_u16(&bytes, &mut dir_cursor)?;
        let _reserved = read_u32(&bytes, &mut dir_cursor)?;
        let offset = read_u64(&bytes, &mut dir_cursor)?;
        let stored_len = read_u64(&bytes, &mut dir_cursor)?;
        let _raw_len = read_u64(&bytes, &mut dir_cursor)?;
        let crc32 = read_u32(&bytes, &mut dir_cursor)?;
        let _item_count = read_u32(&bytes, &mut dir_cursor)?;
        directory.push(DirectoryEntry {
            id,
            flags,
            offset,
            stored_len,
            crc32,
        });
    }

    let mut bundle = Bundle::default();

    for entry in directory {
        let start = entry.offset as usize;
        let end = start + entry.stored_len as usize;
        if end > bytes.len() {
            return Err(Error::InvalidSqrb("section out of bounds".to_string()));
        }
        let stored = &bytes[start..end];
        let raw = if (entry.flags & 0x0004) != 0 {
            zstd_decode(stored)?
        } else {
            stored.to_vec()
        };

        let mut crc = Crc32::new();
        crc.update(&raw);
        if crc.finalize() != entry.crc32 {
            return Err(Error::InvalidSqrb(format!(
                "crc mismatch on section {}",
                entry.id
            )));
        }

        match entry.id {
            1 => {
                let manifest: Manifest = serde_json::from_slice(&raw)?;
                bundle.schema = manifest.schema;
                bundle.replay = manifest.replay;
            }
            2 => bundle.teams = from_msgpack_slice(&raw)?,
            3 => bundle.squads = from_msgpack_slice(&raw)?,
            4 => bundle.players = from_msgpack_slice(&raw)?,
            5 => bundle.actors.vehicles = from_msgpack_slice(&raw)?,
            6 => bundle.actors.helicopters = from_msgpack_slice(&raw)?,
            7 => bundle.actors.deployables = from_msgpack_slice(&raw)?,
            8 => bundle.actors.components = from_msgpack_slice(&raw)?,
            9 => bundle.tracks.players = from_msgpack_slice(&raw)?,
            10 => bundle.tracks.vehicles = from_msgpack_slice(&raw)?,
            11 => bundle.tracks.helicopters = from_msgpack_slice(&raw)?,
            12 => bundle.events.kills = from_msgpack_slice(&raw)?,
            13 => bundle.events.deployments = from_msgpack_slice(&raw)?,
            14 => bundle.events.seat_changes = from_msgpack_slice(&raw)?,
            15 => bundle.events.component_states = from_msgpack_slice(&raw)?,
            16 => bundle.events.vehicle_states = from_msgpack_slice(&raw)?,
            17 => bundle.events.weapon_states = from_msgpack_slice(&raw)?,
            18 => bundle.events.properties = from_msgpack_slice(&raw)?,
            19 => bundle.diagnostics = from_msgpack_slice(&raw)?,
            _ => {}
        }
    }

    Ok(bundle)
}

pub(crate) fn unpack_sqrb(path: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let output_dir = output_dir.as_ref();
    fs::create_dir_all(output_dir).map_err(|source| io_err(output_dir, source))?;
    let bundle = read_sqrb(path)?;
    write_sqrj(&bundle, output_dir.join("bundle.sqrj.json"))?;
    let compat = compat::from_bundle(&bundle);
    let compat_bytes = serde_json::to_vec_pretty(&compat)?;
    fs::write(output_dir.join("compat-match.json"), compat_bytes)
        .map_err(|source| io_err(output_dir.join("compat-match.json"), source))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{
        ActorEntity, ActorGroups, Bundle, ComponentEntity, ComponentStateEvent,
        DecodedPropertyValue, Diagnostics, EventGroups, PropertyEvent, ProvenanceEntry,
        ReplayInfoSection, ReplaySourceInfo, Track3, TrackGroups, TrackSample3,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_bundle() -> Bundle {
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

    #[test]
    fn write_sqrj_uses_compact_json() {
        let bundle = Bundle::default();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("squadreplay-{unique}.sqrj.json"));

        write_sqrj(&bundle, &path).unwrap();

        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(bytes, serde_json::to_vec(&bundle).unwrap());
        assert!(!bytes.contains(&b'\n'));
    }

    #[test]
    fn sqrb_roundtrip_preserves_canonical_bundle() {
        let bundle = sample_bundle();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("squadreplay-{unique}.sqrb"));

        write_sqrb(&bundle, &path).unwrap();
        let roundtrip = read_sqrb(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(
            serde_json::to_value(&roundtrip).unwrap(),
            serde_json::to_value(&bundle).unwrap()
        );
    }
}
