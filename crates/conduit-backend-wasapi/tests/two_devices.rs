//! M1b-33 : deux cartes de rendu réelles ouvertes en même temps.
//!
//! - [`two_render_devices_share_the_qpc_base`] (60 s, test normal) ouvre les deux
//!   premiers endpoints de rendu actifs (de préférence « G27QC A » et « Realtek
//!   Digital Output », ceux du poste de développement), vérifie que leurs
//!   horodatages partagent la base QPC, que leurs positions sont monotones,
//!   qu'aucun ne se déconnecte, et **mesure** la dérive relative des deux horloges
//!   (pente de `positionA − positionB` en fonction du temps, en ppm) ;
//! - [`engine_drives_one_card_and_absorbs_the_other_for_an_hour`] (`#[ignore]`,
//!   1 h) fait la même chose à travers le moteur : une carte pilote le graphe,
//!   l'autre est asynchrone (port + DLL), et le compte de xruns doit rester nul.
//!
//! Les rappels écrivent du silence. Sur un poste sans deux cartes de rendu, les
//! tests se sautent avec un message.

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use conduit_backend::{
    Backend, BackendError, ClockInfo, DeviceDirection, DeviceHandle, DeviceInfo, StreamFormat,
};
use conduit_backend_wasapi::{ClockSource, WasapiBackend, WasapiHandle};
use conduit_core::types::SampleRate;
use conduit_engine::{
    DriverStatus, Engine, EngineConfig, EngineEvent, EngineStatus, NodeState, Notification,
};

/// Durée du test normal.
const RUN: Duration = Duration::from_secs(60);
/// Durée du test d'endurance (`CONDUIT_ENDURANCE_SECS` pour la raccourcir).
const ENDURANCE: Duration = Duration::from_secs(3_600);
/// Les cartes du poste de développement, prises en priorité si elles sont là.
const PREFERRED: [&str; 2] = ["G27QC A", "Realtek Digital Output"];

fn format() -> StreamFormat {
    StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 480,
    }
}

/// Les deux endpoints de rendu à exercer, ou `None` s'il n'y en a pas deux.
fn pick_two(devices: &[DeviceInfo]) -> Option<(DeviceInfo, DeviceInfo)> {
    let renders: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| d.direction == DeviceDirection::Render)
        .collect();
    // Les noms complets portent le fabricant entre parenthèses : sous-chaîne.
    let mut chosen: Vec<&DeviceInfo> = PREFERRED
        .iter()
        .filter_map(|name| renders.iter().copied().find(|d| d.name.contains(name)))
        .collect();
    for d in &renders {
        if chosen.len() >= 2 {
            break;
        }
        if !chosen.iter().any(|c| c.id == d.id) {
            chosen.push(d);
        }
    }
    match chosen.as_slice() {
        [a, b, ..] => Some(((*a).clone(), (*b).clone())),
        _ => None,
    }
}

/// Ce que le rappel d'une carte observe.
#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    last_position: AtomicU64,
    last_timestamp: AtomicU64,
    /// Position ou horodatage qui recule.
    faults: AtomicUsize,
}

impl Probe {
    fn callback(self: &Arc<Self>) -> conduit_backend::AudioCallback {
        let probe = Arc::clone(self);
        Box::new(move |io, clock| {
            if clock.position < probe.last_position.load(Ordering::Relaxed)
                || clock.timestamp_ns < probe.last_timestamp.load(Ordering::Relaxed)
            {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            probe.last_position.store(clock.position, Ordering::Relaxed);
            probe
                .last_timestamp
                .store(clock.timestamp_ns, Ordering::Relaxed);
            probe.calls.fetch_add(1, Ordering::Relaxed);
            io.silence_output();
        })
    }
}

/// Une carte ouverte pour le test.
struct Card {
    info: DeviceInfo,
    handle: WasapiHandle,
    probe: Arc<Probe>,
}

impl Card {
    fn open(backend: &mut WasapiBackend, info: DeviceInfo) -> Self {
        let probe = Arc::new(Probe::default());
        let handle = backend
            .open_handle(&info.id, format(), probe.callback())
            .unwrap_or_else(|e| panic!("ouverture de {} : {e}", info.name));
        let latency = handle.latency();
        eprintln!(
            "{} : {:?}, tampon {} trames, période {} trames, chemin {:?}, horloge {}",
            info.name,
            handle.format(),
            latency.buffer_frames,
            latency.period_frames,
            latency.path,
            handle.clock_source()
        );
        Self {
            info,
            handle,
            probe,
        }
    }

