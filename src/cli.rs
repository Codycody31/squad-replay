use clap::{Parser, Subcommand};
use serde::Serialize;
use squadreplay::bundle::Bundle;
use squadreplay::{Error, ParseOptions, Result, compat, parse_file, read_bundle, sqrb, sqrj};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const ROOT_AFTER_HELP: &str = "\
Examples:
  squadreplay parse match.replay --format sqrj,sqrb --output out/match
  squadreplay parse match.replay --compat-json
  squadreplay inspect match.replay
  squadreplay show out/match.sqrb
  squadreplay unpack out/match.sqrb --output out/unpacked

Use --json to keep machine-readable output for scripts.
";

const PARSE_AFTER_HELP: &str = "\
Examples:
  squadreplay parse match.replay --format sqrj,sqrb
  squadreplay parse match.replay --output out/match --compat-json --json
";

const INSPECT_AFTER_HELP: &str = "\
Examples:
  squadreplay inspect match.replay
  squadreplay inspect match.replay --json
";

const SHOW_AFTER_HELP: &str = "\
Examples:
  squadreplay show out/match.sqrb
  squadreplay show out/match.sqrj.json --json
";

const UNPACK_AFTER_HELP: &str = "\
Examples:
  squadreplay unpack out/match.sqrb --output out/unpacked
  squadreplay unpack out/match.sqrb --output out/unpacked --json
";

#[derive(Debug, Parser)]
#[command(name = "squadreplay")]
#[command(about = "Parse and inspect Squad UE5 replay bundles")]
#[command(after_help = ROOT_AFTER_HELP)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,

    #[arg(
        long,
        global = true,
        help = "Print machine-readable JSON instead of the default terminal summary"
    )]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Parse a .replay file and write one or more bundle outputs")]
    #[command(after_help = PARSE_AFTER_HELP)]
    Parse {
        #[arg(value_name = "REPLAY", help = "Path to the .replay file to parse")]
        input: PathBuf,
        #[arg(
            long,
            short = 'f',
            default_value = "sqrj",
            value_name = "FORMATS",
            help = "Formats to write: sqrj, sqrb, or a comma-separated list"
        )]
        format: String,
        #[arg(
            long,
            short = 'o',
            value_name = "OUTPUT_BASE",
            help = "Output path prefix. Defaults to the input path without the .replay suffix"
        )]
        output: Option<PathBuf>,
        #[arg(
            long,
            help = "Also write a compatibility JSON file for older downstream consumers"
        )]
        compat_json: bool,
        #[arg(
            long,
            help = "Skip raw property events to keep output smaller and easier to inspect"
        )]
        no_properties: bool,
    },
    #[command(about = "Read a .replay file and print a summary")]
    #[command(after_help = INSPECT_AFTER_HELP)]
    Inspect {
        #[arg(value_name = "REPLAY", help = "Path to the .replay file to inspect")]
        input: PathBuf,
        #[arg(
            long,
            help = "Skip raw property events to keep output smaller and easier to inspect"
        )]
        no_properties: bool,
    },
    #[command(about = "Read an existing sqrj or sqrb bundle and print a summary")]
    #[command(after_help = SHOW_AFTER_HELP)]
    Show {
        #[arg(value_name = "BUNDLE", help = "Path to a .sqrj.json or .sqrb bundle")]
        input: PathBuf,
    },
    #[command(about = "Expand an sqrb bundle into section JSON files")]
    #[command(after_help = UNPACK_AFTER_HELP)]
    Unpack {
        #[arg(value_name = "BUNDLE", help = "Path to the .sqrb bundle to unpack")]
        input: PathBuf,
        #[arg(
            long,
            short = 'o',
            value_name = "OUTPUT_DIR",
            help = "Directory to write the unpacked JSON sections into"
        )]
        output: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Sqrj,
    Sqrb,
}

#[derive(Debug, Clone, Default)]
struct OutputSelection {
    sqrj: bool,
    sqrb: bool,
}

impl OutputSelection {
    fn parse_csv(input: &str) -> std::result::Result<Self, String> {
        let mut out = Self::default();
        for part in input.split(',').map(|s| s.trim().to_ascii_lowercase()) {
            match part.as_str() {
                "" => {}
                "sqrj" => out.sqrj = true,
                "sqrb" => out.sqrb = true,
                other => return Err(format!("unsupported format `{other}`")),
            }
        }
        if !out.sqrj && !out.sqrb {
            return Err("at least one format must be selected".to_string());
        }
        Ok(out)
    }

