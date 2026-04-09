//! Serialization benchmarks: SQRB write/read, SQRJ write/read, and compat
//! projection.
//!
//! Each benchmark pre-parses the fixture once (cached via `OnceLock`) so that
//! only the serialization/deserialization cost is measured.

#[path = "common.rs"]
mod common;

use common::{
    TempFile, available_fixtures, parse_fixture_bundle, skip_if_missing, unique_path,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;

// ---------------------------------------------------------------------------
// sqrb_write — binary msgpack + zstd compression
// ---------------------------------------------------------------------------

fn bench_sqrb_write(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("sqrb_write");
    group.sample_size(10).measurement_time(Duration::from_secs(20));

    for info in &fixtures {
        let bundle = parse_fixture_bundle(info.file, true);
        group.throughput(Throughput::Bytes(info.size));

        group.bench_with_input(
            BenchmarkId::from_parameter(info.label),
            &bundle,
            |b, bundle| {
                b.iter(|| {
                    let tmp = TempFile(unique_path("bench-sqrb-w", ".sqrb"));
                    black_box(squadreplay::sqrb::write(bundle, &tmp.0).unwrap());
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// sqrb_read — binary deserialization
// ---------------------------------------------------------------------------

fn bench_sqrb_read(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("sqrb_read");
    group.sample_size(10).measurement_time(Duration::from_secs(15));

    for info in &fixtures {
        let bundle = parse_fixture_bundle(info.file, true);

        // Pre-write once so we measure only the read path.
        let sqrb_file = TempFile(unique_path("bench-sqrb-r", ".sqrb"));
        squadreplay::sqrb::write(bundle, &sqrb_file.0).unwrap();
        let sqrb_size = std::fs::metadata(&sqrb_file.0).map(|m| m.len()).unwrap_or(0);
        group.throughput(Throughput::Bytes(sqrb_size));

        group.bench_with_input(
            BenchmarkId::from_parameter(info.label),
            &sqrb_file.0,
            |b, path| {
                b.iter(|| black_box(squadreplay::sqrb::read(path).unwrap()));
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// sqrj_write — JSON serialization
// ---------------------------------------------------------------------------

fn bench_sqrj_write(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("sqrj_write");
    group.sample_size(10).measurement_time(Duration::from_secs(20));

    for info in &fixtures {
        let bundle = parse_fixture_bundle(info.file, true);
        group.throughput(Throughput::Bytes(info.size));

        group.bench_with_input(
            BenchmarkId::from_parameter(info.label),
            &bundle,
            |b, bundle| {
                b.iter(|| {
                    let tmp = TempFile(unique_path("bench-sqrj-w", ".sqrj.json"));
                    black_box(squadreplay::sqrj::write(bundle, &tmp.0).unwrap());
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// sqrj_read — JSON deserialization
// ---------------------------------------------------------------------------

fn bench_sqrj_read(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("sqrj_read");
    group.sample_size(10).measurement_time(Duration::from_secs(15));

    for info in &fixtures {
        let bundle = parse_fixture_bundle(info.file, true);

        // Pre-write once so we measure only the read path.
        let sqrj_file = TempFile(unique_path("bench-sqrj-r", ".sqrj.json"));
        squadreplay::sqrj::write(bundle, &sqrj_file.0).unwrap();
        let sqrj_size = std::fs::metadata(&sqrj_file.0).map(|m| m.len()).unwrap_or(0);
        group.throughput(Throughput::Bytes(sqrj_size));

        group.bench_with_input(
            BenchmarkId::from_parameter(info.label),
            &sqrj_file.0,
            |b, path| {
                b.iter(|| black_box(squadreplay::sqrj::read(path).unwrap()));
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// compat_projection — compatibility JSON generation (CPU-only)
// ---------------------------------------------------------------------------

fn bench_compat_projection(c: &mut Criterion) {
    skip_if_missing!();

    let fixtures = available_fixtures();
    let mut group = c.benchmark_group("compat_projection");
    group.sample_size(20).measurement_time(Duration::from_secs(10));

    for info in &fixtures {
        let bundle = parse_fixture_bundle(info.file, true);

        group.bench_with_input(
            BenchmarkId::from_parameter(info.label),
            &bundle,
            |b, bundle| {
                b.iter(|| {
                    let compat = squadreplay::compat::from_bundle(bundle);
                    black_box(serde_json::to_vec(&compat).unwrap())
                });
            },
        );
    }

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
        bench_sqrb_write,
        bench_sqrb_read,
        bench_sqrj_write,
        bench_sqrj_read,
        bench_compat_projection
}
criterion_main!(benches);
