//! Périphérique simulé.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use conduit_core::dsp::XorShift32;
use conduit_core::types::SampleRate;

use crate::cable::CableId;
use crate::device::{
    AudioCallback, BackendError, ClockInfo, DeviceDirection, DeviceId, DeviceInfo, StreamFormat,
    StreamIo,
};

/// Description d'un périphérique simulé à créer.
#[derive(Debug, Clone)]
pub struct NullDeviceSpec {
    /// Nom lisible.
    pub name: String,
    /// Sens.
    pub direction: DeviceDirection,
    /// Canaux.
    pub channels: usize,
    /// Fréquence native.
    pub sample_rate: SampleRate,
    /// Autres fréquences acceptées.
    pub sample_rates: Vec<SampleRate>,
    /// Trames par rappel.
    pub block: usize,
    /// Dérive d'horloge en parties par million (+ = plus rapide que le nominal).
    pub drift_ppm: f64,
    /// Gigue des rappels, fraction de la période (0 = aucune, 0,5 = ± 50 %).
    pub jitter: f64,
}

impl Default for NullDeviceSpec {
    fn default() -> Self {
        Self {
            name: "Périphérique simulé".into(),
            direction: DeviceDirection::Render,
            channels: 2,
            sample_rate: SampleRate::HZ_48000,
            sample_rates: Vec::new(),
            block: 256,
            drift_ppm: 0.0,
            jitter: 0.0,
        }
    }
}

impl NullDeviceSpec {
    /// Périphérique de rendu stéréo 48 kHz.
    pub fn render(name: &str) -> Self {
        Self {
            name: name.into(),
            direction: DeviceDirection::Render,
            ..Default::default()
        }
    }

    /// Périphérique de capture stéréo 48 kHz.
    pub fn capture(name: &str) -> Self {
        Self {
            name: name.into(),
            direction: DeviceDirection::Capture,
            ..Default::default()
        }
    }

    /// Fixe la dérive.
    pub fn drift(mut self, ppm: f64) -> Self {
        self.drift_ppm = ppm;
        self
    }

    /// Fixe la gigue.
    pub fn jitter(mut self, fraction: f64) -> Self {
        self.jitter = fraction;
        self
    }

    /// Fixe canaux et bloc.
    pub fn layout(mut self, channels: usize, block: usize) -> Self {
        self.channels = channels;
        self.block = block;
        self
    }

    /// Fixe la fréquence native.
    pub fn rate(mut self, rate: SampleRate) -> Self {
        self.sample_rate = rate;
        self
    }
}

/// Générateur libre : reçoit le tampon entrelacé, la position et le nombre de canaux.
pub type SignalFn = Box<dyn FnMut(&mut [f32], u64, usize) + Send>;

/// Signal produit par un périphérique de capture simulé.
pub enum Signal {
    /// Silence.
    Silence,
    /// Sinus, même signal sur tous les canaux.
    Sine {
        /// Fréquence (Hz, dans l'horloge du périphérique).
        frequency: f64,
        /// Amplitude crête.
        amplitude: f32,
    },
    /// Valeur constante.
    Dc(f32),
    /// Générateur libre.
    Custom(SignalFn),
}

impl core::fmt::Debug for Signal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Signal::Silence => f.write_str("Silence"),
            Signal::Sine {
                frequency,
                amplitude,
            } => write!(f, "Sine({frequency} Hz, {amplitude})"),
            Signal::Dc(v) => write!(f, "Dc({v})"),
            Signal::Custom(_) => f.write_str("Custom"),
        }
    }
}

/// File de boucle locale partagée par les deux côtés d'un câble simulé.
#[derive(Debug)]
pub(super) struct Loopback {
    channels: usize,
    max_frames: usize,
    queue: Mutex<VecDeque<f32>>,
}

impl Loopback {
    pub(super) fn new(channels: usize, max_frames: usize) -> Self {
        Self {
            channels,
            max_frames,
            queue: Mutex::new(VecDeque::new()),
        }
    }

    fn push(&self, interleaved: &[f32]) {
        let mut q = self.queue.lock().unwrap();
        q.extend(interleaved.iter().copied());
        let max = self.max_frames * self.channels;
        while q.len() > max {
            q.pop_front();
        }
    }

