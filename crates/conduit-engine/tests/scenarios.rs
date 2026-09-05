//! Scénarios d'intégration engine + backend null (M0-61, M0-64, M0-65, M0-67, M0-68).

use std::time::Duration;

use conduit_backend::null::{NullBackend, NullDeviceSpec, Signal};
use conduit_backend::{Backend, CableSpec, DeviceDirection, DeviceId};
use conduit_core::graph::{Direction, GraphError};
use conduit_core::types::{ChannelCount, Db, Quantum, SampleRate};
use conduit_engine::{
    Command, DriverChoice, DriverStatus, Engine, EngineConfig, EngineError, EngineEvent,
    InternalKind, NodeKey, NodeState, Notification, Reply,
};

const Q: usize = 256;

fn config() -> EngineConfig {
    EngineConfig {
        quantum: Quantum::new(Q).unwrap(),
        ..Default::default()
    }
}

fn engine_with(spk: bool, mic: bool) -> (Engine, NullBackend, Option<DeviceId>, Option<DeviceId>) {
    let null = NullBackend::new();
    let spk_id = spk.then(|| null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, Q)));
    let mic_id = mic.then(|| null.add_device(NullDeviceSpec::capture("Micro").layout(1, 240)));
    let engine = Engine::new(Box::new(null.clone()), config()).unwrap();
    (engine, null, spk_id, mic_id)
}

fn zero_crossings(s: &[f32]) -> usize {
    s.windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count()
}

fn node_of(engine: &Engine, id: &DeviceId) -> conduit_core::graph::NodeId {
    engine
        .node_by_key(&NodeKey::device("null", id.clone()))
        .expect("nœud du périphérique")
}

#[test]
fn enumeration_creates_nodes_and_default_render_drives() {
    let (mut engine, _null, spk, mic) = engine_with(true, true);
    let nodes = engine.nodes();
    assert_eq!(nodes.len(), 2);
    let spk_node = nodes
        .iter()
        .find(|n| n.key == NodeKey::device("null", spk.clone().unwrap()))
        .unwrap();
    assert_eq!(spk_node.state, NodeState::Driver);
    assert_eq!(spk_node.inputs.len(), 2);
    assert!(spk_node.outputs.is_empty());
    assert_eq!(spk_node.type_name, "device-render");
    let mic_node = nodes
        .iter()
        .find(|n| n.key == NodeKey::device("null", mic.clone().unwrap()))
        .unwrap();
    assert_eq!(mic_node.state, NodeState::Active);
    assert_eq!(mic_node.outputs.len(), 1);
    assert_eq!(
        engine.driver_status(),
        DriverStatus::Device { id: spk.unwrap() }
    );
    let status = engine.status();
    assert_eq!(status.nodes, 2);
    assert_eq!(status.backend, "null");
    assert_eq!(status.devices.len(), 2);
    let notes = engine.tick();
    assert!(notes
        .iter()
        .any(|n| matches!(n, Notification::DriverChanged(DriverStatus::Device { .. }))));
    assert!(notes
        .iter()
        .any(|n| matches!(n, Notification::NodeAdded(_))));
    assert!(format!("{engine:?}").contains("null"));
}

#[test]
fn internal_sine_reaches_driver_output() {
    let (mut engine, null, spk, _) = engine_with(true, false);
    let spk = spk.unwrap();
    let sine = engine
        .execute(Command::AddInternal {
            name: "test".into(),
            kind: InternalKind::Sine {
                frequency: 1000.0,
                amplitude: 0.5,
                channels: 2,
            },
        })
        .unwrap();
    let Reply::Node(sine) = sine else { panic!() };
    assert_eq!(sine.key, NodeKey::internal("test"));
    let spk_node = node_of(&engine, &spk);
    for ch in 0..2 {
        let src = engine.graph().node(sine.id).unwrap().output(ch);
        let dst = engine.graph().node(spk_node).unwrap().input(ch);
        engine.execute(Command::Link { src, dst }).unwrap();
    }
    null.advance(Duration::from_secs(1));
    let rec = null.device(&spk).unwrap().take_recorded();
    let left: Vec<f32> = rec.chunks(2).map(|f| f[0]).collect();
    assert!(left.len() >= 48_000);
    let tail = &left[4800..];
    let zc = zero_crossings(tail) as f64 / 2.0 / (tail.len() as f64 / 48_000.0);
    assert!((zc - 1000.0).abs() < 5.0, "fréquence {zc}");
    let peak = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.5).abs() < 0.01);
    let status = engine.status();
    assert!(status.cycles >= 187);
    assert_eq!(status.xruns, 0);
    assert!(status.timing.count > 0);
    let notes = engine.tick();
    assert!(notes.iter().any(|n| matches!(
        n,
        Notification::Rt(EngineEvent::DriverStarted { device: Some(_) })
    )));
}

