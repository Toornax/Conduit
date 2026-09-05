//! Tests d'intégration des flux WASAPI (M1b-31) contre les cartes son de la machine.
//!
//! Ils supposent un endpoint de rendu par défaut (et, pour la capture, un micro
//! par défaut). Les rappels écrivent du **silence** : rien ne s'entend. Le seul
//! test sonore, [`sine_audible`], est `#[ignore]` et se lance à la main.
//!
//! Le critère « Fait quand » de la ROADMAP (boucle à travers Conduit 1 via
//! l'engine) demande le pilote : il n'est pas exercé ici.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use conduit_backend::{
    Backend, BackendError, DeviceDirection, DeviceHandle, DeviceId, StreamFormat,
};
use conduit_backend_wasapi::{InitPath, WasapiBackend, WasapiHandle};
use conduit_core::types::SampleRate;

/// Un poste sans le périphérique demandé ne peut pas exercer le test : on le dit
/// plutôt que d'échouer.
macro_rules! default_or_skip {
    ($backend:expr, $direction:expr) => {
        match $backend.default_device($direction) {
            Some(id) => id,
            None => {
                eprintln!(
                    "test sauté : aucun périphérique de {} par défaut",
                    $direction
                );
                return;
            }
        }
    };
}

fn backend() -> WasapiBackend {
    WasapiBackend::new().expect("WasapiBackend::new")
}

fn format(rate: SampleRate, channels: usize, block_frames: usize) -> StreamFormat {
    StreamFormat {
        sample_rate: rate,
        channels,
        block_frames,
    }
}