    fn pop_into(&self, out: &mut [f32]) {
        let mut q = self.queue.lock().unwrap();
        for x in out.iter_mut() {
            *x = q.pop_front().unwrap_or(0.0);
        }
    }
}

/// Flux ouvert sur un périphérique simulé.
pub(super) struct Stream {
    pub(super) format: StreamFormat,
    pub(super) callback: AudioCallback,
    pub(super) running: bool,
    pub(super) position: u64,
    pub(super) next_tick_ns: u64,
    pub(super) last_tick_ns: u64,
    period_ns: f64,
    rng: XorShift32,
    buffer: Vec<f32>,
}

struct State {
    info: DeviceInfo,
    present: bool,
    signal: Signal,
    recorded: Vec<f32>,
    max_record_frames: usize,
    callbacks: u64,
    stream: Option<Arc<Mutex<Stream>>>,
}

/// Périphérique simulé.
pub struct NullDevice {
    id: DeviceId,
    spec: NullDeviceSpec,
    loopback: Option<Arc<Loopback>>,
    /// Horloge virtuelle du backend (ns), partagée.
    clock: Arc<AtomicU64>,
    state: Mutex<State>,
}

impl core::fmt::Debug for NullDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = self.state.lock().unwrap();
        f.debug_struct("NullDevice")
            .field("id", &self.id)
            .field("present", &s.present)
            .field("callbacks", &s.callbacks)
            .field("open", &s.stream.is_some())
            .finish()
    }
}

impl NullDevice {
    pub(super) fn new(
        id: DeviceId,
        spec: NullDeviceSpec,
        cable: Option<CableId>,
        loopback: Option<Arc<Loopback>>,
        clock: Arc<AtomicU64>,
    ) -> Self {
        let info = DeviceInfo {
            id: id.clone(),
            name: spec.name.clone(),
            direction: spec.direction,
            channels: spec.channels,
            sample_rate: spec.sample_rate,
            sample_rates: spec.sample_rates.clone(),
            default_block: spec.block,
            is_default: false,
            cable,
        };
        Self {
            id,
            spec,
            loopback,
            clock,
            state: Mutex::new(State {
                info,
                present: true,
                signal: Signal::Silence,
                recorded: Vec::new(),
                max_record_frames: 48_000 * 10,
                callbacks: 0,
                stream: None,
            }),
        }
    }

    /// Identifiant.
    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    /// Description (copie).
    pub fn info(&self) -> DeviceInfo {
        self.state.lock().unwrap().info.clone()
    }

    /// Vrai si le périphérique est encore branché.
    pub fn is_present(&self) -> bool {
        self.state.lock().unwrap().present
    }

    /// Nombre de rappels exécutés.
    pub fn callbacks(&self) -> u64 {
        self.state.lock().unwrap().callbacks
    }

    /// Injecte un signal (périphérique de capture).
    pub fn set_signal(&self, signal: Signal) {
        self.state.lock().unwrap().signal = signal;
    }

    /// Borne l'enregistrement (périphérique de rendu), en trames.
    pub fn set_max_record_frames(&self, frames: usize) {
        self.state.lock().unwrap().max_record_frames = frames;
    }

    /// Récupère et vide ce que le rendu a reçu depuis le dernier appel (entrelacé).
    pub fn take_recorded(&self) -> Vec<f32> {
        std::mem::take(&mut self.state.lock().unwrap().recorded)
    }

    pub(super) fn rename(&self, name: &str) {
        self.state.lock().unwrap().info.name = name.to_string();
    }

    pub(super) fn disconnect(&self) {
        let mut s = self.state.lock().unwrap();
        s.present = false;
        if let Some(st) = &s.stream {
            st.lock().unwrap().running = false;
        }
    }

    pub(super) fn close(&self) {
        self.state.lock().unwrap().stream = None;
    }

    pub(super) fn stream(&self) -> Option<Arc<Mutex<Stream>>> {
        self.state.lock().unwrap().stream.clone()
    }

    /// Temps virtuel courant.
    pub(super) fn now_ns(&self) -> u64 {
        self.clock.load(Ordering::Relaxed)
    }