    fn now(&self) -> ClockInfo {
        self.handle
            .clock_now()
            .unwrap_or_else(|e| panic!("clock_now sur {} : {e}", self.info.name))
    }
}

/// Pente (trames par seconde) de `y` en fonction de `t` par moindres carrés.
fn slope(samples: &[(f64, f64)]) -> f64 {
    let n = samples.len() as f64;
    let (mut st, mut sy, mut stt, mut sty) = (0.0, 0.0, 0.0, 0.0);
    for &(t, y) in samples {
        st += t;
        sy += y;
        stt += t * t;
        sty += t * y;
    }
    let denominator = n * stt - st * st;
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        (n * sty - st * sy) / denominator
    }
}

#[test]
fn two_render_devices_share_the_qpc_base() {
    let mut backend = WasapiBackend::new().expect("WasapiBackend::new");
    let devices = backend.devices().expect("énumération");
    let Some((first, second)) = pick_two(&devices) else {
        eprintln!("test sauté : moins de deux périphériques de rendu actifs");
        return;
    };
    let mut a = Card::open(&mut backend, first);
    let mut b = Card::open(&mut backend, second);
    for card in [&a, &b] {
        assert!(
            matches!(card.handle.clock_source(), ClockSource::AudioClock { .. }),
            "{} : IAudioClock attendue, obtenu {}",
            card.info.name,
            card.handle.clock_source()
        );
    }
    a.handle.start().expect("démarrage A");
    b.handle.start().expect("démarrage B");
    std::thread::sleep(Duration::from_secs(1));

    // Même base de temps : deux lectures immédiates, l'une après l'autre, et les
    // horodatages des derniers rappels, sont à moins de 100 ms.
    let a0 = a.now();
    let b0 = b.now();
    let base_gap = a0.timestamp_ns.abs_diff(b0.timestamp_ns);
    let published_gap = a
        .handle
        .clock()
        .timestamp_ns
        .abs_diff(b.handle.clock().timestamp_ns);
    eprintln!(
        "base QPC : écart immédiat {} µs, écart des derniers rappels {} µs",
        base_gap / 1_000,
        published_gap / 1_000
    );
    assert!(base_gap < 100_000_000, "bases différentes : {base_gap} ns");
    assert!(
        published_gap < 100_000_000,
        "horodatages publiés éloignés : {published_gap} ns"
    );

    // Dérive : `positionA − positionB` contre le temps, un point par seconde.
    let started = Instant::now();
    let mut samples: Vec<(f64, f64)> = Vec::new();
    let rate = f64::from(format().sample_rate.hz());
    while started.elapsed() < RUN {
        std::thread::sleep(Duration::from_secs(1));
        let (na, nb) = (a.now(), b.now());
        let t = (na.timestamp_ns - a0.timestamp_ns) as f64 / 1e9;
        let da = (na.position - a0.position) as f64;
        let db = (nb.position - b0.position) as f64;
        samples.push((t, da - db));
        let elapsed = started.elapsed().as_secs();
        if elapsed % 10 == 0 {
            eprintln!(
                "  t = {elapsed:>3} s : A {:>9} trames, B {:>9} trames, A − B = {:>6} trames, \
                 avance A {} / B {} trames",
                da,
                db,
                da - db,
                a.handle.write_ahead_frames(),
                b.handle.write_ahead_frames()
            );
        }
    }
    let (na, nb) = (a.now(), b.now());
    let seconds_a = (na.timestamp_ns - a0.timestamp_ns) as f64 / 1e9;
    let seconds_b = (nb.timestamp_ns - b0.timestamp_ns) as f64 / 1e9;
    let rate_a = (na.position - a0.position) as f64 / seconds_a;
    let rate_b = (nb.position - b0.position) as f64 / seconds_b;
    let drift = slope(&samples);
    eprintln!(
        "{} : {rate_a:.3} Hz ({:+.1} ppm) ; {} : {rate_b:.3} Hz ({:+.1} ppm) ; \
         dérive relative A − B : {drift:+.3} trames/s = {:+.1} ppm sur {:.0} s",
        a.info.name,
        (rate_a / rate - 1.0) * 1e6,
        b.info.name,
        (rate_b / rate - 1.0) * 1e6,
        drift / rate * 1e6,
        seconds_a
    );
    for card in [&a, &b] {
        let stats = card.handle.clock_stats();
        eprintln!(
            "{} : {} rappels, {} appels GetPosition ({} échecs, {} ns en moyenne, {} ns au pire)",
            card.info.name,
            card.probe.calls.load(Ordering::Relaxed),
            stats.calls,
            stats.faults,
            stats.mean_ns,
            stats.max_ns
        );
    }

    // Chaque carte tourne à ±2 % de sa fréquence nominale.
    assert!(
        (rate_a / rate - 1.0).abs() < 0.02,
        "{} : {rate_a} Hz",
        a.info.name
    );
    assert!(
        (rate_b / rate - 1.0).abs() < 0.02,
        "{} : {rate_b} Hz",
        b.info.name
    );
    // Aucune déconnexion, positions et horodatages monotones.
    for card in [&mut a, &mut b] {
        assert!(card.handle.is_running(), "{} s'est arrêtée", card.info.name);
        match card.handle.stop() {
            Ok(()) => {}
            Err(BackendError::Disconnected(id)) => panic!("{} déconnectée ({id})", card.info.name),
            Err(e) => panic!("arrêt de {} : {e}", card.info.name),
        }
        assert_eq!(
            card.probe.faults.load(Ordering::Relaxed),
            0,
            "{} : position ou horodatage non monotones",
            card.info.name
        );
        assert_eq!(card.handle.clock_stats().faults, 0);
    }
}

