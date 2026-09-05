//! Nœud de périphérique : un par périphérique, dont le **rôle** change à chaud.
//!
//! Le rôle est remplacé par une boîte aux lettres SPSC lue au début de chaque
//! `process` : le graphe n'est pas recompilé, l'identifiant du nœud et ses liens
//! sont conservés (F-20, F-21). L'ancien rôle est renvoyé au fil de gestion pour
//! libération hors temps réel.

use conduit_backend::DeviceDirection;
use conduit_core::asyncport::{GraphReader, GraphWriter};
use conduit_core::node::{Node, NodeIo, PortSpec, ProcessContext};
use conduit_core::ring::{RingBuffer, RingConsumer, RingProducer};
use conduit_core::types::SampleRate;

/// Rôle courant d'un nœud de périphérique.
pub enum Role {
    /// Périphérique absent ou fermé : silence.
    Suspended,
    /// Capture asynchrone : les trames arrivent par un port asynchrone.
    AsyncCapture(GraphReader),
    /// Rendu asynchrone : les trames partent par un port asynchrone.
    AsyncRender(GraphWriter),
    /// Capture pilote : le rappel du périphérique dépose les trames du cycle courant.
    DriverCapture(RingConsumer<f32>),
    /// Rendu pilote : le rappel du périphérique relève les trames du cycle courant.
    DriverRender(RingProducer<f32>),
}

impl core::fmt::Debug for Role {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Role::Suspended => "Suspended",
            Role::AsyncCapture(_) => "AsyncCapture",
            Role::AsyncRender(_) => "AsyncRender",
            Role::DriverCapture(_) => "DriverCapture",
            Role::DriverRender(_) => "DriverRender",
        })
    }
}

impl Role {
    /// Nom court.
    pub fn name(&self) -> &'static str {
        match self {
            Role::Suspended => "suspended",
            Role::AsyncCapture(_) | Role::AsyncRender(_) => "async",
            Role::DriverCapture(_) | Role::DriverRender(_) => "driver",
        }
    }
}

/// Côté gestion : change le rôle et libère les anciens.
#[derive(Debug)]
pub struct DeviceNodeControl {
    mailbox: RingProducer<Role>,
    retired: RingConsumer<Role>,
}

impl DeviceNodeControl {
    /// Envoie un nouveau rôle. Libère d'abord les rôles retirés. Retourne `Err(role)`
    /// si la boîte est pleine (le fil audio ne tourne pas).
    #[allow(clippy::result_large_err)] // le rôle refusé est rendu à l'appelant, par conception
    pub fn set_role(&mut self, role: Role) -> Result<(), Role> {
        self.collect();
        self.mailbox.push(role)
    }

    /// Libère les rôles retirés par le fil audio. Retourne le nombre libéré.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while self.retired.pop().is_some() {
            n += 1;
        }
        n
    }

    /// Rôles envoyés pas encore adoptés.
    pub fn pending(&self) -> usize {
        self.mailbox.len()
    }
}

/// Nœud de périphérique.
pub struct DeviceNode {
    direction: DeviceDirection,
    channels: usize,
    role: Role,
    mailbox: RingConsumer<Role>,
    retired: RingProducer<Role>,
    /// Réserve locale si la file de retour est pleine (pré-allouée).
    overflow: Vec<Role>,
    scratch: Box<[f32]>,
    /// Trames servies avec du signal réel (diagnostic).
    active_cycles: u64,
}

impl core::fmt::Debug for DeviceNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeviceNode")
            .field("direction", &self.direction)
            .field("channels", &self.channels)
            .field("role", &self.role)
            .finish()
    }
}

impl DeviceNode {
    /// Capacité de la boîte aux lettres.
    pub const MAILBOX_CAPACITY: usize = 8;

    /// Crée un nœud suspendu et sa commande.
    pub fn new(
        direction: DeviceDirection,
        channels: usize,
        max_frames: usize,
    ) -> (Self, DeviceNodeControl) {
        let (mp, mc) = RingBuffer::with_capacity(Self::MAILBOX_CAPACITY);
        let (rp, rc) = RingBuffer::with_capacity(Self::MAILBOX_CAPACITY + 2);
        let node = Self {
            direction,
            channels,
            role: Role::Suspended,
            mailbox: mc,
            retired: rp,
            overflow: Vec::with_capacity(Self::MAILBOX_CAPACITY + 2),
            scratch: vec![0.0; channels * max_frames].into_boxed_slice(),
            active_cycles: 0,
        };
        (
            node,
            DeviceNodeControl {
                mailbox: mp,
                retired: rc,
            },
        )
    }

