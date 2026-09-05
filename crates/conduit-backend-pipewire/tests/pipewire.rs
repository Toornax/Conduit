//! Tests d'intégration du backend PipeWire contre un démon headless.
//!
//! Chaque test lance son propre démon (voir `common`). Sans le binaire `pipewire`
//! dans le `PATH` — c'est-à-dire hors du devshell Nix — les tests se sautent avec un
//! message plutôt que d'échouer.

#![cfg(target_os = "linux")]

mod common;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use conduit_backend::{
    Backend, BackendError, DeviceDirection, DeviceEvent, DeviceId, StreamFormat,
};
use conduit_backend_pipewire::PipewireBackend;
use conduit_core::types::SampleRate;

use common::PwDaemon;

/// Nœud fourni par la configuration de test.
const NULL_SINK: &str = "Null-Sink";

macro_rules! daemon_or_skip {
    ($start:expr, $why:expr) => {
        match $start {
            Some(daemon) => daemon,
            None => {
                eprintln!("test sauté : {} introuvable (hors devshell Nix ?)", $why);
                return;
            }
        }
    };
}

fn format() -> StreamFormat {
    StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 256,
    }
}

#[test]
fn enumerates_the_null_sink_as_a_render_device() {
    let daemon = daemon_or_skip!(PwDaemon::start(), "pipewire");
    let backend = PipewireBackend::connect(Some(daemon.name())).expect("connexion");

    let devices = backend.devices().expect("énumération");
    let sink = devices
        .iter()
        .find(|d| d.id.as_str() == NULL_SINK)
        .unwrap_or_else(|| panic!("Null-Sink absent de {devices:?}"));
    assert_eq!(sink.direction, DeviceDirection::Render);
    assert_eq!(sink.channels, 2);
    assert_eq!(sink.sample_rate, SampleRate::HZ_48000);
    assert_eq!(sink.default_block, 256);
    assert_eq!(sink.cable, None);
    assert_eq!(backend.name(), "pipewire");
    // Le Dummy-Driver n'a pas de `media.class` : il n'est pas un périphérique.
    assert!(devices.iter().all(|d| d.id.as_str() != "Dummy-Driver"));
}

#[test]
fn reports_added_and_removed_nodes() {
    let daemon = daemon_or_skip!(PwDaemon::start(), "pipewire");
    let mut backend = PipewireBackend::connect(Some(daemon.name())).expect("connexion");
    let events = backend.subscribe();

    // `pw-loopback` publie un nœud `Audio/Sink` qui vit aussi longtemps que le
    // processus : sa mort produit le retrait.
    let mut loopback = match daemon.tool(
        "pw-loopback",
        &["--capture-props=media.class=Audio/Sink node.name=Test-Sink audio.position=[FL FR]"],
    ) {
        Some(child) => child,
        None => {
            eprintln!("test sauté : pw-loopback introuvable");
            return;
        }
    };

    let added = wait_for(&events, Duration::from_secs(5), |event| match event {
        DeviceEvent::Added(info) if info.id.as_str() == "Test-Sink" => Some(info.clone()),
        _ => None,
    });
    let added = added.expect("événement Added pour Test-Sink");
    assert_eq!(added.direction, DeviceDirection::Render);
    assert!(backend
        .devices()
        .unwrap()
        .iter()
        .any(|d| d.id.as_str() == "Test-Sink"));

    let _ = loopback.kill();
    let _ = loopback.wait();

    let removed = wait_for(&events, Duration::from_secs(5), |event| match event {
        DeviceEvent::Removed { id } if id.as_str() == "Test-Sink" => Some(()),
        _ => None,
    });
    assert!(removed.is_some(), "événement Removed pour Test-Sink");
    assert!(backend
        .devices()
        .unwrap()
        .iter()
        .all(|d| d.id.as_str() != "Test-Sink"));
}