/// Résumé d'un état du moteur pour le journal du test d'endurance.
fn describe(status: &EngineStatus) -> String {
    let devices: Vec<String> = status
        .devices
        .iter()
        .filter(|d| d.state != NodeState::Suspended)
        .map(|d| {
            format!(
                "{:?} ur {} or {} fill {} ratio {:.6} {}",
                d.state,
                d.underruns,
                d.overruns,
                d.fill,
                d.ratio,
                if d.locked { "verrouillée" } else { "libre" }
            )
        })
        .collect();
    format!(
        "xruns {} (cycles hors budget {}), cycles {}, max {} µs ; {}",
        status.xruns,
        status.timing.overruns,
        status.cycles,
        status.timing.max_ns / 1_000,
        devices.join(" | ")
    )
}

/// Critère « Fait quand » de M1b-33 : le moteur pilote une carte, absorbe la
/// dérive de l'autre par son port asynchrone, et ne compte aucun xrun en 1 h.
///
/// ```sh
/// cargo test -p conduit-backend-wasapi --test two_devices -- --ignored --nocapture
/// ```
///
/// `CONDUIT_ENDURANCE_SECS` raccourcit la durée (mise au point).
#[test]
#[ignore = "une heure avec le moteur : à lancer à la main"]
fn engine_drives_one_card_and_absorbs_the_other_for_an_hour() {
    let duration = std::env::var("CONDUIT_ENDURANCE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(ENDURANCE);
    let backend = WasapiBackend::new().expect("WasapiBackend::new");
    if pick_two(&backend.devices().expect("énumération")).is_none() {
        eprintln!("test sauté : moins de deux périphériques de rendu actifs");
        return;
    }
    let mut engine = Engine::new(Box::new(backend), EngineConfig::default()).expect("Engine::new");
    let status = engine.status();
    let DriverStatus::Device { id: driver } = &status.driver else {
        panic!("le pilote devrait être une carte : {:?}", status.driver);
    };
    let async_renders = status
        .devices
        .iter()
        .filter(|d| d.state == NodeState::Active)
        .count();
    assert!(
        async_renders >= 1,
        "au moins une carte asynchrone attendue : {status:?}"
    );
    eprintln!(
        "pilote {driver}, {async_renders} périphérique(s) asynchrone(s), {duration:?} ; {}",
        describe(&status)
    );

    let started = Instant::now();
    let mut last_report = Instant::now();
    let (mut busy, mut overruns, mut device_xruns) = (0u64, 0u64, 0u64);
    while started.elapsed() < duration {
        std::thread::sleep(Duration::from_millis(50));
        for n in engine.tick() {
            match n {
                Notification::Rt(EngineEvent::ExecutorBusy) => busy += 1,
                Notification::Rt(EngineEvent::CycleOverrun { .. }) => overruns += 1,
                Notification::Rt(EngineEvent::DeviceXrun { device, underrun }) => {
                    device_xruns += 1;
                    eprintln!(
                        "  t = {:?} : xrun sur {device} ({})",
                        started.elapsed(),
                        if underrun {
                            "sous-alimentation"
                        } else {
                            "débordement"
                        }
                    );
                }
                _ => {}
            }
        }
        if last_report.elapsed() >= Duration::from_secs(60) {
            last_report = Instant::now();
            eprintln!(
                "  t = {:>5} s : {} ; exécuteur occupé {busy}",
                started.elapsed().as_secs(),
                describe(&engine.status())
            );
        }
    }
    let status = engine.status();
    eprintln!(
        "fin après {:?} : {} ; exécuteur occupé {busy}, cycles hors budget {overruns}, \
         xruns périphériques {device_xruns}",
        started.elapsed(),
        describe(&status)
    );
    engine.shutdown();
    assert!(
        matches!(status.driver, DriverStatus::Device { .. }),
        "le pilote a changé : {:?}",
        status.driver
    );
    assert_eq!(status.xruns, 0, "{}", describe(&status));
    assert_eq!(device_xruns, 0);
}
