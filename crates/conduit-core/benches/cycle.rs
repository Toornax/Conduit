//! Bancs de mesure : cycle de traitement et rééchantillonneur (M0-43).

use conduit_core::dsp::{ResampleQuality, Resampler};
use conduit_core::graph::GraphBuilder;
use conduit_core::nodes::{EqBand, EqualizerNode, MeterNode, MixerNode, SineNode};
use conduit_core::types::{Quantum, SampleRate};
use conduit_core::Executor;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// Graphe « 10 nœuds stéréo » de SPEC §5.6 : 4 sinus → mixeur 4 bus → EQ 3 bandes →
/// VU-mètre, plus 3 sinus libres.
fn build(quantum: usize) -> Executor {
    let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(quantum).unwrap());
    let mix = b.add_node(Box::new(MixerNode::new(4, 2)), "mix");
    for k in 0..4 {
        let s = b.add_node(
            Box::new(SineNode::new(220.0 * (k + 1) as f32, 0.2, 2)),
            "sine",
        );
        for ch in 0..2 {
            let src = b.node(s).unwrap().output(ch);
            let dst = b.node(mix).unwrap().input(2 * k + ch);
            b.add_link(src, dst).unwrap();
        }
    }
    let eq = b.add_node(
        Box::new(EqualizerNode::new(
            2,
            &[
                EqBand::low_shelf(120.0, 2.0),
                EqBand::peaking(2000.0, 1.0, -3.0),
                EqBand::high_shelf(8000.0, 1.5),
            ],
        )),
        "eq",
    );
    let meter = b.add_node(Box::new(MeterNode::new(2)), "meter");
    for ch in 0..2 {
        b.add_link(
            b.node(mix).unwrap().output(ch),
            b.node(eq).unwrap().input(ch),
        )
        .unwrap();
        b.add_link(
            b.node(eq).unwrap().output(ch),
            b.node(meter).unwrap().input(ch),
        )
        .unwrap();
    }
    for _ in 0..3 {
        b.add_node(Box::new(SineNode::new(1000.0, 0.1, 2)), "free");
    }
    Executor::standalone(b.compile())
}

fn bench_cycle(c: &mut Criterion) {
    let mut g = c.benchmark_group("cycle");
    for &q in &[64usize, 256, 1024] {
        let mut ex = build(q);
        g.throughput(Throughput::Elements(q as u64));
        g.bench_with_input(BenchmarkId::new("graph_10_nodes_stereo", q), &q, |b, &q| {
            b.iter(|| ex.run(q));
        });
    }
    g.finish();
}

fn bench_resampler(c: &mut Criterion) {
    let mut g = c.benchmark_group("resampler");
    let block = 256;
    for quality in [
        ResampleQuality::Fast,
        ResampleQuality::Normal,
        ResampleQuality::Best,
    ] {
        let mut r = Resampler::new(2, 48_000.0 / 44_100.0, quality, block);
        let input: Vec<f32> = (0..block * 2).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut out = vec![0.0f32; block * 2];
        g.throughput(Throughput::Elements(block as u64));
        g.bench_function(
            BenchmarkId::new("stereo_44k1_to_48k", format!("{quality:?}")),
            |b| {
                b.iter(|| {
                    let need = r.input_needed(block);
                    if need > 0 {
                        r.push(&input[..need.min(block) * 2]);
                    }
                    r.pull(&mut out)
                });
            },
        );
    }
    g.finish();
}

criterion_group!(benches, bench_cycle, bench_resampler);
criterion_main!(benches);
