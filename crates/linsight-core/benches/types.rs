// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use linsight_core::{Reading, Sample, SensorId};

fn bench_serde(c: &mut Criterion) {
    let sample = Sample {
        sensor: SensorId::new("cpu.util"),
        ts_micros: 1_700_000_000_000_000,
        reading: Reading::Scalar(42.5),
    };
    let sample_json = serde_json::to_string(&sample).unwrap();

    c.bench_function("json encode Sample", |b| {
        b.iter(|| serde_json::to_string(black_box(&sample)).unwrap())
    });
    c.bench_function("json decode Sample", |b| {
        b.iter(|| serde_json::from_str::<Sample>(black_box(&sample_json)).unwrap())
    });

    c.bench_function("clone Reading", |b| b.iter(|| black_box(sample.reading.clone())));
}

criterion_group!(benches, bench_serde);
criterion_main!(benches);