    pub(super) fn open(
        &self,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<Arc<Mutex<Stream>>, BackendError> {
        let now_ns = self.now_ns();
        let mut s = self.state.lock().unwrap();
        if !s.present {
            return Err(BackendError::Disconnected(self.id.clone()));
        }
        if s.stream.is_some() {
            return Err(BackendError::Busy(self.id.clone()));
        }
        if !s.info.supports_rate(format.sample_rate) {
            return Err(BackendError::UnsupportedFormat {
                device: self.id.clone(),
                reason: format!(
                    "fréquence {} non supportée (natif : {})",
                    format.sample_rate, s.info.sample_rate
                ),
            });
        }
        if format.channels != s.info.channels {
            return Err(BackendError::UnsupportedFormat {
                device: self.id.clone(),
                reason: format!(
                    "{} canaux demandés, {} disponibles",
                    format.channels, s.info.channels
                ),
            });
        }
        let block = format.block_frames.max(1);
        let format = StreamFormat {
            block_frames: block,
            ..format
        };
        let rate = format.sample_rate.as_f64() * (1.0 + self.spec.drift_ppm * 1e-6);
        let period_ns = block as f64 / rate * 1e9;
        let stream = Arc::new(Mutex::new(Stream {
            format,
            callback,
            running: false,
            position: 0,
            next_tick_ns: now_ns,
            last_tick_ns: now_ns,
            period_ns,
            rng: XorShift32::new(self.id.as_str().len() as u32 + 17),
            buffer: vec![0.0; block * format.channels],
        }));
        s.stream = Some(Arc::clone(&stream));
        Ok(stream)
    }

    /// Exécute un rappel dû à l'instant `tick_ns`.
    pub(super) fn run_callback(&self, stream: &Arc<Mutex<Stream>>, tick_ns: u64) {
        let mut st = stream.lock().unwrap();
        let st = &mut *st;
        let frames = st.format.block_frames;
        let ch = st.format.channels;
        let clock = ClockInfo {
            position: st.position,
            timestamp_ns: tick_ns,
            frames,
        };
        match self.spec.direction {
            DeviceDirection::Capture => {
                {
                    let mut s = self.state.lock().unwrap();
                    match &self.loopback {
                        Some(lb) => lb.pop_into(&mut st.buffer),
                        None => fill_signal(
                            &mut s.signal,
                            &mut st.buffer,
                            st.position,
                            ch,
                            st.format.sample_rate.as_f64(),
                        ),
                    }
                }
                let mut io = StreamIo {
                    input: Some(&st.buffer),
                    output: None,
                };
                (st.callback)(&mut io, &clock);
            }
            DeviceDirection::Render => {
                let mut io = StreamIo {
                    input: None,
                    output: Some(&mut st.buffer),
                };
                (st.callback)(&mut io, &clock);
                let mut s = self.state.lock().unwrap();
                if let Some(lb) = &self.loopback {
                    lb.push(&st.buffer);
                }
                let max = s.max_record_frames * ch;
                if s.recorded.len() < max {
                    let room = max - s.recorded.len();
                    s.recorded
                        .extend_from_slice(&st.buffer[..st.buffer.len().min(room)]);
                }
            }
        }
        self.state.lock().unwrap().callbacks += 1;
        st.position += frames as u64;
        st.last_tick_ns = tick_ns;
        // Prochain tick : période nominale + gigue non cumulative.
        let base = tick_ns as f64 + st.period_ns;
        let jitter = if self.spec.jitter > 0.0 {
            f64::from(st.rng.next_f32()) * self.spec.jitter * st.period_ns
        } else {
            0.0
        };
        st.next_tick_ns = (base + jitter).max(tick_ns as f64 + 1.0) as u64;
    }
}

fn fill_signal(signal: &mut Signal, buf: &mut [f32], position: u64, ch: usize, rate: f64) {
    match signal {
        Signal::Silence => buf.fill(0.0),
        Signal::Dc(v) => buf.fill(*v),
        Signal::Sine {
            frequency,
            amplitude,
        } => {
            for (i, frame) in buf.chunks_mut(ch).enumerate() {
                let t = (position + i as u64) as f64 / rate;
                let v = (2.0 * core::f64::consts::PI * *frequency * t).sin() as f32 * *amplitude;
                frame.fill(v);
            }
        }
        Signal::Custom(f) => f(buf, position, ch),
    }
}