    fn iter(&self) -> std::vec::IntoIter<OutputFormat> {
        let mut formats = Vec::new();
        if self.sqrj {
            formats.push(OutputFormat::Sqrj);
        }
        if self.sqrb {
            formats.push(OutputFormat::Sqrb);
        }
        formats.into_iter()
    }
}

#[derive(Debug, Clone, Default)]
struct WrittenOutputs {
    sqrj: Option<PathBuf>,
    sqrb: Option<PathBuf>,
    compat_json: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BundleSummary<'a> {
    input: &'a str,
    map_name: Option<&'a str>,
    squad_version: Option<&'a str>,
    duration_ms: u64,
    teams: usize,
    squads: usize,
    players: usize,
    vehicles: usize,
    helicopters: usize,
    deployables: usize,
    components: usize,
    player_tracks: usize,
    vehicle_tracks: usize,
    helicopter_tracks: usize,
    kills: usize,
    deployments: usize,
    seat_changes: usize,
    component_states: usize,
    vehicle_states: usize,
    weapon_states: usize,
    property_events: usize,
    frames_processed: u64,
    packets_processed: u64,
    actor_opens: u64,
}

fn default_output_base(input: &Path) -> PathBuf {
    match (input.parent(), input.file_stem()) {
        (Some(parent), Some(stem)) => parent.join(stem),
        _ => input.with_extension(""),
    }
}

fn output_path_with_suffix(base: impl AsRef<Path>, suffix: &str) -> PathBuf {
    let mut path = base.as_ref().as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn written_outputs_json(written: &WrittenOutputs) -> serde_json::Value {
    serde_json::json!({
        "sqrj": written.sqrj.as_deref().map(path_text),
        "sqrb": written.sqrb.as_deref().map(path_text),
        "compat_json": written.compat_json.as_deref().map(path_text),
    })
}

fn io_err(path: impl AsRef<Path>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

fn write_outputs(
    bundle: &Bundle,
    formats: &OutputSelection,
    output_base: &Path,
    write_compat: bool,
) -> Result<WrittenOutputs> {
    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent).map_err(|source| io_err(parent, source))?;
    }

    let mut written = WrittenOutputs::default();
    for format in formats.iter() {
        match format {
            OutputFormat::Sqrj => {
                let path = output_path_with_suffix(output_base, ".sqrj.json");
                sqrj::write(bundle, &path)?;
                written.sqrj = Some(path);
            }
            OutputFormat::Sqrb => {
                let path = output_path_with_suffix(output_base, ".sqrb");
                sqrb::write(bundle, &path)?;
                written.sqrb = Some(path);
            }
        }
    }

    if write_compat {
        let path = output_path_with_suffix(output_base, ".compat-match.json");
        let file = File::create(&path).map_err(|source| io_err(&path, source))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &compat::from_bundle(bundle))?;
        // Explicit flush: `BufWriter::drop` swallows flush errors; without
        // this we could report success while leaving a truncated file.
        writer.flush().map_err(|source| io_err(&path, source))?;
        written.compat_json = Some(path);
    }

    Ok(written)
}

fn summarize<'a>(input: &'a str, bundle: &'a Bundle) -> BundleSummary<'a> {
    BundleSummary {
        input,
        map_name: bundle.replay.map_name.as_deref(),
        squad_version: bundle.replay.squad_version.as_deref(),
        duration_ms: bundle.replay.duration_ms,
        teams: bundle.teams.len(),
        squads: bundle.squads.len(),
        players: bundle.players.len(),
        vehicles: bundle.actors.vehicles.len(),
        helicopters: bundle.actors.helicopters.len(),
        deployables: bundle.actors.deployables.len(),
        components: bundle.actors.components.len(),
        player_tracks: bundle.tracks.players.len(),
        vehicle_tracks: bundle.tracks.vehicles.len(),
        helicopter_tracks: bundle.tracks.helicopters.len(),
        kills: bundle.events.kills.len(),
        deployments: bundle.events.deployments.len(),
        seat_changes: bundle.events.seat_changes.len(),
        component_states: bundle.events.component_states.len(),
        vehicle_states: bundle.events.vehicle_states.len(),
        weapon_states: bundle.events.weapon_states.len(),
        property_events: bundle.events.properties.len(),
        frames_processed: bundle.diagnostics.frames_processed,
        packets_processed: bundle.diagnostics.packets_processed,
        actor_opens: bundle.diagnostics.actor_opens,
    }
}

