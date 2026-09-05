//! Preuve que le cycle de traitement n'alloue pas (M0-27).

use conduit_core::graph::GraphBuilder;
use conduit_core::node::{Node, NodeIo, PortSpec, ProcessContext};
use conduit_core::slot::GraphSlot;
use conduit_core::types::{Db, Quantum, SampleRate};
use conduit_core::Executor;
use conduit_testing::{assert_no_alloc, count_allocs, GuardAllocator};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

struct Osc {
    phase: f32,
    step: f32,
}

impl Node for Osc {
    fn type_name(&self) -> &'static str {
        "osc"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::stereo()
    }
    fn prepare(&mut self, sample_rate: SampleRate, _: usize) {
        self.step = 440.0 / sample_rate.as_f64() as f32;
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let (_ins, mut outs) = io.split();
        for i in 0..io_frames(&mut outs) {
            let v = (self.phase * core::f32::consts::TAU).sin();
            self.phase = (self.phase + self.step).fract();
            for o in outs.iter_mut() {
                o[i] = v;
            }
        }
    }
}

fn io_frames(outs: &mut conduit_core::node::Outputs<'_>) -> usize {
    outs.get(0).len()
}

struct Mix(usize);

impl Node for Mix {
    fn type_name(&self) -> &'static str {
        "mix"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.0)
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::stereo()
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let n = io.num_inputs();
        for out in 0..2 {
            let (i0, o) = io.in_out(out % n, out);
            o.copy_from_slice(i0);
        }
        for i in 2..n {
            let (inp, o) = io.in_out(i, i % 2);
            for (o, x) in o.iter_mut().zip(inp) {
                *o += x;
            }
        }
    }
}

/// Nœud fautif : alloue dans `process`.
struct Leaky;

impl Node for Leaky {
    fn type_name(&self) -> &'static str {
        "leaky"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn process(&mut self, _: &ProcessContext, _: &mut NodeIo<'_>) {
        let v: Vec<f32> = Vec::with_capacity(4);
        std::hint::black_box(v);
    }
}

fn build_graph() -> GraphBuilder {
    let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(256).unwrap());
    let mix = b.add_node(Box::new(Mix(6)), "mix");
    for k in 0..3 {
        let osc = b.add_node(
            Box::new(Osc {
                phase: 0.0,
                step: 0.0,
            }),
            format!("osc{k}"),
        );
        let oi = b.node(osc).unwrap().clone();
        let mi = b.node(mix).unwrap().clone();
        let l = b.add_link(oi.output(0), mi.input(2 * k)).unwrap();
        b.link_gain(l).unwrap().set_db(Db::new(-6.0));
        b.add_link(oi.output(1), mi.input(2 * k + 1)).unwrap();
    }
    b
}

#[test]
fn cycle_does_not_allocate() {
    let mut b = build_graph();
    let (mut slot, mut ex) = GraphSlot::new();
    slot.publish(&mut b).unwrap();
    // Le premier cycle installe le graphe (déplacements, pas d'allocation).
    let (r, n) = count_allocs(|| ex.run(256));
    assert!(r.graph_swapped);
    assert_eq!(n, 0, "l'installation d'un graphe n'alloue pas");
    // Changement de gain en cours de route : rampe sans allocation.
    let gain = b
        .nodes()
        .next()
        .map(|n| b.node_gain(n.id).unwrap())
        .unwrap();
    gain.set_db(Db::new(-12.0));
    for _ in 0..100 {
        let r = assert_no_alloc(|| ex.run(256));
        assert_eq!(r.nodes_run, 4);
    }
    // Nouvelle version publiée pendant l'exécution : adoption sans allocation.
    let extra = b.add_node(
        Box::new(Osc {
            phase: 0.0,
            step: 0.0,
        }),
        "extra",
    );
    let _ = extra;
    slot.publish(&mut b).unwrap();
    let r = assert_no_alloc(|| ex.run(256));
    assert!(r.graph_swapped);
    assert_eq!(r.nodes_run, 5);
    assert_eq!(slot.collect(), 1);
}

#[test]
fn standalone_executor_cycle_does_not_allocate() {
    let mut b = build_graph();
    let mut ex = Executor::standalone(b.compile());
    for frames in [256, 128, 1, 256] {
        assert_no_alloc(|| ex.run(frames));
    }
    assert_eq!(ex.position(), 256 + 128 + 1 + 256);
}

#[test]
fn allocating_node_is_caught() {
    let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(64).unwrap());
    b.add_node(Box::new(Leaky), "leaky");
    let mut ex = Executor::standalone(b.compile());
    let (_, n) = count_allocs(|| ex.run(64));
    assert!(n >= 1, "le nœud fautif doit être détecté");
}

#[test]
#[should_panic(expected = "interdite dans une section temps réel")]
fn allocating_node_panics_in_forbid_mode() {
    let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(64).unwrap());
    b.add_node(Box::new(Leaky), "leaky");
    let mut ex = Executor::standalone(b.compile());
    assert_no_alloc(|| ex.run(64));
}

#[test]
fn utility_nodes_do_not_allocate() {
    use conduit_core::nodes::{
        ChannelAdapter, EqBand, EqualizerNode, MeterNode, MixerNode, NoiseColor, NoiseNode,
        SineNode, SplitterNode,
    };
    let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(256).unwrap());
    let sine = b.add_node(Box::new(SineNode::new(440.0, 0.5, 2)), "sine");
    let noise = b.add_node(Box::new(NoiseNode::new(NoiseColor::Pink, 0.1, 2)), "noise");
    let mix = b.add_node(Box::new(MixerNode::new(2, 2)), "mix");
    let eq = b.add_node(
        Box::new(EqualizerNode::new(
            2,
            &[
                EqBand::low_shelf(100.0, 3.0),
                EqBand::peaking(1000.0, 1.0, -6.0),
                EqBand::high_pass(40.0, 0.7),
            ],
        )),
        "eq",
    );
    let meter = b.add_node(Box::new(MeterNode::new(2)), "meter");
    let split = b.add_node(Box::new(SplitterNode::new(2, 2)), "split");
    let mono = b.add_node(Box::new(ChannelAdapter::auto(2, 1)), "mono");
    let info = |b: &GraphBuilder, id| b.node(id).unwrap().clone();
    for ch in 0..2 {
        b.add_link(info(&b, sine).output(ch), info(&b, mix).input(ch))
            .unwrap();
        b.add_link(info(&b, noise).output(ch), info(&b, mix).input(2 + ch))
            .unwrap();
        b.add_link(info(&b, mix).output(ch), info(&b, eq).input(ch))
            .unwrap();
        b.add_link(info(&b, eq).output(ch), info(&b, meter).input(ch))
            .unwrap();
        b.add_link(info(&b, meter).output(ch), info(&b, split).input(ch))
            .unwrap();
        b.add_link(info(&b, split).output(2 + ch), info(&b, mono).input(ch))
            .unwrap();
    }
    let eq_ctrl = b.node_gain(eq).unwrap();
    let mut ex = Executor::standalone(b.compile());
    for i in 0..50 {
        if i == 10 {
            eq_ctrl.set_db(Db::new(-3.0));
        }
        let r = assert_no_alloc(|| ex.run(256));
        assert_eq!(r.nodes_run, 7);
    }
}
