//! Decode speed on the largest fixture (spec 7.8). `cargo bench -p mtgo-protocol`.
//! Results are recorded in the PR, never gated.

use std::hint::black_box;

use chrono::NaiveDate;
use criterion::{Criterion, criterion_group, criterion_main};
use mtgo_protocol::{FileContext, decode_meta_message, parse_game_management, split_complete};

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/logs/draft_hobbit.log"
    ))
    .expect("the draft_hobbit fixture")
}

fn decode(c: &mut Criterion) {
    let bytes = fixture();
    let events = split_complete(&bytes);
    let day = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
    let metas: Vec<_> = events
        .iter()
        .filter_map(|event| parse_game_management(&event.text).meta_message)
        .collect();

    c.bench_function("split draft_hobbit into events", |b| {
        b.iter(|| split_complete(black_box(&bytes)).len())
    });
    c.bench_function("prepare every event (classify, decode, date)", |b| {
        b.iter(|| {
            let mut ctx = FileContext::new(1, day, chrono_tz::UTC, None);
            events.iter().filter_map(|raw| ctx.prepare(raw)).count()
        })
    });
    c.bench_function("decode every MetaMessage", |b| {
        b.iter(|| {
            metas
                .iter()
                .filter_map(|meta| decode_meta_message(meta))
                .count()
        })
    });
}

criterion_group!(benches, decode);
criterion_main!(benches);
