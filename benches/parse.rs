//! End-to-end parsing benchmarks.
//!
//! Covers `parse_file`, `parse_bytes`, property-retention comparison, and a
//! full round-trip (parse -> sqrb write -> sqrb read).

#[path = "common.rs"]
mod common;

use common::{
    TempFile, available_fixtures, fixture_path, load_fixture_bytes, skip_if_missing, unique_path,
    JENSENS,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use squadreplay::ParseOptions;
use std::hint::black_box;
use std::time::Duration;

// ---------------------------------------------------------------------------
// parse_file — includes file I/O for real-world timing
// ---------------------------------------------------------------------------

fn bench_parse_file(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("parse_file");

    for info in &fixtures {
        let path = fixture_path(info.file).unwrap();
        group.throughput(Throughput::Bytes(info.size));
        group.sample_size(10);
        group.measurement_time(if info.size > 20_000_000 {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(15)
        });

        group.bench_with_input(
            BenchmarkId::new("default_opts", info.label),
            &path,
            |b, path| {
                let opts = ParseOptions::default();
                b.iter(|| black_box(squadreplay::parse_file(path, &opts).unwrap()));
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// parse_bytes — isolates CPU from file I/O
// ---------------------------------------------------------------------------

fn bench_parse_bytes(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("parse_bytes");

    for info in &fixtures {
        let bytes = load_fixture_bytes(info.file);
        group.throughput(Throughput::Bytes(info.size));
        group.sample_size(10);
        group.measurement_time(if info.size > 20_000_000 {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(15)
        });

        group.bench_with_input(
            BenchmarkId::new("default_opts", info.label),
            &bytes,
            |b, data| {
                let opts = ParseOptions::default();
                b.iter(|| {
                    black_box(
                        squadreplay::parse_bytes(data, Some(info.file.to_string()), &opts).unwrap(),
                    )
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// property_retention — with vs without property events
// ---------------------------------------------------------------------------

fn bench_property_retention(c: &mut Criterion) {
    let Some(path) = fixture_path(JENSENS) else {
        eprintln!("benchmark skipped: Jensen's fixture not found");
        return;
    };

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let bytes = load_fixture_bytes(JENSENS);

    let mut group = c.benchmark_group("property_retention");
    group
        .throughput(Throughput::Bytes(size))
        .sample_size(10)
        .measurement_time(Duration::from_secs(20));

    group.bench_function("with_properties", |b| {
        let opts = ParseOptions {
            include_property_events: true,
        };
        b.iter(|| {
            black_box(squadreplay::parse_bytes(bytes, Some(JENSENS.to_string()), &opts).unwrap())
        });
    });

    group.bench_function("without_properties", |b| {
        let opts = ParseOptions {
            include_property_events: false,
        };
        b.iter(|| {
            black_box(squadreplay::parse_bytes(bytes, Some(JENSENS.to_string()), &opts).unwrap())
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// full_roundtrip — parse -> sqrb write -> sqrb read
// ---------------------------------------------------------------------------

fn bench_full_roundtrip(c: &mut Criterion) {
    let Some(path) = fixture_path(JENSENS) else {
        eprintln!("benchmark skipped: Jensen's fixture not found");
        return;
    };

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let bytes = load_fixture_bytes(JENSENS);

    let mut group = c.benchmark_group("full_roundtrip");
    group
        .throughput(Throughput::Bytes(size))
        .sample_size(10)
        .measurement_time(Duration::from_secs(30));

    // Measures the full pipeline: parse + sqrb write + sqrb read.
    // The write/read paths are also benchmarked individually in
    // serialization.rs; this benchmark captures the combined latency.
    group.bench_function("jensens_11mb", |b| {
        let opts = ParseOptions::default();
        b.iter(|| {
            let bundle =
                squadreplay::parse_bytes(bytes, Some(JENSENS.to_string()), &opts).unwrap();
            let tmp = TempFile(unique_path("bench-roundtrip", ".sqrb"));
            squadreplay::sqrb::write(&bundle, &tmp.0).unwrap();
            black_box(squadreplay::sqrb::read(&tmp.0).unwrap())
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion wiring
// ---------------------------------------------------------------------------

criterion_group! {
    name = benches;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.02);
    targets =
        bench_parse_file,
        bench_parse_bytes,
        bench_property_retention,
        bench_full_roundtrip
}
criterion_main!(benches);