/// Ce que le rappel observe, lisible depuis le fil de test.
#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    /// Position attendue au prochain rappel (position + trames du précédent).
    expected_position: AtomicU64,
    last_timestamp: AtomicU64,
    /// Tampon de la mauvaise taille, position ou horodatage incohérents.
    faults: AtomicUsize,
    /// Au moins un rappel a reçu de l'entrée (capture).
    saw_input: AtomicBool,
    frames_min: AtomicUsize,
    frames_max: AtomicUsize,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            frames_min: AtomicUsize::new(usize::MAX),
            ..Self::default()
        })
    }

    /// Rappel de test : vérifie les invariants, écrit du silence.
    fn callback(self: &Arc<Self>, channels: usize) -> conduit_backend::AudioCallback {
        let probe = Arc::clone(self);
        Box::new(move |io, clock| {
            if clock.frames == 0 {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            if clock.position != probe.expected_position.load(Ordering::Relaxed) {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            probe
                .expected_position
                .store(clock.position + clock.frames as u64, Ordering::Relaxed);
            if clock.timestamp_ns < probe.last_timestamp.load(Ordering::Relaxed) {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            probe
                .last_timestamp
                .store(clock.timestamp_ns, Ordering::Relaxed);
            probe.frames_min.fetch_min(clock.frames, Ordering::Relaxed);
            probe.frames_max.fetch_max(clock.frames, Ordering::Relaxed);
            match io.output.as_deref() {
                Some(out) if out.len() == clock.frames * channels => {}
                Some(_) => {
                    probe.faults.fetch_add(1, Ordering::Relaxed);
                }
                None => {}
            }
            match io.input {
                Some(input) if input.len() == clock.frames * channels => {
                    probe.saw_input.store(true, Ordering::Relaxed);
                }
                Some(_) => {
                    probe.faults.fetch_add(1, Ordering::Relaxed);
                }
                None => {}
            }
            io.silence_output();
            probe.calls.fetch_add(1, Ordering::Relaxed);
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    fn faults(&self) -> usize {
        self.faults.load(Ordering::Relaxed)
    }
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    condition()
}

/// Affiche périodes, tampon et latence (visible avec `--nocapture`).
fn report(handle: &WasapiHandle, what: &str) {
    let latency = handle.latency();
    eprintln!(
        "{what} : format {:?}, tampon {} trames, période {} trames, latence WASAPI {:?}, \
         chemin {:?}, temps réel {:?}",
        handle.format(),
        latency.buffer_frames,
        latency.period_frames,
        latency.stream_latency,
        latency.path,
        handle.rt_outcome().map(|o| o.to_string()),
    );
}

/// Vérifie le contrat du trait autour de `start` / `stop` / `start` / `stop`.
fn exercise(handle: &mut dyn DeviceHandle, probe: &Arc<Probe>, min_calls: usize) {
    assert!(!handle.is_running());
    assert_eq!(handle.clock().position, 0);

    handle.start().expect("démarrage");
    assert!(handle.is_running());
    std::thread::sleep(Duration::from_secs(1));
    let calls = probe.calls();
    assert!(
        calls >= min_calls,
        "seulement {calls} rappels en 1 s (attendu ≥ {min_calls})"
    );
    let clock = handle.clock();
    assert!(clock.position > 0, "position figée : {clock:?}");
    assert!(clock.timestamp_ns > 0, "horodatage nul : {clock:?}");
    assert!(clock.frames > 0);

    let stopping = Instant::now();
    handle.stop().expect("arrêt");
    assert!(
        stopping.elapsed() < Duration::from_secs(1),
        "stop a pris {:?}",
        stopping.elapsed()
    );
    assert!(!handle.is_running());
    let after_stop = probe.calls();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        probe.calls(),
        after_stop,
        "des rappels ont eu lieu après stop"
    );
    assert_eq!(
        probe.faults(),
        0,
        "tampon, position ou horodatage incohérents"
    );
    eprintln!(
        "  {calls} rappels en 1 s, {} à {} trames par rappel, position finale {}",
        probe.frames_min.load(Ordering::Relaxed),
        probe.frames_max.load(Ordering::Relaxed),
        clock.position
    );

    // Redémarrage : la position repart de zéro, les rappels reprennent.
    probe.expected_position.store(0, Ordering::Relaxed);
    handle.start().expect("second démarrage");
    assert!(handle.is_running());
    assert!(
        wait_until(Duration::from_secs(2), || probe.calls() > after_stop),
        "aucun rappel après le second start"
    );
    // Deux `start` de suite : idempotent.
    handle.start().expect("start idempotent");
    handle.stop().expect("second arrêt");
    handle.stop().expect("stop idempotent");
    assert_eq!(probe.faults(), 0);
}

#[test]
fn render_default_at_48k_stereo() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let probe = Probe::new();
    let wanted = format(SampleRate::HZ_48000, 2, 480);
    let mut handle = backend
        .open_handle(&id, wanted, probe.callback(2))
        .expect("ouverture du rendu par défaut");
    report(&handle, "rendu 48 kHz stéréo, 480 demandées");
    let got = handle.format();
    assert_eq!(got.sample_rate, wanted.sample_rate);
    assert_eq!(got.channels, wanted.channels);
    assert!(got.block_frames >= 1);
    assert_eq!(handle.info().id, id);
    assert_eq!(handle.info().direction, DeviceDirection::Render);
    assert!(format!("{handle:?}").contains("WasapiHandle"));
    assert!(handle.rt_outcome().is_none(), "pas de fil avant start");

    let latency = handle.latency();
    assert!(latency.buffer_frames >= latency.period_frames);
    assert_eq!(latency.period_frames, got.block_frames);
    if let InitPath::LowLatency {
        period_frames,
        periods,
    } = latency.path
    {
        assert!(period_frames >= periods.min && period_frames <= periods.max);
        assert_eq!(period_frames % periods.fundamental, 0);
        assert!(period_frames as usize >= 480 || period_frames == periods.max);
    }

    // 1 s à 48 kHz avec des tampons ≤ 960 trames : au moins 50 réveils.
    exercise(&mut handle, &probe, 50);
    assert!(
        probe.frames_max.load(Ordering::Relaxed) >= 1,
        "aucun rappel"
    );
    let rt = handle.rt_outcome().expect("promotion tentée");
    eprintln!("fil du flux : {rt}");
    drop(handle);
}

#[test]
fn render_default_converted_to_44100_mono() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let probe = Probe::new();
    let wanted = format(SampleRate::HZ_44100, 1, 441);
    let mut handle = backend
        .open_handle(&id, wanted, probe.callback(1))
        .expect("ouverture en 44,1 kHz mono");
    report(&handle, "rendu 44,1 kHz mono");
    let got = handle.format();
    assert_eq!(got.sample_rate, SampleRate::HZ_44100);
    assert_eq!(got.channels, 1);
    // Le format demandé diffère (presque toujours) du mixage : chemin conversion.
    let mix = backend
        .devices()
        .expect("énumération")
        .into_iter()
        .find(|d| d.id == id)
        .expect("le rendu par défaut est énuméré");
    if mix.channels != 1 || mix.sample_rate != SampleRate::HZ_44100 {
        assert_eq!(handle.latency().path, InitPath::Converted);
    }
    // 1 s à 44,1 kHz par tranches de ~10 ms : au moins 50 réveils.
    exercise(&mut handle, &probe, 50);
}

#[test]
fn capture_default_delivers_input() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Capture);
    let probe = Probe::new();
    let wanted = format(SampleRate::HZ_48000, 2, 480);
    let mut handle = backend
        .open_handle(&id, wanted, probe.callback(2))
        .expect("ouverture du micro par défaut");
    report(&handle, "capture 48 kHz stéréo");
    assert_eq!(handle.info().direction, DeviceDirection::Capture);
    assert_eq!(handle.format().channels, 2);
    // Un paquet par période (~10 ms) : au moins 50 en 1 s.
    exercise(&mut handle, &probe, 50);
    assert!(
        probe.saw_input.load(Ordering::Relaxed),
        "aucune entrée reçue"
    );
    assert!(probe.frames_min.load(Ordering::Relaxed) >= 1);
}

