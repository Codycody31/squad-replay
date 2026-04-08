use crate::bundle::{Bundle, ReplayInfoSection, SchemaInfo};
use crate::compat;
use crate::error::{Error, Result};
use crc32fast::Hasher as Crc32;
use rayon::prelude::*;
use rmp_serde::decode::from_slice as from_msgpack_slice;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};
use std::path::Path;

// Compression level. Was 10, which put zstd at ~75% of total CPU on large
// replays. Level 3 is zstd's default and gives output roughly 30% bigger
// for several times the throughput, which is the right tradeoff here.
const SQRB_ZSTD_LEVEL: i32 = 3;

// Worker count for zstd's built-in multithreading on the Properties
// section. Four leaves headroom for the rayon workers running the other
// sections; going wider oversubscribes without helping wall time.
const SQRB_ZSTD_HEAVY_WORKERS: u32 = 4;

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

fn zstd_decode(data: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::decode_all(Cursor::new(data))
        .map_err(|source| Error::Message(format!("zstd decode failed: {source}")))
}

/// Write adapter that passes bytes through to an inner writer while
/// accumulating a CRC32 and a running byte count. Lets the serializer
/// stream straight into zstd while still recovering the pre-compression
/// size and checksum that the sqrb header needs.
struct CountingCrcWriter<W: Write> {
    inner: W,
    crc: Crc32,
    bytes: u64,
}

impl<W: Write> CountingCrcWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            crc: Crc32::new(),
            bytes: 0,
        }
    }

    fn finish(self) -> (W, u32, u64) {
        (self.inner, self.crc.finalize(), self.bytes)
    }
}

