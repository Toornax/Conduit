//! M1b-32 : mode exclusif WASAPI sur les cartes son de la machine.
//!
//! [`every_render_device_reports_what_exclusive_gives`] **documente le matériel** :
//! pour chaque endpoint de rendu énuméré, il ouvre le même flux (48 kHz stéréo) en
//! [`ExclusivePolicy::Never`] puis en [`ExclusivePolicy::Preferred`] et imprime ce
//! que chacun a donné — mode obtenu, format matériel, période, taille de tampon,
//! latence. Il **passe quel que soit le résultat** : une carte qui refuse
//! l'exclusif est un fait à consigner, pas un échec de Conduit. Quand l'exclusif
//! est obtenu, il vérifie en plus que le rappel tourne, que la position avance,
//! que l'arrêt est propre, et **compare la latence au mode partagé** sur la même
//! carte (le critère « latence mesurée inférieure » de la ROADMAP).
//!
//! Les rappels écrivent du **silence** : rien ne s'entend, y compris en exclusif où
//! le flux prend le périphérique pour lui seul pendant quelques centaines de
//! millisecondes.
//!
//! Les tests se sérialisent : deux flux exclusifs sur la même carte s'excluent par
//! construction, et c'est justement ce que
//! [`required_on_a_device_already_taken_says_why`] provoque exprès.

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use conduit_backend::{
    Backend, BackendError, DeviceDirection, DeviceHandle, DeviceInfo, StreamFormat,
};
use conduit_backend_wasapi::{ExclusivePolicy, InitPath, ShareMode, WasapiBackend};
use conduit_core::types::SampleRate;

/// Un flux exclusif à la fois : les tests de ce binaire se marchent dessus sinon.
static SERIAL: Mutex<()> = Mutex::new(());

/// Durée d'un flux : assez pour une centaine de réveils, assez court pour ne pas
/// monopoliser une carte.
const RUN: Duration = Duration::from_millis(400);

fn format() -> StreamFormat {
    StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 480,
    }
}

