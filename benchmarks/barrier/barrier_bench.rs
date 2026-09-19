//! What the in-memory barrier costs a request (ADR-0015 D13, duck plan M5).
//!
//! The end-to-end benchmark measures a laptop with the load generator sharing
//! its cores, so a change of a few percent disappears into the noise. This
//! measures the added work directly: the same response body, built from a
//! sealed secret and from a plain one.

use core_crypto::Barrier;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use naughtian_kallisto::server::responses;

const SECRET: &str = r#"{"username":"payment","password":"duck-fixture-value"}"#;
const CREATED: &str = "2026-09-17T00:00:00Z";

fn bench_serving_one_secret(c: &mut Criterion) {
    let barrier = Barrier::new().expect("a barrier");
    let sealed = barrier.seal(SECRET.as_bytes()).expect("sealing");

    let mut group = c.benchmark_group("barrier");

    // What M3 did: the cleartext was sitting in the table, and the request only
    // had to build a body around it.
    group.bench_function("build_body_from_cleartext", |b| {
        b.iter(|| black_box(responses::kv_data(black_box(SECRET), 12, CREATED)));
    });

    // What M5 does: open the one secret asked for into this thread's buffer,
    // build the body inside the callback, wipe the buffer.
    group.bench_function("open_then_build_body", |b| {
        b.iter(|| {
            let body = barrier
                .with_plaintext(black_box(&sealed), |text| {
                    responses::kv_data(text, 12, CREATED)
                })
                .expect("opening");
            black_box(body)
        });
    });

    // The decryption alone, with no body building, so the two numbers above can
    // be read against something.
    group.bench_function("open_only", |b| {
        b.iter(|| {
            black_box(
                barrier
                    .with_plaintext(black_box(&sealed), str::len)
                    .expect("opening"),
            )
        });
    });

    group.finish();
}

/// Snapshot build is not on the read path — it runs twice a minute — but a file
/// with a few hundred secrets should still not stall the refresh thread.
fn bench_sealing_a_whole_file(c: &mut Criterion) {
    let secrets: Vec<String> = (0..64)
        .map(|i| format!(r#"{{"user":"u{i}","password":"p{i}-{}"}}"#, "x".repeat(24)))
        .collect();

    c.bench_function("barrier/seal_64_secrets", |b| {
        b.iter(|| {
            let barrier = Barrier::new().expect("a barrier");
            for secret in &secrets {
                black_box(barrier.seal(secret.as_bytes()).expect("sealing"));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_serving_one_secret,
    bench_sealing_a_whole_file
);
criterion_main!(benches);