#[test]
fn render_stream_runs_between_start_and_stop() {
    let daemon = daemon_or_skip!(
        PwDaemon::start_with_session(),
        "pipewire ou wireplumber (le cadencement d'un flux exige un gestionnaire de session)"
    );
    let mut backend = PipewireBackend::connect(Some(daemon.name())).expect("connexion");
    let id = DeviceId::new(NULL_SINK);
    assert!(common::wait_until(Duration::from_secs(5), || backend
        .devices()
        .map(|d| d.iter().any(|i| i.id == id))
        .unwrap_or(false)));

    let calls = Arc::new(AtomicUsize::new(0));
    let faults = Arc::new(AtomicUsize::new(0));
    let expected = Arc::new(AtomicU64::new(0));
    let mut handle = {
        let calls = Arc::clone(&calls);
        let faults = Arc::clone(&faults);
        let expected = Arc::clone(&expected);
        backend
            .open(
                &id,
                format(),
                Box::new(move |io, clock| {
                    // `position` avance exactement de `frames` d'un rappel à l'autre.
                    if clock.position != expected.load(Ordering::Relaxed) {
                        faults.fetch_add(1, Ordering::Relaxed);
                    }
                    expected.store(clock.position + clock.frames as u64, Ordering::Relaxed);
                    match io.output.as_deref() {
                        Some(out) if out.len() == clock.frames * 2 => {}
                        _ => {
                            faults.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    io.silence_output();
                    calls.fetch_add(1, Ordering::Relaxed);
                }),
            )
            .expect("ouverture du Null-Sink")
    };

    assert_eq!(handle.format(), format());
    assert_eq!(handle.info().id, id);
    assert!(!handle.is_running());
    assert_eq!(handle.clock().position, 0);

    handle.start().expect("démarrage");
    assert!(handle.is_running());
    assert!(
        common::wait_until(Duration::from_secs(2), || calls.load(Ordering::Relaxed) > 0),
        "aucun rappel dans les 2 s"
    );

    handle.stop().expect("arrêt");
    assert!(!handle.is_running());
    let after_stop = calls.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        calls.load(Ordering::Relaxed),
        after_stop,
        "des rappels ont eu lieu après stop"
    );
    assert_eq!(
        faults.load(Ordering::Relaxed),
        0,
        "horloge ou tampon incohérent"
    );

    let clock = handle.clock();
    assert!(clock.position > 0);
    assert!(clock.frames > 0);
    assert!(clock.timestamp_ns > 0);

    // Un second flux sur le même périphérique est refusé tant que le premier vit.
    assert!(matches!(
        backend.open(&id, format(), Box::new(|_, _| {})),
        Err(BackendError::Busy(_))
    ));

    drop(handle);
    // Après fermeture, le périphérique est de nouveau ouvrable.
    let again = backend.open(&id, format(), Box::new(|_, _| {}));
    assert!(again.is_ok(), "réouverture refusée : {again:?}");
}

/// L'`Engine` détruit son backend avant ses poignées : la destruction ne doit pas
/// attendre une réponse d'un fil de boucle déjà arrêté.
#[test]
fn dropping_the_backend_before_the_handle_does_not_hang() {
    let daemon = daemon_or_skip!(PwDaemon::start(), "pipewire");
    let mut backend = PipewireBackend::connect(Some(daemon.name())).expect("connexion");
    let handle = backend
        .open(&DeviceId::new(NULL_SINK), format(), Box::new(|_, _| {}))
        .expect("ouverture du Null-Sink");
    let start = Instant::now();
    drop(backend);
    drop(handle);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "destruction trop lente : {:?}",
        start.elapsed()
    );
}

#[test]
fn connecting_to_a_missing_daemon_says_what_to_check() {
    common::runtime_dir();
    let name = format!("pipewire-conduit-absent-{}", std::process::id());
    let error = PipewireBackend::connect(Some(&name)).expect_err("le démon n'existe pas");
    let message = error.to_string();
    assert!(message.contains(&name), "{message}");
    assert!(message.contains("injoignable"), "{message}");
    assert!(message.contains("démarré"), "{message}");
}

#[test]
fn opening_an_unknown_device_is_not_found() {
    let daemon = daemon_or_skip!(PwDaemon::start(), "pipewire");
    let mut backend = PipewireBackend::connect(Some(daemon.name())).expect("connexion");
    let missing = DeviceId::new("ce-peripherique-nexiste-pas");
    let error = backend
        .open(&missing, format(), Box::new(|_, _| {}))
        .expect_err("périphérique inconnu");
    assert!(matches!(error, BackendError::NotFound(id) if id == missing));
    assert!(backend.cable_control().is_none());
    assert_eq!(backend.default_device(DeviceDirection::Capture), None);
}

/// Attend un événement satisfaisant `pick`, en ignorant les autres.
fn wait_for<T>(
    events: &conduit_backend::EventReceiver,
    timeout: Duration,
    mut pick: impl FnMut(&DeviceEvent) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(value) = pick(&event) {
                    return Some(value);
                }
            }
            Err(_) => return None,
        }
    }
    None
}
