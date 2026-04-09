//! Classification micro-benchmarks.
//!
//! These exercise the hot-path classification functions that run on every
//! property event during parsing. They are gated behind the `bench-internals`
//! Cargo feature.
//!
//! Run with: `cargo bench --features bench-internals --bench classify`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use squadreplay::bench_internals::{
    ClassifyFlags, classify_deployable_event_type, infer_component_type_name, infer_group_leaf,
    is_deployable_primary_type, is_helicopter_type, is_soldier_type, is_vehicle_type,
    normalize_type,
};

// ---------------------------------------------------------------------------
// Representative type names from real replays
// ---------------------------------------------------------------------------

const SOLDIER_TYPES: &[&str] = &[
    "BP_Soldier_RUS",
    "BP_Soldier_CAF_Rifleman",
    "BP_Soldier_USA_MedicKit",
    "BP_Soldier_MEA_Pilot",
];

const VEHICLE_TYPES: &[&str] = &[
    "BP_BTR80_RUS",
    "BP_MRAP_M1151_USA",
    "BP_Logi_Ural375_RUS",
    "BP_BRDM2_RUS",
    "BP_T72B3_RUS",
];

const HELICOPTER_TYPES: &[&str] = &[
    "BP_Heli_MI8_RUS",
    "BP_Heli_UH60_USA",
    "BP_Heli_MRH90_AUS",
    "BP_Heli_SA330_MEA",
];

const DEPLOYABLE_TYPES: &[&str] = &[
    "BP_FOBRadio_RUS",
    "BP_HAB_RUS",
    "BP_Deployable_ATGM_RUS",
    "BP_Deployable_MortarTube_RUS",
];

const NON_MATCHING_TYPES: &[&str] = &[
    "SQGameState",
    "SQPlayerState",
    "GameNetworkManager",
    "WorldSettings",
    "SQTeam",
];

const DEPLOYABLE_EVENT_TYPES: &[&str] = &[
    "BP_FOBRadio_RUS",
    "BP_HAB_RUS",
    "BP_Deployable_ATGM_RUS",
    "BP_Deployable_MortarTube_RUS",
    "BP_Deployable_HMG_RUS",
    "BP_Logi_Ural375_RUS",
    "SQPlayerState",
];

const COMPONENT_GROUP_PATHS: &[&str] = &[
    "/Script/Squad.SQVehicleComponent",
    "/Script/Squad.SQSoldierMovementComponent",
    "/Game/Vehicles/BTR80/BP_BTR80_Turret.BP_BTR80_Turret_C",
    "/Script/Squad.SQPlayerStateComponent",
];

const NORMALIZE_INPUTS: &[&str] = &[
    "BP_Soldier_RUS.BP_Soldier_RUS_C",
    "BP_BTR80_RUS",
    "SQGameState",
    "BP_Heli_MI8_RUS.BP_Heli_MI8_RUS_C",
];

const GROUP_LEAF_PATHS: &[&str] = &[
    "/Script/Squad.SQVehicleComponent",
    "/Game/Vehicles/BTR80/BP_BTR80_Turret.BP_BTR80_Turret_C",
    "SQGameState",
    "/Game/Maps/Fallujah/Gameplay_Layers/BP_Fallujah_Seed.BP_Fallujah_Seed_C",
];

// ---------------------------------------------------------------------------
// ClassifyFlags::from_group_leaf — composite classification
// ---------------------------------------------------------------------------

fn bench_classify_flags(c: &mut Criterion) {
    let mut group = c.benchmark_group("classify_flags");

    let all_types: Vec<(&str, &[&str])> = vec![
        ("soldier", SOLDIER_TYPES),
        ("vehicle", VEHICLE_TYPES),
        ("helicopter", HELICOPTER_TYPES),
        ("deployable", DEPLOYABLE_TYPES),
        ("non_matching", NON_MATCHING_TYPES),
    ];

    for (category, names) in &all_types {
        group.bench_with_input(
            BenchmarkId::from_parameter(category),
            names,
            |b, names| {
                b.iter(|| {
                    for name in *names {
                        black_box(ClassifyFlags::from_group_leaf(black_box(name)));
                    }
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Individual is_*_type checks
// ---------------------------------------------------------------------------

fn bench_is_type_checks(c: &mut Criterion) {
    let mut group = c.benchmark_group("is_type_checks");

    // Soldier
    group.bench_function("is_soldier_type/positive", |b| {
        b.iter(|| {
            for name in SOLDIER_TYPES {
                black_box(is_soldier_type(black_box(name)));
            }
        });
    });
    group.bench_function("is_soldier_type/negative", |b| {
        b.iter(|| {
            for name in NON_MATCHING_TYPES {
                black_box(is_soldier_type(black_box(name)));
            }
        });
    });

    // Vehicle
    group.bench_function("is_vehicle_type/positive", |b| {
        b.iter(|| {
            for name in VEHICLE_TYPES {
                black_box(is_vehicle_type(black_box(name)));
            }
        });
    });
    group.bench_function("is_vehicle_type/negative", |b| {
        b.iter(|| {
            for name in NON_MATCHING_TYPES {
                black_box(is_vehicle_type(black_box(name)));
            }
        });
    });

    // Helicopter
    group.bench_function("is_helicopter_type/positive", |b| {
        b.iter(|| {
            for name in HELICOPTER_TYPES {
                black_box(is_helicopter_type(black_box(name)));
            }
        });
    });
    group.bench_function("is_helicopter_type/negative", |b| {
        b.iter(|| {
            for name in NON_MATCHING_TYPES {
                black_box(is_helicopter_type(black_box(name)));
            }
        });
    });

    // Deployable
    group.bench_function("is_deployable_primary_type/positive", |b| {
        b.iter(|| {
            for name in DEPLOYABLE_TYPES {
                black_box(is_deployable_primary_type(black_box(name)));
            }
        });
    });
    group.bench_function("is_deployable_primary_type/negative", |b| {
        b.iter(|| {
            for name in NON_MATCHING_TYPES {
                black_box(is_deployable_primary_type(black_box(name)));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn bench_classify_utilities(c: &mut Criterion) {
    let mut group = c.benchmark_group("classify_utilities");

    // classify_deployable_event_type
    group.bench_function("classify_deployable_event_type", |b| {
        b.iter(|| {
            for name in DEPLOYABLE_EVENT_TYPES {
                black_box(classify_deployable_event_type(black_box(name)));
            }
        });
    });

    // infer_component_type_name
    group.bench_function("infer_component_type_name", |b| {
        b.iter(|| {
            for path in COMPONENT_GROUP_PATHS {
                black_box(infer_component_type_name(black_box(path), None));
            }
        });
    });

    group.bench_function("infer_component_type_name/with_hint", |b| {
        b.iter(|| {
            for path in COMPONENT_GROUP_PATHS {
                black_box(infer_component_type_name(
                    black_box(path),
                    Some("SomeHintPath"),
                ));
            }
        });
    });

    // normalize_type
    group.bench_function("normalize_type/dot_path", |b| {
        b.iter(|| {
            for input in NORMALIZE_INPUTS {
                black_box(normalize_type(black_box(input)));
            }
        });
    });

    // infer_group_leaf
    group.bench_function("infer_group_leaf", |b| {
        b.iter(|| {
            for path in GROUP_LEAF_PATHS {
                black_box(infer_group_leaf(black_box(path)));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion wiring
// ---------------------------------------------------------------------------

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets =
        bench_classify_flags,
        bench_is_type_checks,
        bench_classify_utilities
}
criterion_main!(benches);