#[test]
fn async_capture_is_resampled_into_driver_output() {
    let (mut engine, null, spk, mic) = engine_with(true, true);
    let (spk, mic) = (spk.unwrap(), mic.unwrap());
    null.device(&mic).unwrap().set_signal(Signal::Sine {
        frequency: 440.0,
        amplitude: 0.8,
    });
    let mic_node = node_of(&engine, &mic);
    let spk_node = node_of(&engine, &spk);
    engine
        .execute(Command::LinkByName {
            src_node: mic_node,
            src_port: "MONO".into(),
            dst_node: spk_node,
            dst_port: "FL".into(),
        })
        .unwrap();
    null.advance(Duration::from_secs(4));
    let rec = null.device(&spk).unwrap().take_recorded();
    let left: Vec<f32> = rec.chunks(2).map(|f| f[0]).collect();
    let right: Vec<f32> = rec.chunks(2).map(|f| f[1]).collect();
    let tail = &left[left.len() - 48_000..];
    let f = zero_crossings(tail) as f64 / 2.0 / 1.0;
    assert!((f - 440.0).abs() < 3.0, "fréquence {f}");
    let peak = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.8).abs() < 0.02, "crête {peak}");
    assert!(right.iter().all(|&x| x == 0.0), "FR non lié = silence");
    let status = engine.status();
    let mic_status = status.devices.iter().find(|d| d.id == mic).unwrap();
    assert_eq!(mic_status.state, NodeState::Active);
    assert_eq!(mic_status.underruns, 0);
    assert_eq!(mic_status.overruns, 0);
    assert!(mic_status.locked, "DLL verrouillée");
    assert!((mic_status.ratio - 1.0).abs() < 1e-3);
}