#[test]
fn opening_an_unknown_device_is_not_found() {
    let mut backend = backend();
    let id = DeviceId::new("{0.0.0.00000000}.{00000000-0000-0000-0000-000000000000}");
    let result = backend.open(
        &id,
        format(SampleRate::HZ_48000, 2, 480),
        Box::new(|io, _| io.silence_output()),
    );
    assert!(
        matches!(result, Err(BackendError::NotFound(ref d)) if *d == id),
        "{result:?}"
    );
}

#[test]
fn zero_channels_is_unsupported() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let result = backend.open(
        &id,
        format(SampleRate::HZ_48000, 0, 480),
        Box::new(|io, _| io.silence_output()),
    );
    assert!(
        matches!(result, Err(BackendError::UnsupportedFormat { ref device, .. }) if *device == id),
        "{result:?}"
    );
}

/// La poignée (`Box<dyn DeviceHandle>`) survit au backend qui l'a créée.
#[test]
fn handle_outlives_the_backend() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let probe = Probe::new();
    let mut handle = backend
        .open(&id, format(SampleRate::HZ_48000, 2, 480), probe.callback(2))
        .expect("ouverture");
    drop(backend);
    handle.start().expect("démarrage sans backend");
    assert!(wait_until(Duration::from_secs(2), || probe.calls() > 0));
    handle.stop().expect("arrêt sans backend");
    assert_eq!(probe.faults(), 0);
}

/// Détruire une poignée qui tourne l'arrête proprement.
#[test]
fn dropping_a_running_handle_stops_it() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let probe = Probe::new();
    let mut handle = backend
        .open(&id, format(SampleRate::HZ_48000, 2, 480), probe.callback(2))
        .expect("ouverture");
    handle.start().expect("démarrage");
    assert!(wait_until(Duration::from_secs(2), || probe.calls() > 0));
    let dropping = Instant::now();
    drop(handle);
    assert!(dropping.elapsed() < Duration::from_secs(1));
    let after = probe.calls();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(probe.calls(), after, "des rappels ont eu lieu après drop");
}

/// Joue un sinus à 440 Hz, −20 dBFS, pendant 0,5 s sur le rendu par défaut.
///
/// ```sh
/// cargo test -p conduit-backend-wasapi --test stream sine_audible -- --ignored --nocapture
/// ```
#[test]
#[ignore = "émet un son : à lancer à la main"]
fn sine_audible() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let wanted = format(SampleRate::HZ_48000, 2, 480);
    let mut phase = 0.0f32;
    let step = 440.0 / 48_000.0;
    let amplitude = 10f32.powf(-20.0 / 20.0);
    let mut handle = backend
        .open_handle(
            &id,
            wanted,
            Box::new(move |io, _| {
                let Some(out) = io.output.as_deref_mut() else {
                    return;
                };
                for frame in out.chunks_mut(2) {
                    let v = (phase * core::f32::consts::TAU).sin() * amplitude;
                    phase = (phase + step).fract();
                    frame.fill(v);
                }
            }),
        )
        .expect("ouverture");
    report(&handle, "sinus 440 Hz");
    handle.start().expect("démarrage");
    std::thread::sleep(Duration::from_millis(500));
    handle.stop().expect("arrêt");
}
