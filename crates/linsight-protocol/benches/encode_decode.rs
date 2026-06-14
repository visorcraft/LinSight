// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use linsight_core::{Category, Reading, Sample, SensorId, SensorKind, Unit};
use linsight_protocol::{ClientMsg, RequestOp, ServerMsg};

fn sample() -> ServerMsg {
    ServerMsg::Sample(Sample {
        sensor: SensorId::new("cpu.util"),
        ts_micros: 1_700_000_000_000_000,
        reading: Reading::Scalar(42.5),
    })
}

fn sensor_list() -> ServerMsg {
    ServerMsg::SensorList(vec![linsight_protocol::SensorInfo {
        id: SensorId::new("cpu.util"),
        display_name: "CPU utilization".into(),
        unit: Unit::Percent,
        kind: SensorKind::Scalar,
        category: Category::Cpu,
        native_rate_hz: 1.0,
        min: Some(0.0),
        max: Some(100.0),
        device_id: None,
        plugin_id: "com.visorcraft.linsight.cpu".into(),
        device_key: Some("cpu:0".into()),
        device_label: Some("CPU".into()),
        tags: vec!["cpu".into()],
    }])
}

fn subscribe() -> ClientMsg {
    ClientMsg::Subscribe {
        sensors: vec![SensorId::new("cpu.util"), SensorId::new("mem.used")],
        rate_hz: Some(2.0),
    }
}

fn request() -> ClientMsg {
    ClientMsg::Request { req_id: 42, op: RequestOp::GetHardware }
}

fn bench_encode(c: &mut Criterion) {
    let sample_bytes = postcard::to_allocvec(&sample()).unwrap();
    let list_bytes = postcard::to_allocvec(&sensor_list()).unwrap();
    let sub_bytes = postcard::to_allocvec(&subscribe()).unwrap();
    let req_bytes = postcard::to_allocvec(&request()).unwrap();

    c.bench_function("encode ServerMsg::Sample", |b| {
        b.iter(|| postcard::to_allocvec(black_box(&sample())).unwrap())
    });
    c.bench_function("encode ServerMsg::SensorList(1)", |b| {
        b.iter(|| postcard::to_allocvec(black_box(&sensor_list())).unwrap())
    });
    c.bench_function("encode ClientMsg::Subscribe(2)", |b| {
        b.iter(|| postcard::to_allocvec(black_box(&subscribe())).unwrap())
    });
    c.bench_function("encode ClientMsg::Request", |b| {
        b.iter(|| postcard::to_allocvec(black_box(&request())).unwrap())
    });

    c.bench_function("decode ServerMsg::Sample", |b| {
        b.iter(|| postcard::from_bytes::<ServerMsg>(black_box(&sample_bytes)).unwrap())
    });
    c.bench_function("decode ServerMsg::SensorList(1)", |b| {
        b.iter(|| postcard::from_bytes::<ServerMsg>(black_box(&list_bytes)).unwrap())
    });
    c.bench_function("decode ClientMsg::Subscribe(2)", |b| {
        b.iter(|| postcard::from_bytes::<ClientMsg>(black_box(&sub_bytes)).unwrap())
    });
    c.bench_function("decode ClientMsg::Request", |b| {
        b.iter(|| postcard::from_bytes::<ClientMsg>(black_box(&req_bytes)).unwrap())
    });

    c.bench_function("sample wire size", |b| {
        b.iter(|| black_box(postcard::to_allocvec(&sample()).unwrap().len()))
    });
}

criterion_group!(benches, bench_encode);
criterion_main!(benches);
