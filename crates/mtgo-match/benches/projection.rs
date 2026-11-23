//! Projection speed on the largest fixture (spec 7.8). `cargo bench -p mtgo-match`.
//! "rebuild" folds every event then finalizes once; "live" finalizes after
//! every event, as the tracker does. Results are recorded, never gated.

use criterion::{Criterion, criterion_group, criterion_main};
use fixture_tools::{load_fixture, prepared_events};
use mtgo_match::Projector;

fn projection(c: &mut Criterion) {
    let fixture = load_fixture("draft_hobbit");
    let events = prepared_events(&fixture);
    let now = fixture.meta.now_ms();

    c.bench_function("rebuild draft_hobbit", |b| {
        b.iter(|| {
            let mut projector = Projector::new(vec![]);
            for event in &events {
                projector.apply(event);
            }
            projector.finalize(now);
            projector.views().len()
        })
    });
    c.bench_function("live draft_hobbit (finalize per event)", |b| {
        b.iter(|| {
            let mut projector = Projector::new(vec![]);
            for event in &events {
                projector.apply(event);
                projector.finalize(event.at_ms);
            }
            projector.views().len()
        })
    });
}

criterion_group!(benches, projection);
criterion_main!(benches);
