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
use conduit_backend_wasapi::{ClockSource, InitPath, WasapiBackend, WasapiHandle};
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
    /// Dernière position vue : la suivante ne doit pas être plus petite (la
    /// position vient de l'horloge matérielle, elle n'avance pas de `frames`
    /// exactement à chaque rappel).
    last_position: AtomicU64,
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
            if clock.position < probe.last_position.load(Ordering::Relaxed) {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            probe.last_position.store(clock.position, Ordering::Relaxed);
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

/// Affiche périodes, tampon, latence et horloge (visible avec `--nocapture`).
fn report(handle: &WasapiHandle, what: &str) {
    let latency = handle.latency();
    eprintln!(
        "{what} : format {:?}, tampon {} trames, période {} trames, latence WASAPI {:?}, \
         chemin {:?}, horloge {}, temps réel {:?}",
        handle.format(),
        latency.buffer_frames,
        latency.period_frames,
        latency.stream_latency,
        latency.path,
        handle.clock_source(),
        handle.rt_outcome().map(|o| o.to_string()),
    );
}

/// Affiche ce que l'horloge a coûté et la latence estimée, après un `start()`.
fn report_clock(handle: &WasapiHandle) {
    let stats = handle.clock_stats();
    eprintln!(
        "  horloge : {} appels GetPosition, {} échecs, {} ns en moyenne, {} ns au pire ; \
         latence estimée {} trames",
        stats.calls,
        stats.faults,
        stats.mean_ns,
        stats.max_ns,
        handle.write_ahead_frames()
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
    probe.last_position.store(0, Ordering::Relaxed);
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
        current_period_frames,
        periods,
    } = latency.path
    {
        assert!(period_frames >= periods.min && period_frames <= periods.max);
        assert_eq!(period_frames % periods.fundamental, 0);
        assert!(period_frames as usize >= 480 || period_frames == periods.max);
        // La période que le moteur dit servir est elle aussi dans les bornes qu'il
        // annonce : c'est la même valeur que `engine_periods()` publie.
        assert!(current_period_frames >= periods.min && current_period_frames <= periods.max);
        assert_eq!(
            handle.engine_periods(),
            Some((periods, current_period_frames))
        );
    } else {
        assert_eq!(handle.engine_periods(), None);
    }

    // 1 s à 48 kHz avec des tampons ≤ 960 trames : au moins 50 réveils.
    exercise(&mut handle, &probe, 50);
    assert!(
        probe.frames_max.load(Ordering::Relaxed) >= 1,
        "aucun rappel"
    );
    let rt = handle.rt_outcome().expect("promotion tentée");
    eprintln!("fil du flux : {rt}");
    report_clock(&handle);
    drop(handle);
}

/// M1b-33 : sur le rendu par défaut, 2 s, la position avance de ≈ 48 000 trames/s
/// et l'horodatage de ≈ 1 s/s (±2 %), la position ne recule jamais,
/// Δposition / Δhorodatage ≈ 48 kHz, et `clock_now()` (lecture immédiate) est
/// cohérent avec `clock()` (dernier rappel) à une période près.
#[test]
fn clock_follows_the_hardware_at_48k() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let probe = Probe::new();
    let mut handle = backend
        .open_handle(&id, format(SampleRate::HZ_48000, 2, 480), probe.callback(2))
        .expect("ouverture du rendu par défaut");
    report(&handle, "horloge du rendu par défaut");
    let source = handle.clock_source();
    assert!(
        matches!(source, ClockSource::AudioClock { .. }),
        "IAudioClock attendue sur une carte réelle, obtenu {source}"
    );
    // Avant `start()` : rien n'a bougé, et l'horloge immédiate dit pareil.
    assert_eq!(handle.clock().position, 0);
    assert_eq!(handle.clock_now().expect("clock_now").position, 0);

    handle.start().expect("démarrage");
    // Le matériel démarre sa lecture avec un peu de retard : on laisse passer
    // le préremplissage avant de mesurer.
    std::thread::sleep(Duration::from_millis(300));
    let first = handle.clock_now().expect("clock_now");
    let wall = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    let last = handle.clock_now().expect("clock_now");
    let elapsed = wall.elapsed();
    let published = handle.clock();

    let d_position = last.position.saturating_sub(first.position) as f64;
    let d_timestamp = last.timestamp_ns.saturating_sub(first.timestamp_ns) as f64;
    let elapsed_ns = elapsed.as_nanos() as f64;
    let rate = d_position / (d_timestamp / 1e9);
    let period = handle.format().block_frames as u64;
    eprintln!(
        "  {elapsed:?} : Δposition {d_position} trames ({:.1} trames/s selon l'horloge murale), \
         Δhorodatage {:.3} s, Δposition/Δhorodatage = {rate:.1} Hz",
        d_position / elapsed.as_secs_f64(),
        d_timestamp / 1e9
    );
    report_clock(&handle);

    assert!(
        (d_position / (48_000.0 * elapsed.as_secs_f64()) - 1.0).abs() < 0.02,
        "la position n'avance pas à 48 000 trames/s : {d_position} en {elapsed:?}"
    );
    assert!(
        (d_timestamp / elapsed_ns - 1.0).abs() < 0.02,
        "l'horodatage n'avance pas à 1 s/s : {d_timestamp} ns en {elapsed:?}"
    );
    assert!(
        (rate / 48_000.0 - 1.0).abs() < 0.02,
        "Δposition / Δhorodatage = {rate} Hz, attendu ≈ 48 000"
    );
    assert!(last.position > first.position, "position figée");
    assert!(last.timestamp_ns > first.timestamp_ns, "horodatage figé");

    // `clock()` date du dernier rappel, `clock_now()` de maintenant : moins d'une
    // période d'écart, dans un sens ou dans l'autre.
    let gap_frames = last.position.abs_diff(published.position);
    let gap_ns = last.timestamp_ns.abs_diff(published.timestamp_ns);
    eprintln!("  clock_now − clock : {gap_frames} trames, {gap_ns} ns");
    assert!(
        gap_frames <= period,
        "clock_now et clock diffèrent de {gap_frames} trames (période {period})"
    );
    assert!(
        gap_ns <= period * 1_000_000_000 / 48_000,
        "clock_now et clock diffèrent de {gap_ns} ns"
    );

    // Latence estimée : ce qu'on a écrit d'avance tient dans le tampon partagé.
    let latency = handle.latency();
    assert!(latency.write_ahead_frames > 0, "aucune avance d'écriture");
    assert!(
        latency.write_ahead_frames <= (latency.buffer_frames + latency.period_frames) as u64,
        "avance d'écriture {} > tampon {} + période {}",
        latency.write_ahead_frames,
        latency.buffer_frames,
        latency.period_frames
    );

    let stats = handle.clock_stats();
    assert_eq!(
        stats.faults, 0,
        "GetPosition a échoué {} fois",
        stats.faults
    );
    assert!(
        stats.calls >= 150,
        "seulement {} appels en 2,3 s",
        stats.calls
    );
    handle.stop().expect("arrêt");
    assert_eq!(probe.faults(), 0, "position ou horodatage non monotones");
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
