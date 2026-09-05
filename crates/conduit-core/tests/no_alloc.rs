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