/// Ce que le rappel observe.
#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    last_position: AtomicU64,
    /// Position qui recule, tampon de la mauvaise taille, horodatage qui recule.
    faults: AtomicUsize,
    frames_min: AtomicUsize,
    frames_max: AtomicUsize,
    last_timestamp: AtomicU64,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            frames_min: AtomicUsize::new(usize::MAX),
            ..Self::default()
        })
    }

    fn callback(self: &Arc<Self>, channels: usize) -> conduit_backend::AudioCallback {
        let probe = Arc::clone(self);
        Box::new(move |io, clock| {
            if clock.frames == 0 || clock.position < probe.last_position.load(Ordering::Relaxed) {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            if clock.timestamp_ns < probe.last_timestamp.load(Ordering::Relaxed) {
                probe.faults.fetch_add(1, Ordering::Relaxed);
            }
            probe.last_position.store(clock.position, Ordering::Relaxed);
            probe
                .last_timestamp
                .store(clock.timestamp_ns, Ordering::Relaxed);
            probe.frames_min.fetch_min(clock.frames, Ordering::Relaxed);
            probe.frames_max.fetch_max(clock.frames, Ordering::Relaxed);
            // Le rappel reçoit toujours du `f32`, quel que soit le format matériel.
            if let Some(out) = io.output.as_deref() {
                if out.len() != clock.frames * channels {
                    probe.faults.fetch_add(1, Ordering::Relaxed);
                }
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

/// Ce qu'une ouverture a donné, imprimable et comparable.
struct Measured {
    mode: ShareMode,
    path: InitPath,
    buffer_frames: usize,
    period_frames: usize,
    period: Duration,
    stream_latency: Duration,
    write_ahead_frames: u64,
    calls: usize,
    clock: String,
    refusal: Option<String>,
}

impl Measured {
    /// Latence de bout en bout retenue pour la comparaison : la taille du tampon
    /// est ce que le flux a devant lui avant que le matériel ne lise, et c'est la
    /// grandeur que le mode exclusif fait baisser.
    fn buffer_micros(&self) -> u128 {
        (self.buffer_frames as u128) * 1_000_000 / 48_000
    }
}

/// Ouvre, mesure, arrête. Rend `Err` si l'ouverture elle-même a échoué.
fn measure(
    policy: ExclusivePolicy,
    id: &conduit_backend::DeviceId,
) -> Result<(Measured, Arc<Probe>), BackendError> {
    let mut backend = WasapiBackend::new().expect("WasapiBackend::new");
    backend.set_exclusive_policy(policy);
    assert_eq!(backend.exclusive_policy(), policy);
    let probe = Probe::new();
    let mut handle = backend.open_handle(id, format(), probe.callback(2))?;
    handle.start().expect("démarrage");
    std::thread::sleep(RUN);
    let latency = handle.latency();
    let measured = Measured {
        mode: handle.share_mode(),
        path: latency.path,
        buffer_frames: latency.buffer_frames,
        period_frames: latency.period_frames,
        period: latency.period,
        stream_latency: latency.stream_latency,
        write_ahead_frames: latency.write_ahead_frames,
        calls: probe.calls(),
        clock: handle.clock_source().to_string(),
        refusal: handle.exclusive_refusal().map(str::to_owned),
    };
    let stopping = Instant::now();
    handle.stop().expect("arrêt");
    assert!(
        stopping.elapsed() < Duration::from_secs(1),
        "stop a pris {:?}",
        stopping.elapsed()
    );
    assert!(!handle.is_running());
    Ok((measured, probe))
}

/// Une ligne du tableau que la ROADMAP demande.
fn report(name: &str, what: &str, m: &Measured) {
    eprintln!(
        "  {name} | {what:<9} | mode {} | format matériel {} | tampon {} trames ({} µs) | \
         période {} trames ({:?}) | GetStreamLatency {:?} | avance {} trames | {} rappels | {}",
        m.mode,
        m.path.sample_type(),
        m.buffer_frames,
        m.buffer_micros(),
        m.period_frames,
        m.period,
        m.stream_latency,
        m.write_ahead_frames,
        m.calls,
        m.clock,
    );
    if let InitPath::Exclusive {
        period_hns,
        realigned,
        ..
    } = m.path
    {
        eprintln!(
            "           période {period_hns} × 100 ns, tampon réaligné : {}",
            if realigned { "oui" } else { "non" }
        );
    }
    if let Some(reason) = &m.refusal {
        eprintln!("           exclusif refusé : {reason}");
    }
}

fn render_devices(backend: &WasapiBackend) -> Vec<DeviceInfo> {
    backend
        .devices()
        .expect("énumération")
        .into_iter()
        .filter(|d| d.direction == DeviceDirection::Render)
        .collect()
}

/// Le test qui documente le matériel : chaque carte de rendu, en partagé puis en
/// exclusif souhaité. Passe quel que soit ce que le matériel accepte.
#[test]
#[ignore = "prend les cartes en mode exclusif, ce qui évince les autres applications : à lancer à la main (-- --ignored)"]
fn every_render_device_reports_what_exclusive_gives() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let backend = WasapiBackend::new().expect("WasapiBackend::new");
    let devices = render_devices(&backend);
    drop(backend);
    if devices.is_empty() {
        eprintln!("test sauté : aucun périphérique de rendu actif");
        return;
    }
    eprintln!(
        "Mode exclusif à 48 kHz stéréo, {} carte(s) :",
        devices.len()
    );
    let mut obtained = 0usize;
    for device in &devices {
        let (shared, shared_probe) = match measure(ExclusivePolicy::Never, &device.id) {
            Ok(measured) => measured,
            Err(e) => {
                eprintln!("  {} | partagé : ouverture refusée ({e})", device.name);
                continue;
            }
        };
        report(&device.name, "partagé", &shared);
        // `Never` ne doit jamais donner autre chose que du partagé.
        assert_eq!(shared.mode, ShareMode::Shared, "{}", device.name);
        assert!(shared.refusal.is_none(), "{}", device.name);
        assert_eq!(shared_probe.faults(), 0, "{}", device.name);

        match measure(ExclusivePolicy::Preferred, &device.id) {
            Ok((exclusive, probe)) => {
                report(&device.name, "souhaité", &exclusive);
                if exclusive.mode == ShareMode::Shared {
                    // Repli documenté : la raison doit être là, et lisible.
                    let reason = exclusive
                        .refusal
                        .as_deref()
                        .expect("un repli en partagé doit dire pourquoi");
                    assert!(reason.len() > 20, "raison trop maigre : {reason}");
                    continue;
                }
                obtained += 1;
                assert!(exclusive.refusal.is_none());
                assert!(
                    matches!(exclusive.path, InitPath::Exclusive { .. }),
                    "{:?}",
                    exclusive.path
                );
                // Le rappel a tourné, la position a avancé, rien d'incohérent.
                assert!(
                    exclusive.calls >= 10,
                    "{} : seulement {} rappels en {RUN:?}",
                    device.name,
                    exclusive.calls
                );
                assert_eq!(probe.faults(), 0, "{}", device.name);
                assert!(
                    probe.last_position.load(Ordering::Relaxed) > 0,
                    "{} : position figée en exclusif",
                    device.name
                );
                assert!(probe.frames_max.load(Ordering::Relaxed) > 0);
                // Le critère de la ROADMAP : la latence doit baisser.
                eprintln!(
                    "           latence : {} µs en exclusif contre {} µs en partagé \
                     ({} trames contre {})",
                    exclusive.buffer_micros(),
                    shared.buffer_micros(),
                    exclusive.buffer_frames,
                    shared.buffer_frames,
                );
                assert!(
                    exclusive.buffer_frames <= shared.buffer_frames,
                    "{} : le tampon exclusif ({} trames) n'est pas plus petit que le \
                     partagé ({} trames)",
                    device.name,
                    exclusive.buffer_frames,
                    shared.buffer_frames
                );
            }
            Err(e) => eprintln!("  {} | souhaité : {e}", device.name),
        }
    }
    eprintln!(
        "Exclusif obtenu sur {obtained} carte(s) sur {}.",
        devices.len()
    );
}

/// `Never` est le défaut, et il ne donne jamais d'exclusif.
#[test]
fn never_is_the_default_and_always_shares() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut backend = WasapiBackend::new().expect("WasapiBackend::new");
    assert_eq!(
        backend.exclusive_policy(),
        ExclusivePolicy::Never,
        "le défaut doit rester le mode partagé"
    );
    let Some(id) = backend.default_device(DeviceDirection::Render) else {
        eprintln!("test sauté : aucun périphérique de rendu par défaut");
        return;
    };
    // Un aller-retour de politique ne change pas le défaut du prochain backend.
    backend.set_exclusive_policy(ExclusivePolicy::Preferred);
    assert_eq!(backend.exclusive_policy(), ExclusivePolicy::Preferred);
    backend.set_exclusive_policy(ExclusivePolicy::Never);
    drop(backend);

    let (measured, probe) = measure(ExclusivePolicy::Never, &id).expect("ouverture en partagé");
    report("rendu par défaut", "Never", &measured);
    assert_eq!(measured.mode, ShareMode::Shared);
    assert!(matches!(
        measured.path,
        InitPath::LowLatency { .. } | InitPath::Converted
    ));
    assert_eq!(measured.path.sample_type().to_string(), "float32");
    assert!(measured.calls >= 10, "{} rappels", measured.calls);
    assert_eq!(probe.faults(), 0);
    assert_eq!(
        WasapiBackend::new()
            .expect("WasapiBackend::new")
            .exclusive_policy(),
        ExclusivePolicy::Never
    );
}

/// `Required` sur un périphérique déjà pris : l'erreur doit être une
/// [`BackendError::UnsupportedFormat`] qui dit pourquoi et quoi faire.
///
/// On provoque le cas en ouvrant **deux fois** : le premier flux (exclusif s'il est
/// possible, partagé sinon) tient le périphérique, le second l'exige en exclusif.
/// Sur un matériel qui refuse déjà l'exclusif tout court, la même erreur tombe pour
/// une autre raison, et c'est aussi ce qu'on veut vérifier : le message.
#[test]
#[ignore = "prend les cartes en mode exclusif, ce qui évince les autres applications : à lancer à la main (-- --ignored)"]
fn required_on_a_device_already_taken_says_why() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut first = WasapiBackend::new().expect("WasapiBackend::new");
    let Some(id) = first.default_device(DeviceDirection::Render) else {
        eprintln!("test sauté : aucun périphérique de rendu par défaut");
        return;
    };
    first.set_exclusive_policy(ExclusivePolicy::Preferred);
    let holder_probe = Probe::new();
    let mut holder = first
        .open_handle(&id, format(), holder_probe.callback(2))
        .expect("premier flux");
    let held = holder.share_mode();
    holder.start().expect("démarrage du premier flux");
    std::thread::sleep(Duration::from_millis(150));
    eprintln!("premier flux : mode {held}");

    let mut second = WasapiBackend::new().expect("WasapiBackend::new");
    second.set_exclusive_policy(ExclusivePolicy::Required);
    let result = second.open_handle(&id, format(), Box::new(|io, _| io.silence_output()));
    match result {
        Err(BackendError::UnsupportedFormat { device, reason }) => {
            eprintln!("Required refusé, message : {reason}");
            assert_eq!(device, id);
            assert!(
                reason.contains("exclusif"),
                "le message doit nommer le mode exclusif : {reason}"
            );
            assert!(
                reason.contains("Preferred"),
                "le message doit dire quoi faire : {reason}"
            );
            assert!(reason.len() > 60, "message trop maigre : {reason}");
        }
        Err(other) => panic!("erreur inattendue : {other}"),
        Ok(handle) => {
            // Le pilote accepte deux flux exclusifs (ou l'un des deux est partagé) :
            // pas d'erreur à vérifier, mais le mode obtenu doit être l'exclusif —
            // `Required` ne rend jamais un flux partagé.
            eprintln!(
                "Required accepté malgré le premier flux ({held}) : mode {}",
                handle.share_mode()
            );
            assert_eq!(handle.share_mode(), ShareMode::Exclusive);
        }
    }
    holder.stop().expect("arrêt du premier flux");
    assert_eq!(holder_probe.faults(), 0);
}

/// `Required` sur un endpoint inexistant reste une `NotFound` : la politique ne
/// masque pas les erreurs plus fondamentales.
#[test]
fn required_does_not_mask_a_missing_device() {
    let mut backend = WasapiBackend::new().expect("WasapiBackend::new");
    backend.set_exclusive_policy(ExclusivePolicy::Required);
    let id =
        conduit_backend::DeviceId::new("{0.0.0.00000000}.{00000000-0000-0000-0000-000000000000}");
    let result = backend.open(&id, format(), Box::new(|io, _| io.silence_output()));
    assert!(
        matches!(result, Err(BackendError::NotFound(ref d)) if *d == id),
        "{result:?}"
    );
}
