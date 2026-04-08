mod common;

use common::{fixture_dir, sample_bundle, unique_path};
use squadreplay::{sqrb, sqrj};
use std::ffi::OsStr;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn arg(value: &str) -> &OsStr {
    OsStr::new(value)
}

fn run<'a>(args: impl IntoIterator<Item = &'a OsStr>) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squadreplay"))
        .args(args)
        .output()
        .expect("binary should run")
}

fn path_with_suffix(base: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(suffix);
    std::path::PathBuf::from(path)
}

#[test]
fn help_commands_render_examples() {
    let root = run([arg("--help")]);
    assert!(root.status.success());
    let root_stdout = String::from_utf8(root.stdout).expect("stdout should be utf8");
    assert!(root_stdout.contains("Parse and inspect Squad UE5 replay bundles"));
    assert!(root_stdout.contains("squadreplay parse match.replay"));

    let parse = run([arg("parse"), arg("--help")]);
    assert!(parse.status.success());
    let parse_stdout = String::from_utf8(parse.stdout).expect("stdout should be utf8");
    assert!(parse_stdout.contains("Parse a .replay file and write one or more bundle outputs"));
    assert!(parse_stdout.contains("--compat-json"));

    let inspect = run([arg("inspect"), arg("--help")]);
    assert!(inspect.status.success());
    let inspect_stdout = String::from_utf8(inspect.stdout).expect("stdout should be utf8");
    assert!(inspect_stdout.contains("Read a .replay file and print a summary"));

    let show = run([arg("show"), arg("--help")]);
    assert!(show.status.success());
    let show_stdout = String::from_utf8(show.stdout).expect("stdout should be utf8");
    assert!(show_stdout.contains("Read an existing sqrj or sqrb bundle and print a summary"));

    let unpack = run([arg("unpack"), arg("--help")]);
    assert!(unpack.status.success());
    let unpack_stdout = String::from_utf8(unpack.stdout).expect("stdout should be utf8");
    assert!(unpack_stdout.contains("Expand an sqrb bundle into section JSON files"));
}

#[test]
fn show_and_unpack_work_with_generated_bundles() {
    let bundle = sample_bundle();
    let sqrj_path = unique_path("squadreplay-cli", ".sqrj.json");
    let sqrb_path = unique_path("squadreplay-cli", ".sqrb");
    let unpack_dir = unique_path("squadreplay-unpack", "");

    sqrj::write(&bundle, &sqrj_path).expect("sqrj write should succeed");
    sqrb::write(&bundle, &sqrb_path).expect("sqrb write should succeed");
    fs::create_dir_all(&unpack_dir).expect("temporary unpack dir should be created");

    let show = run([arg("show"), sqrb_path.as_os_str()]);
    assert!(show.status.success());
    let show_stdout = String::from_utf8(show.stdout).expect("stdout should be utf8");
    assert!(show_stdout.contains("Bundle summary"));
    assert!(show_stdout.contains("Map: Jensens_Range"));

    let unpack = run([
        arg("unpack"),
        sqrb_path.as_os_str(),
        arg("--output"),
        unpack_dir.as_os_str(),
    ]);
    assert!(unpack.status.success());
    assert!(unpack_dir.join("bundle.sqrj.json").exists());
    assert!(unpack_dir.join("compat-match.json").exists());

    fs::remove_file(&sqrj_path).expect("temporary sqrj should be removable");
    fs::remove_file(&sqrb_path).expect("temporary sqrb should be removable");
    fs::remove_file(unpack_dir.join("bundle.sqrj.json"))
        .expect("temporary unpacked sqrj should be removable");
    fs::remove_file(unpack_dir.join("compat-match.json"))
        .expect("temporary compat json should be removable");
    fs::remove_dir(&unpack_dir).expect("temporary unpack dir should be removable");
}

#[test]
fn parse_json_output_shape_is_verified_when_fixture_is_available() {
    let Some(fixture_dir) = fixture_dir() else {
        return;
    };
    let fixture_path = fixture_dir.join("rtb-jensens-range-wpmc-vs-turkey-20260407.replay");
    if !fixture_path.exists() {
        return;
    }

    let output_base = unique_path("squadreplay-parse", "");
    let output = run([
        arg("parse"),
        fixture_path.as_os_str(),
        arg("--json"),
        arg("--output"),
        output_base.as_os_str(),
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("\"written\""));
    assert!(stdout.contains("\"summary\""));

    let sqrj_path = path_with_suffix(&output_base, ".sqrj.json");
    if sqrj_path.exists() {
        fs::remove_file(sqrj_path).expect("temporary parsed sqrj should be removable");
    }
}

#[cfg(unix)]
#[test]
fn unpack_json_supports_non_utf8_bundle_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let bundle = sample_bundle();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let mut name = format!("squadreplay-cli-{unique}-").into_bytes();
    name.push(0xFF);
    name.extend_from_slice(b".sqrb");

    let sqrb_path = std::env::temp_dir().join(OsString::from_vec(name));
    let unpack_dir = unique_path("squadreplay-nonutf8-unpack", "");

    sqrb::write(&bundle, &sqrb_path).expect("sqrb write should succeed");
    fs::create_dir_all(&unpack_dir).expect("temporary unpack dir should be created");

    let unpack = run([
        arg("unpack"),
        sqrb_path.as_os_str(),
        arg("--output"),
        unpack_dir.as_os_str(),
        arg("--json"),
    ]);
    assert!(unpack.status.success());

    let stdout: serde_json::Value =
        serde_json::from_slice(&unpack.stdout).expect("stdout should be valid json");
    assert_eq!(
        stdout.get("status").and_then(|value| value.as_str()),
        Some("ok")
    );
    assert!(
        stdout
            .get("input")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        stdout
            .get("output")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(unpack_dir.join("bundle.sqrj.json").exists());
    assert!(unpack_dir.join("compat-match.json").exists());

    fs::remove_file(&sqrb_path).expect("temporary sqrb should be removable");
    fs::remove_file(unpack_dir.join("bundle.sqrj.json"))
        .expect("temporary unpacked sqrj should be removable");
    fs::remove_file(unpack_dir.join("compat-match.json"))
        .expect("temporary compat json should be removable");
    fs::remove_dir(&unpack_dir).expect("temporary unpack dir should be removable");
}