fn option_text(value: Option<&str>) -> &str {
    value.unwrap_or("unknown")
}

fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn render_summary_text(title: &str, summary: &BundleSummary<'_>) -> String {
    [
        title.to_string(),
        format!("Input: {}", summary.input),
        format!("Map: {}", option_text(summary.map_name)),
        format!("Squad version: {}", option_text(summary.squad_version)),
        format!("Duration: {}", format_duration(summary.duration_ms)),
        format!(
            "Entities: {} teams, {} squads, {} players",
            summary.teams, summary.squads, summary.players
        ),
        format!(
            "Actors: {} vehicles, {} helicopters, {} deployables, {} components",
            summary.vehicles, summary.helicopters, summary.deployables, summary.components
        ),
        format!(
            "Tracks: {} player, {} vehicle, {} helicopter",
            summary.player_tracks, summary.vehicle_tracks, summary.helicopter_tracks
        ),
        format!(
            "Events: {} kills, {} deployments, {} seat changes, {} property events",
            summary.kills, summary.deployments, summary.seat_changes, summary.property_events
        ),
        format!(
            "Diagnostics: {} frames, {} packets, {} actor opens",
            summary.frames_processed, summary.packets_processed, summary.actor_opens
        ),
    ]
    .join("\n")
}

fn render_written_outputs(written: &WrittenOutputs) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(path) = written.sqrj.as_ref() {
        lines.push(format!("  - {}", path_text(path)));
    }
    if let Some(path) = written.sqrb.as_ref() {
        lines.push(format!("  - {}", path_text(path)));
    }
    if let Some(path) = written.compat_json.as_ref() {
        lines.push(format!("  - {}", path_text(path)));
    }

    lines
}

fn render_parse_text(
    input: &Path,
    output_base: &Path,
    written: &WrittenOutputs,
    bundle: &Bundle,
) -> String {
    let input_display = path_text(input);
    let summary = summarize(&input_display, bundle);
    let mut lines = vec![
        "Replay converted".to_string(),
        format!("Input: {}", path_text(input)),
        format!("Output base: {}", path_text(output_base)),
        format!("Map: {}", option_text(summary.map_name)),
        format!("Squad version: {}", option_text(summary.squad_version)),
        format!("Duration: {}", format_duration(summary.duration_ms)),
        format!(
            "Entities: {} teams, {} squads, {} players",
            summary.teams, summary.squads, summary.players
        ),
        format!(
            "Events: {} kills, {} deployments, {} seat changes, {} property events",
            summary.kills, summary.deployments, summary.seat_changes, summary.property_events
        ),
        "Wrote:".to_string(),
    ];
    lines.extend(render_written_outputs(written));
    lines.join("\n")
}