impl<W: Write> Write for CountingCrcWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.crc.update(&buf[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Encode one sqrb section by streaming the serializer output into an
/// in-memory buffer, optionally through zstd on the way.
///
/// Pass a nonzero `zstd_mt_workers` to enable zstd's internal thread pool.
/// The pool has a fixed setup cost so it's only worth turning on for
/// sections large enough to pay it back (currently just Properties).
fn encode_section_streaming<F>(
    id: SectionId,
    json_payload: bool,
    compress: bool,
    item_count: u32,
    zstd_mt_workers: u32,
    serialize: F,
) -> Result<EncodedSection>
where
    F: FnOnce(&mut dyn Write) -> Result<()>,
{
    let mut flags: u16 = if json_payload { 0x0001 } else { 0x0002 };
    // Initial capacity only, the Vec grows on demand.
    let stored = Vec::<u8>::with_capacity(64 * 1024);
    let counter = CountingCrcWriter::new(stored);

    // The BufWriter above the CRC layer is load-bearing: without it
    // rmp_serde hands crc32fast tiny slices and falls off its SIMD path.
    // Keep the buffer fat enough that pclmulqdq stays engaged.
    const CRC_BUF_CAP: usize = 64 * 1024;

    let (stored, raw_len, crc32) = if compress {
        flags |= 0x0004;
        // CountingCrcWriter sits above zstd so raw_len and crc32 reflect
        // pre-compression bytes, matching the sqrb header contract.
        let mut zstd_encoder = zstd::stream::Encoder::new(
            Vec::<u8>::with_capacity(64 * 1024),
            SQRB_ZSTD_LEVEL,
        )
        .map_err(|source| Error::Message(format!("zstd encode init failed: {source}")))?;
        if zstd_mt_workers > 0 {
            zstd_encoder
                .multithread(zstd_mt_workers)
                .map_err(|source| {
                    Error::Message(format!("zstd multithread init failed: {source}"))
                })?;
        }
        let counter = CountingCrcWriter::new(&mut zstd_encoder);
        let mut buffered = BufWriter::with_capacity(CRC_BUF_CAP, counter);
        serialize(&mut buffered)?;
        buffered
            .flush()
            .map_err(|source| Error::Message(format!("sqrb buffer flush failed: {source}")))?;
        let counter = buffered
            .into_inner()
            .map_err(|source| Error::Message(format!("sqrb buffer unwrap failed: {source}")))?;
        let (_, crc32, raw_len) = counter.finish();
        let compressed = zstd_encoder
            .finish()
            .map_err(|source| Error::Message(format!("zstd encode finish failed: {source}")))?;
        (compressed, raw_len, crc32)
    } else {
        let mut buffered = BufWriter::with_capacity(CRC_BUF_CAP, counter);
        serialize(&mut buffered)?;
        buffered
            .flush()
            .map_err(|source| Error::Message(format!("sqrb buffer flush failed: {source}")))?;
        let counter = buffered
            .into_inner()
            .map_err(|source| Error::Message(format!("sqrb buffer unwrap failed: {source}")))?;
        let (stored, crc32, raw_len) = counter.finish();
        (stored, raw_len, crc32)
    };

    // The old encoder skipped zstd for payloads under 1 KiB; streaming
    // makes that check awkward and the overhead on small sections is
    // negligible, so we let zstd run unconditionally.
    Ok(EncodedSection {
        id,
        flags,
        stored_len: stored.len() as u64,
        raw_len,
        crc32,
        item_count,
        stored,
    })
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

/// Returns a closure that msgpack-encodes `slice` as one sqrb section.
fn msgpack_section<'a, T>(
    id: SectionId,
    compress: bool,
    zstd_mt_workers: u32,
    slice: &'a [T],
) -> Box<dyn FnOnce() -> Result<EncodedSection> + Send + 'a>
where
    T: Serialize + Sync,
{
    let item_count = slice.len() as u32;
    Box::new(move || {
        encode_section_streaming(id, false, compress, item_count, zstd_mt_workers, |w| {
            rmp_serde::encode::write_named(w, &slice)
                .map_err(|source| Error::Message(format!("msgpack encode failed: {source}")))?;
            Ok(())
        })
    })
}

/// Single-value variant of [`msgpack_section`], used for `Diagnostics`.
fn msgpack_section_single<'a, T>(
    id: SectionId,
    compress: bool,
    value: &'a T,
) -> Box<dyn FnOnce() -> Result<EncodedSection> + Send + 'a>
where
    T: Serialize + Sync,
{
    Box::new(move || {
        encode_section_streaming(id, false, compress, 1, 0, |w| {
            rmp_serde::encode::write_named(w, value)
                .map_err(|source| Error::Message(format!("msgpack encode failed: {source}")))?;
            Ok(())
        })
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

    // Each entry is a boxed closure so rayon can own them and run them in
    // parallel. The closures borrow from `bundle` and `manifest`, which
    // both outlive the parallel collect below, so no cloning is needed.
    type SectionEncoder<'a> = Box<dyn FnOnce() -> Result<EncodedSection> + Send + 'a>;

    // Manifest is JSON, everything else is msgpack. The heavier sections
    // get zstd compression.
    let sections: Vec<SectionEncoder<'_>> = vec![
        Box::new(move || {
            encode_section_streaming(SectionId::Manifest, true, false, 1, 0, |w| {
                serde_json::to_writer_pretty(w, &manifest)?;
                Ok(())
            })
        }),
        msgpack_section(SectionId::Teams, false, 0, &bundle.teams),
        msgpack_section(SectionId::Squads, false, 0, &bundle.squads),
        msgpack_section(SectionId::Players, false, 0, &bundle.players),
        msgpack_section(SectionId::Vehicles, false, 0, &bundle.actors.vehicles),
        msgpack_section(SectionId::Helicopters, false, 0, &bundle.actors.helicopters),
        msgpack_section(SectionId::Deployables, false, 0, &bundle.actors.deployables),
        msgpack_section(SectionId::Components, false, 0, &bundle.actors.components),
        msgpack_section(SectionId::PlayerTracks, true, 0, &bundle.tracks.players),
        msgpack_section(SectionId::VehicleTracks, true, 0, &bundle.tracks.vehicles),
        msgpack_section(SectionId::HelicopterTracks, true, 0, &bundle.tracks.helicopters),
        msgpack_section(SectionId::Kills, false, 0, &bundle.events.kills),
        msgpack_section(SectionId::Deployments, false, 0, &bundle.events.deployments),
        msgpack_section(SectionId::SeatChanges, false, 0, &bundle.events.seat_changes),
        msgpack_section(SectionId::ComponentStates, true, 0, &bundle.events.component_states),
        msgpack_section(SectionId::VehicleStates, true, 0, &bundle.events.vehicle_states),
        msgpack_section(SectionId::WeaponStates, true, 0, &bundle.events.weapon_states),
        // Properties is the only section big enough to repay the cost of
        // spinning up zstd's worker pool.
        msgpack_section(
            SectionId::Properties,
            true,
            SQRB_ZSTD_HEAVY_WORKERS,
            &bundle.events.properties,
        ),
        msgpack_section_single(SectionId::Diagnostics, false, &bundle.diagnostics),
    ];
    let section_count = sections.len() as u32;

    // Encode sections in parallel. Properties dominates wall time either
    // way, so most of the win comes from zstdmt inside that encoder; the
    // rest just overlap for free.
    let encoded: Vec<EncodedSection> = sections
        .into_par_iter()
        .map(|encoder| encoder())
        .collect::<Result<Vec<_>>>()?;

    let header_len = 4 + 2 + 2 + 4 + 8 + 4 + 8;
    let mut offset = header_len as u64;
    let file = File::create(path).map_err(|source| io_err(path, source))?;
    let mut writer = BufWriter::new(file);
    let mut directory = Vec::with_capacity(encoded.len());

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
        .write_all(&section_count.to_le_bytes())
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

    for section in encoded {
        write_encoded_section(&mut writer, &mut directory, &mut offset, section)?;
    }

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
                    group_path: "/Script/Squad.SQRotorComponent".into(),
                    property_name: "Health".into(),
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
