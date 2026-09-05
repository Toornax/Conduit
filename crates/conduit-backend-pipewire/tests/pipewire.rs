//! Tests d'intégration du backend PipeWire contre un démon headless.
//!
//! Chaque test lance son propre démon (voir `common`). Sans le binaire `pipewire`
//! dans le `PATH` — c'est-à-dire hors du devshell Nix — les tests se sautent avec un
//! message plutôt que d'échouer.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

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
    let deadline = std::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
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