    /// Sens.
    pub fn direction(&self) -> DeviceDirection {
        self.direction
    }

    /// Rôle courant (côté fil audio).
    pub fn role(&self) -> &Role {
        &self.role
    }

    fn adopt_roles(&mut self) {
        while let Some(new) = self.mailbox.pop() {
            let old = core::mem::replace(&mut self.role, new);
            if let Err(old) = self.retired.push(old) {
                if self.overflow.len() < self.overflow.capacity() {
                    self.overflow.push(old);
                } else {
                    core::mem::forget(old);
                }
            }
        }
        while let Some(r) = self.overflow.pop() {
            if let Err(r) = self.retired.push(r) {
                self.overflow.push(r);
                break;
            }
        }
    }
}

impl Node for DeviceNode {
    fn type_name(&self) -> &'static str {
        match self.direction {
            DeviceDirection::Capture => "device-capture",
            DeviceDirection::Render => "device-render",
        }
    }

    fn inputs(&self) -> Vec<PortSpec> {
        match self.direction {
            DeviceDirection::Capture => vec![],
            DeviceDirection::Render => PortSpec::layout(self.channels),
        }
    }

    fn outputs(&self) -> Vec<PortSpec> {
        match self.direction {
            DeviceDirection::Capture => PortSpec::layout(self.channels),
            DeviceDirection::Render => vec![],
        }
    }

    fn prepare(&mut self, _: SampleRate, max_frames: usize) {
        if self.scratch.len() < self.channels * max_frames {
            self.scratch = vec![0.0; self.channels * max_frames].into_boxed_slice();
        }
    }

    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        self.adopt_roles();
        let frames = io.frames();
        let ch = self.channels;
        let n = frames * ch;
        match self.direction {
            DeviceDirection::Capture => {
                let real = match &mut self.role {
                    Role::AsyncCapture(reader) => reader.read(&mut self.scratch[..n]),
                    Role::DriverCapture(ring) => {
                        let got = ring.read(&mut self.scratch[..n]);
                        self.scratch[got..n].fill(0.0);
                        got == n
                    }
                    _ => {
                        io.silence_outputs();
                        return;
                    }
                };
                if real {
                    self.active_cycles += 1;
                }
                let (_, mut outs) = io.split();
                for (c, out) in outs.iter_mut().enumerate() {
                    for (i, o) in out.iter_mut().enumerate() {
                        *o = self.scratch[i * ch + c];
                    }
                }
            }
            DeviceDirection::Render => {
                if matches!(
                    self.role,
                    Role::Suspended | Role::AsyncCapture(_) | Role::DriverCapture(_)
                ) {
                    return;
                }
                let (ins, _) = io.split();
                for c in 0..ch {
                    let src = ins.get(c);
                    for (i, &x) in src.iter().enumerate() {
                        self.scratch[i * ch + c] = x;
                    }
                }
                match &mut self.role {
                    Role::AsyncRender(writer) => {
                        writer.write(&self.scratch[..n]);
                    }
                    Role::DriverRender(ring) => {
                        ring.write(&self.scratch[..n]);
                    }
                    _ => {}
                }
                self.active_cycles += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_core::asyncport::{input_port, output_port, AsyncPortConfig};

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            frames,
            max_frames: frames,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        }
    }

    #[test]
    fn suspended_capture_is_silent_and_render_ignores() {
        let (mut cap, _c) = DeviceNode::new(DeviceDirection::Capture, 2, 8);
        assert_eq!(cap.type_name(), "device-capture");
        assert_eq!(cap.outputs().len(), 2);
        assert!(cap.inputs().is_empty());
        let mut outs: Vec<Box<[f32]>> = vec![vec![1.0; 8].into(), vec![1.0; 8].into()];
        cap.process(&ctx(8), &mut NodeIo::new(&[], &[], &mut outs, 8));
        assert!(outs.iter().all(|o| o.iter().all(|&x| x == 0.0)));
        let (mut ren, _c) = DeviceNode::new(DeviceDirection::Render, 1, 8);
        assert_eq!(ren.inputs().len(), 1);
        let ins: Vec<Box<[f32]>> = vec![vec![1.0; 8].into()];
        ren.process(&ctx(8), &mut NodeIo::new(&ins, &[true], &mut [], 8));
        assert_eq!(ren.role().name(), "suspended");
        assert!(format!("{ren:?}").contains("Suspended"));
    }

    #[test]
    fn role_switch_driver_capture_and_render() {
        let (mut cap, mut ctl) = DeviceNode::new(DeviceDirection::Capture, 2, 4);
        let (mut p, c) = RingBuffer::with_capacity(16);
        ctl.set_role(Role::DriverCapture(c)).unwrap();
        assert_eq!(ctl.pending(), 1);
        p.write(&[1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0]);
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 4].into(), vec![0.0; 4].into()];
        cap.process(&ctx(4), &mut NodeIo::new(&[], &[], &mut outs, 4));
        assert_eq!(&outs[0][..], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&outs[1][..], &[10.0, 20.0, 30.0, 40.0]);
        assert_eq!(cap.role().name(), "driver");
        // Données partielles : le reste est du silence.
        p.write(&[5.0, 50.0]);
        cap.process(&ctx(4), &mut NodeIo::new(&[], &[], &mut outs, 4));
        assert_eq!(&outs[0][..], &[5.0, 0.0, 0.0, 0.0]);
        // Retour en suspendu : l'ancien rôle est retiré et collecté côté gestion
        // (le rôle initial a déjà été collecté par set_role).
        ctl.set_role(Role::Suspended).unwrap();
        cap.process(&ctx(4), &mut NodeIo::new(&[], &[], &mut outs, 4));
        assert_eq!(ctl.collect(), 1);
        assert_eq!(ctl.collect(), 0);

        let (mut ren, mut rctl) = DeviceNode::new(DeviceDirection::Render, 2, 4);
        let (p2, mut c2) = RingBuffer::with_capacity(16);
        rctl.set_role(Role::DriverRender(p2)).unwrap();
        let ins: Vec<Box<[f32]>> = vec![vec![1.0, 2.0, 3.0, 4.0].into(), vec![9.0; 4].into()];
        ren.process(&ctx(4), &mut NodeIo::new(&ins, &[true, true], &mut [], 4));
        let mut got = [0.0; 8];
        assert_eq!(c2.read(&mut got), 8);
        assert_eq!(got, [1.0, 9.0, 2.0, 9.0, 3.0, 9.0, 4.0, 9.0]);
        assert_eq!(ren.role().name(), "driver");
    }

    #[test]
    fn async_roles_go_through_ports() {
        let cfg = AsyncPortConfig::new(1, SampleRate::HZ_48000, 64);
        let (mut writer, reader) = input_port(&cfg);
        let (mut cap, mut ctl) = DeviceNode::new(DeviceDirection::Capture, 1, 64);
        ctl.set_role(Role::AsyncCapture(reader)).unwrap();
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 64].into()];
        for _ in 0..8 {
            writer.write(&[0.5; 64]);
        }
        for _ in 0..3 {
            cap.process(&ctx(64), &mut NodeIo::new(&[], &[], &mut outs, 64));
        }
        assert!((outs[0][63] - 0.5).abs() < 1e-3);
        assert_eq!(cap.role().name(), "async");

        let (gw, mut dr) = output_port(&cfg);
        let (mut ren, mut rctl) = DeviceNode::new(DeviceDirection::Render, 1, 64);
        rctl.set_role(Role::AsyncRender(gw)).unwrap();
        let ins: Vec<Box<[f32]>> = vec![vec![0.25; 64].into()];
        for _ in 0..8 {
            ren.process(&ctx(64), &mut NodeIo::new(&ins, &[true], &mut [], 64));
        }
        let mut out = [0.0; 64];
        assert!(dr.read(&mut out));
        assert!((out[63] - 0.25).abs() < 1e-3);
        assert_eq!(rctl.collect(), 1, "le rôle initial Suspended a été retiré");
        rctl.set_role(Role::Suspended).unwrap();
        ren.process(&ctx(64), &mut NodeIo::new(&ins, &[true], &mut [], 64));
        assert_eq!(rctl.collect(), 1);
    }

    #[test]
    fn mailbox_full_returns_role() {
        let (_node, mut ctl) = DeviceNode::new(DeviceDirection::Capture, 1, 4);
        for _ in 0..DeviceNode::MAILBOX_CAPACITY {
            ctl.set_role(Role::Suspended).unwrap();
        }
        assert!(matches!(
            ctl.set_role(Role::Suspended),
            Err(Role::Suspended)
        ));
    }

    #[test]
    fn prepare_grows_scratch() {
        let (mut n, _c) = DeviceNode::new(DeviceDirection::Render, 2, 4);
        n.prepare(SampleRate::HZ_48000, 256);
        assert_eq!(n.scratch.len(), 512);
        assert_eq!(n.direction(), DeviceDirection::Render);
        assert_eq!(n.type_name(), "device-render");
    }
}