fn render_unpack_text(input: &Path, output: &Path) -> String {
    [
        "Bundle unpacked".to_string(),
        format!("Input: {}", path_text(input)),
        format!("Output directory: {}", path_text(output)),
    ]
    .join("\n")
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_parse_result_json(
    input: &Path,
    output_base: &Path,
    written: &WrittenOutputs,
    bundle: &Bundle,
) -> Result<()> {
    let compat_preview = compat::from_bundle(bundle);
    let input_display = path_text(input);
    print_json(&serde_json::json!({
        "input": path_text(input),
        "outputBase": path_text(output_base),
        "written": written_outputs_json(written),
        "summary": summarize(&input_display, bundle),
        "compatPreview": {
            "mapName": compat_preview.map_name,
            "squadVersion": compat_preview.squad_version,
            "matchDurationSeconds": compat_preview.match_duration_seconds,
            "kills": compat_preview.kills.len(),
            "positionsPerSecond": compat_preview.positions_per_second.len(),
            "vehiclePositionsPerSecond": compat_preview.vehicle_positions_per_second.len(),
            "deployableEvents": compat_preview.deployable_events.len(),
        }
    }))
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Parse {
            input,
            format,
            output,
            compat_json,
            no_properties,
        } => {
            let formats = OutputSelection::parse_csv(&format).map_err(Error::Message)?;
            let output_base = output.unwrap_or_else(|| default_output_base(&input));
            let options = ParseOptions {
                include_property_events: !no_properties,
            };
            let bundle = parse_file(&input, &options)?;
            let written = write_outputs(&bundle, &formats, &output_base, compat_json)?;

            if cli.json {
                print_parse_result_json(&input, &output_base, &written, &bundle)
            } else {
                println!(
                    "{}",
                    render_parse_text(&input, &output_base, &written, &bundle)
                );
                Ok(())
            }
        }
        Command::Inspect {
            input,
            no_properties,
        } => {
            let options = ParseOptions {
                include_property_events: !no_properties,
            };
            let bundle = parse_file(&input, &options)?;
            let input_display = path_text(&input);
            let summary = summarize(&input_display, &bundle);

            if cli.json {
                print_json(&summary)
            } else {
                println!("{}", render_summary_text("Replay summary", &summary));
                Ok(())
            }
        }
        Command::Show { input } => {
            let bundle = read_bundle(&input)?;
            let input_display = path_text(&input);
            let summary = summarize(&input_display, &bundle);

            if cli.json {
                print_json(&summary)
            } else {
                println!("{}", render_summary_text("Bundle summary", &summary));
                Ok(())
            }
        }
        Command::Unpack { input, output } => {
            sqrb::unpack(&input, &output)?;

            if cli.json {
                print_json(&serde_json::json!({
                    "input": path_text(&input),
                    "output": path_text(&output),
                    "status": "ok"
                }))
            } else {
                println!("{}", render_unpack_text(&input, &output));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use squadreplay::bundle::{
        ActorEntity, ActorGroups, ComponentEntity, ComponentStateEvent, DeploymentEvent,
        Diagnostics, EventGroups, KillEvent, ReplayInfoSection, SeatChangeEvent, Track3,
        TrackGroups, VehicleStateEvent, WeaponStateEvent,
    };

    fn sample_bundle() -> Bundle {
        Bundle {
            replay: ReplayInfoSection {
                map_name: Some("Jensen's Range".to_string()),
                squad_version: Some("8.1.0".to_string()),
                duration_ms: 2_146_000,
                ..ReplayInfoSection::default()
            },
            teams: vec![Default::default(), Default::default()],
            squads: vec![Default::default(); 6],
            players: vec![Default::default(); 72],
            actors: ActorGroups {
                vehicles: vec![ActorEntity::default(); 12],
                helicopters: vec![ActorEntity::default(); 2],
                deployables: vec![ActorEntity::default(); 18],
                components: vec![ComponentEntity::default(); 44],
            },
            tracks: TrackGroups {
                players: vec![Track3::default(); 40],
                vehicles: vec![Track3::default(); 11],
                helicopters: vec![Track3::default(); 2],
            },
            events: EventGroups {
                kills: vec![KillEvent::default(); 15],
                deployments: vec![DeploymentEvent::default(); 9],
                seat_changes: vec![SeatChangeEvent::default(); 6],
                component_states: vec![ComponentStateEvent::default(); 5],
                vehicle_states: vec![VehicleStateEvent::default(); 7],
                weapon_states: vec![WeaponStateEvent::default(); 4],
                properties: vec![Default::default(); 125],
            },
            diagnostics: Diagnostics {
                frames_processed: 3_220,
                packets_processed: 8_441,
                actor_opens: 381,
                ..Diagnostics::default()
            },
            ..Bundle::default()
        }
    }

    #[test]
    fn summary_rendering_stays_readable() {
        let bundle = sample_bundle();
        let summary = summarize("match.replay", &bundle);
        let rendered = render_summary_text("Replay summary", &summary);

        assert!(rendered.contains("Replay summary"));
        assert!(rendered.contains("Input: match.replay"));
        assert!(rendered.contains("Map: Jensen's Range"));
        assert!(rendered.contains("Duration: 35:46"));
        assert!(rendered.contains("Entities: 2 teams, 6 squads, 72 players"));
        assert!(
            rendered
                .contains("Events: 15 kills, 9 deployments, 6 seat changes, 125 property events")
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_suffix_preserves_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let base = PathBuf::from(OsString::from_vec(b"match-\xFF".to_vec()));
        let output = output_path_with_suffix(&base, ".sqrb");

        assert_eq!(
            output
                .file_name()
                .expect("path should have a file name")
                .as_bytes(),
            b"match-\xFF.sqrb"
        );
    }
}