#[test]
fn cycle_is_refused_with_explicit_message() {
    let (mut engine, _null, _, _) = engine_with(false, false);
    let Reply::Node(a) = engine
        .execute(Command::AddInternal {
            name: "a".into(),
            kind: InternalKind::Equalizer {
                channels: 1,
                bands: vec![],
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let Reply::Node(b) = engine
        .execute(Command::AddInternal {
            name: "b".into(),
            kind: InternalKind::Meter { channels: 1 },
        })
        .unwrap()
    else {
        panic!()
    };
    engine
        .execute(Command::Link {
            src: a.id.output_port(0),
            dst: b.id.input_port(0),
        })
        .unwrap();
    let err = engine
        .execute(Command::Link {
            src: b.id.output_port(0),
            dst: a.id.input_port(0),
        })
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Graph(GraphError::WouldCycle { .. })
    ));
    assert!(err.to_string().contains("boucle"));
    assert!(matches!(
        engine.execute(Command::AddInternal {
            name: "a".into(),
            kind: InternalKind::Silence { channels: 1 }
        }),
        Err(EngineError::DuplicateName(_))
    ));
    assert!(matches!(
        engine.execute(Command::LinkByName {
            src_node: a.id,
            src_port: "FL".into(),
            dst_node: b.id,
            dst_port: "MONO".into()
        }),
        Err(EngineError::UnknownPortName { .. })
    ));
}

trait PortHelpers {
    fn output_port(self, i: u16) -> conduit_core::graph::PortId;
    fn input_port(self, i: u16) -> conduit_core::graph::PortId;
}

impl PortHelpers for conduit_core::graph::NodeId {
    fn output_port(self, i: u16) -> conduit_core::graph::PortId {
        conduit_core::graph::PortId::new(self, Direction::Output, i)
    }
    fn input_port(self, i: u16) -> conduit_core::graph::PortId {
        conduit_core::graph::PortId::new(self, Direction::Input, i)
    }
}

#[test]
fn gains_mute_and_meter_via_commands() {
    let (mut engine, null, spk, _) = engine_with(true, false);
    let spk = spk.unwrap();
    let Reply::Node(sine) = engine
        .execute(Command::AddInternal {
            name: "s".into(),
            kind: InternalKind::Sine {
                frequency: 1000.0,
                amplitude: 1.0,
                channels: 1,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let Reply::Node(meter) = engine
        .execute(Command::AddInternal {
            name: "m".into(),
            kind: InternalKind::Meter { channels: 1 },
        })
        .unwrap()
    else {
        panic!()
    };
    let Reply::Link(l1) = engine
        .execute(Command::Link {
            src: sine.id.output_port(0),
            dst: meter.id.input_port(0),
        })
        .unwrap()
    else {
        panic!()
    };
    let spk_node = node_of(&engine, &spk);
    engine
        .execute(Command::Link {
            src: meter.id.output_port(0),
            dst: spk_node.input_port(0),
        })
        .unwrap();
    engine
        .execute(Command::SetLinkGain {
            link: l1.link.id,
            gain_db: Some(Db::new(-6.0)),
            muted: None,
        })
        .unwrap();
    null.advance(Duration::from_secs(1));
    let Reply::Meter(r) = engine
        .execute(Command::ReadMeter { node: meter.id })
        .unwrap()
    else {
        panic!()
    };
    assert!((r[0].peak - 0.501).abs() < 0.01, "crête {}", r[0].peak);
    // Muet du nœud sinus : la sortie tombe à zéro.
    engine
        .execute(Command::SetNodeGain {
            node: sine.id,
            gain_db: None,
            muted: Some(true),
        })
        .unwrap();
    // Décroissance de crête 20 dB/s : après 4 s, −80 dB.
    null.advance(Duration::from_secs(4));
    let Reply::Meter(r) = engine
        .execute(Command::ReadMeter { node: meter.id })
        .unwrap()
    else {
        panic!()
    };
    assert!(r[0].peak < 1e-3, "crête après muet {}", r[0].peak);
    let Reply::Nodes(nodes) = engine.execute(Command::Nodes).unwrap() else {
        panic!()
    };
    assert!(nodes.iter().find(|n| n.id == sine.id).unwrap().muted);
    let Reply::Links(links) = engine.execute(Command::Links).unwrap() else {
        panic!()
    };
    assert!(
        (links
            .iter()
            .find(|l| l.link.id == l1.link.id)
            .unwrap()
            .gain_db
            .get()
            + 6.0)
            .abs()
            < 0.01
    );
    // Paramètres.
    engine
        .execute(Command::SetParam {
            node: sine.id,
            name: "frequency".into(),
            value: 2000.0,
        })
        .unwrap();
    assert!(matches!(
        engine.execute(Command::SetParam {
            node: sine.id,
            name: "frequency".into(),
            value: -1.0
        }),
        Err(EngineError::InvalidParam { .. })
    ));
    assert!(matches!(
        engine.execute(Command::SetParam {
            node: meter.id,
            name: "amplitude".into(),
            value: 0.5
        }),
        Err(EngineError::InvalidParam { .. })
    ));
    assert!(engine
        .execute(Command::ReadMeter { node: sine.id })
        .is_err());
    engine
        .execute(Command::SetLabel {
            node: sine.id,
            label: "Générateur".into(),
        })
        .unwrap();
    let Reply::Node(n) = engine.execute(Command::Ports { node: sine.id }).unwrap() else {
        panic!()
    };
    assert_eq!(n.label, "Générateur");
    engine
        .execute(Command::Unlink { link: l1.link.id })
        .unwrap();
    engine
        .execute(Command::RemoveNode { node: sine.id })
        .unwrap();
    assert!(engine.graph().node(sine.id).is_err());
    assert!(matches!(
        engine.execute(Command::RemoveNode { node: spk_node }),
        Err(EngineError::DevicePresent(_))
    ));
    engine.execute(Command::ResetXruns).unwrap();
    assert_eq!(engine.status().xruns, 0);
}

#[test]
fn unplug_suspends_and_replug_resumes_with_identical_graph() {
    let (mut engine, null, spk, mic) = engine_with(true, true);
    let (spk, mic) = (spk.unwrap(), mic.unwrap());
    null.device(&mic).unwrap().set_signal(Signal::Dc(0.5));
    let mic_node = node_of(&engine, &mic);
    let spk_node = node_of(&engine, &spk);
    engine
        .execute(Command::Link {
            src: mic_node.output_port(0),
            dst: spk_node.input_port(1),
        })
        .unwrap();
    null.advance(Duration::from_secs(1));
    let before: Vec<_> = engine.links();
    let nodes_before = engine.nodes();
    // Débranchement.
    null.remove_device(&mic);
    let notes = engine.tick();
    assert!(notes.contains(&Notification::NodeStateChanged {
        id: mic_node,
        state: NodeState::Suspended
    }));
    assert_eq!(engine.links(), before, "les liens sont conservés");
    assert_eq!(engine.nodes().len(), nodes_before.len());
    assert_eq!(
        engine.node_descriptor(mic_node).unwrap().state,
        NodeState::Suspended
    );
    null.device(&spk).unwrap().take_recorded();
    null.advance(Duration::from_secs(1));
    let rec = null.device(&spk).unwrap().take_recorded();
    let right: Vec<f32> = rec.chunks(2).map(|f| f[1]).collect();
    assert!(
        right[right.len() - 4800..].iter().all(|&x| x == 0.0),
        "silence pendant la suspension"
    );
    // Réapparition, même identifiant.
    let again = null.add_device(NullDeviceSpec::capture("Micro").layout(1, 240));
    assert_eq!(again, mic);
    null.device(&mic).unwrap().set_signal(Signal::Dc(0.5));
    let notes = engine.tick();
    assert!(notes.contains(&Notification::NodeStateChanged {
        id: mic_node,
        state: NodeState::Active
    }));
    assert_eq!(node_of(&engine, &mic), mic_node, "même nœud");
    assert_eq!(
        engine.links(),
        before,
        "graphe identique après réapparition"
    );
    null.advance(Duration::from_secs(2));
    let rec = null.device(&spk).unwrap().take_recorded();
    let right: Vec<f32> = rec.chunks(2).map(|f| f[1]).collect();
    assert!(
        (right[right.len() - 1] - 0.5).abs() < 1e-3,
        "l'audio est revenu : {}",
        right[right.len() - 1]
    );
    // Un périphérique absent peut être oublié explicitement.
    null.remove_device(&mic);
    engine.tick();
    engine
        .execute(Command::RemoveNode { node: mic_node })
        .unwrap();
    assert!(engine.links().is_empty());
}

#[test]
fn driver_switch_keeps_audio_flowing() {
    let null = NullBackend::new();
    let a = null.add_device(NullDeviceSpec::render("A").layout(1, Q));
    let b = null.add_device(NullDeviceSpec::render("B").layout(1, 300).drift(500.0));
    let mut engine = Engine::new(Box::new(null.clone()), config()).unwrap();
    assert_eq!(
        engine.driver_status(),
        DriverStatus::Device { id: a.clone() }
    );
    let Reply::Node(sine) = engine
        .execute(Command::AddInternal {
            name: "s".into(),
            kind: InternalKind::Sine {
                frequency: 1000.0,
                amplitude: 0.5,
                channels: 1,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let (an, bn) = (node_of(&engine, &a), node_of(&engine, &b));
    engine
        .execute(Command::Link {
            src: sine.id.output_port(0),
            dst: an.input_port(0),
        })
        .unwrap();
    engine
        .execute(Command::Link {
            src: sine.id.output_port(0),
            dst: bn.input_port(0),
        })
        .unwrap();
    null.advance(Duration::from_secs(3));
    let ra = null.device(&a).unwrap().take_recorded();
    let rb = null.device(&b).unwrap().take_recorded();
    let peak = |v: &[f32]| v[v.len() / 2..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((peak(&ra) - 0.5).abs() < 0.02, "A (pilote) : {}", peak(&ra));
    assert!(
        (peak(&rb) - 0.5).abs() < 0.02,
        "B (asynchrone) : {}",
        peak(&rb)
    );
    let cycles_before = engine.status().cycles;
    // Bascule : B devient pilote, A asynchrone.
    engine
        .execute(Command::SetDriver {
            choice: DriverChoice::Device { id: b.clone() },
        })
        .unwrap();
    assert_eq!(
        engine.driver_status(),
        DriverStatus::Device { id: b.clone() }
    );
    assert_eq!(engine.node_descriptor(bn).unwrap().state, NodeState::Driver);
    assert_eq!(engine.node_descriptor(an).unwrap().state, NodeState::Active);
    let notes = engine.tick();
    assert!(notes
        .iter()
        .any(|n| *n == Notification::DriverChanged(DriverStatus::Device { id: b.clone() })));
    null.advance(Duration::from_secs(3));
    assert!(engine.status().cycles > cycles_before);
    let ra = null.device(&a).unwrap().take_recorded();
    let rb = null.device(&b).unwrap().take_recorded();
    assert!(
        (peak(&ra) - 0.5).abs() < 0.02,
        "A (asynchrone) : {}",
        peak(&ra)
    );
    assert!((peak(&rb) - 0.5).abs() < 0.02, "B (pilote) : {}", peak(&rb));
    // Silence lors de la bascule : au plus deux quanta de zéros consécutifs sur B.
    let zeros = rb.iter().take_while(|&&x| x == 0.0).count();
    assert!(zeros <= 2 * Q + 300, "silence de {zeros} trames");
    // Choix invalide.
    assert!(matches!(
        engine.execute(Command::SetDriver {
            choice: DriverChoice::Device {
                id: "absent".into()
            }
        }),
        Err(EngineError::Backend(_))
    ));
    // Le pilote disparaît : repli automatique sur A (auto) ou interne.
    engine
        .execute(Command::SetDriver {
            choice: DriverChoice::Auto,
        })
        .unwrap();
    null.remove_device(&b);
    engine.tick();
    assert_eq!(
        engine.driver_status(),
        DriverStatus::Device { id: a.clone() }
    );
    null.remove_device(&a);
    engine.tick();
    assert_eq!(engine.driver_status(), DriverStatus::Internal);
}

#[test]
fn internal_clock_drives_when_no_device() {
    let (mut engine, _null, _, _) = engine_with(false, false);
    assert_eq!(engine.driver_status(), DriverStatus::Internal);
    let Reply::Node(sine) = engine
        .execute(Command::AddInternal {
            name: "s".into(),
            kind: InternalKind::Sine {
                frequency: 1000.0,
                amplitude: 0.7,
                channels: 1,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let Reply::Node(meter) = engine
        .execute(Command::AddInternal {
            name: "m".into(),
            kind: InternalKind::Meter { channels: 1 },
        })
        .unwrap()
    else {
        panic!()
    };
    engine
        .execute(Command::Link {
            src: sine.id.output_port(0),
            dst: meter.id.input_port(0),
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let Reply::Meter(r) = engine
        .execute(Command::ReadMeter { node: meter.id })
        .unwrap()
    else {
        panic!()
    };
    assert!((r[0].peak - 0.7).abs() < 0.02, "crête {}", r[0].peak);
    let status = engine.status();
    assert!(status.cycles > 30, "cycles {}", status.cycles);
    assert_eq!(status.driver, DriverStatus::Internal);
    // Bascule explicite vers interne puis retour auto : idempotent.
    engine
        .execute(Command::SetDriver {
            choice: DriverChoice::Internal,
        })
        .unwrap();
    engine
        .execute(Command::SetDriver {
            choice: DriverChoice::Auto,
        })
        .unwrap();
    assert_eq!(engine.driver_status(), DriverStatus::Internal);
    engine.shutdown();
    assert_eq!(engine.driver_status(), DriverStatus::None);
}

#[test]
fn xruns_are_counted_and_reported() {
    let (mut engine, null, spk, mic) = engine_with(true, true);
    let (spk, mic) = (spk.unwrap(), mic.unwrap());
    engine
        .execute(Command::Link {
            src: node_of(&engine, &mic).output_port(0),
            dst: node_of(&engine, &spk).input_port(0),
        })
        .unwrap();
    null.advance(Duration::from_secs(2));
    engine.tick();
    assert_eq!(engine.status().xruns, 0);
    // Le micro cesse de fournir : on arrête son flux (comme un pilote bloqué).
    let mic_dev = null.device(&mic).unwrap();
    let _ = mic_dev;
    // Simule un blocage : on désactive puis réactive en retirant/réajoutant sans
    // passer par le moteur n'est pas possible ; on force la sous-alimentation en
    // vidant le tampon via une capture beaucoup plus lente (dérive énorme).
    null.remove_device(&mic);
    engine.tick();
    let slow = null.add_device(
        NullDeviceSpec::capture("Micro")
            .layout(1, 240)
            .drift(-50_000.0),
    );
    assert_eq!(slow, mic);
    engine.tick();
    null.advance(Duration::from_secs(3));
    let notes = engine.tick();
    let status = engine.status();
    let d = status.devices.iter().find(|d| d.id == mic).unwrap();
    assert!(d.underruns > 0, "sous-alimentations attendues : {d:?}");
    assert!(status.xruns > 0);
    assert!(notes.iter().any(|n| matches!(
        n,
        Notification::Rt(EngineEvent::DeviceXrun { underrun: true, .. })
    )));
    engine.execute(Command::ResetXruns).unwrap();
    assert_eq!(
        engine
            .status()
            .devices
            .iter()
            .find(|d| d.id == mic)
            .unwrap()
            .underruns,
        0
    );
}

#[test]
fn cables_create_device_nodes() {
    let (mut engine, null, _, _) = engine_with(false, false);
    let Reply::Cable(c) = engine
        .execute(Command::CableAdd {
            spec: CableSpec {
                name: Some("Musique".into()),
                channels: ChannelCount::STEREO,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(c.name, "Musique");
    let nodes = engine.nodes();
    assert_eq!(nodes.len(), 2, "un nœud par côté du câble");
    assert!(nodes
        .iter()
        .any(|n| n.device.as_ref().map(|d| d.direction) == Some(DeviceDirection::Render)));
    let Reply::Cables(list) = engine.execute(Command::CableList).unwrap() else {
        panic!()
    };
    assert_eq!(list.len(), 1);
    engine
        .execute(Command::CableRename {
            id: c.id,
            name: "Jeu".into(),
        })
        .unwrap();
    let Reply::Cable(c6) = engine
        .execute(Command::CableSetChannels {
            id: c.id,
            channels: ChannelCount::new(6).unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(c6.channels.get(), 6);
    engine.tick();
    let render = engine
        .node_by_key(&NodeKey::device("null", c6.render.clone()))
        .unwrap();
    assert_eq!(
        engine.graph().node(render).unwrap().inputs.len(),
        6,
        "nœud recréé avec 6 entrées"
    );
    engine.execute(Command::CableRemove { id: c.id }).unwrap();
    engine.tick();
    assert!(engine
        .nodes()
        .iter()
        .all(|n| n.state == NodeState::Suspended));
    let notes = engine.tick();
    let _ = notes;
    assert_eq!(null.devices().unwrap().len(), 0);
    // Câble par défaut : « Conduit 2 » (le numéro 1 a été utilisé).
    let Reply::Cable(c2) = engine
        .execute(Command::CableAdd {
            spec: CableSpec::default(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(c2.name.starts_with("Conduit"));
}

#[test]
fn driver_at_unsupported_rate_is_refused_with_advice() {
    let null = NullBackend::new();
    let dev = null.add_device(
        NullDeviceSpec::render("44k")
            .layout(2, 441)
            .rate(SampleRate::HZ_44100),
    );
    let mut engine = Engine::new(Box::new(null.clone()), config()).unwrap();
    assert_eq!(
        engine.driver_status(),
        DriverStatus::Internal,
        "un périphérique 44,1 kHz ne pilote pas un graphe 48 kHz"
    );
    let err = engine
        .execute(Command::SetDriver {
            choice: DriverChoice::Device { id: dev.clone() },
        })
        .unwrap_err();
    assert!(matches!(err, EngineError::CannotDrive { .. }));
    assert!(err.to_string().contains("44"));
    // Mais il fonctionne en asynchrone (rééchantillonné).
    assert_eq!(
        engine
            .node_descriptor(node_of(&engine, &dev))
            .unwrap()
            .state,
        NodeState::Active
    );
    let d = engine
        .status()
        .devices
        .into_iter()
        .find(|d| d.id == dev)
        .unwrap();
    assert!((d.ratio - 44_100.0 / 48_000.0).abs() < 0.01);
}
