//! Tests d'intégration du backend WASAPI contre les cartes son de la machine.
//!
//! Ils supposent au moins un endpoint de rendu actif (n'importe quel poste Windows
//! avec une sortie audio). Le critère « Fait quand » de M1b-30 — branchement d'un
//! casque USB → `DeviceEvent::Added` — demande une main humaine : c'est le test
//! `#[ignore]` [`hotplug_produces_added_then_removed`].

#![cfg(windows)]

use std::time::{Duration, Instant};

use conduit_backend::{
    Backend, CableControl, CableError, CableId, CableInfo, CableSpec, DeviceDirection, DeviceEvent,
};
use conduit_backend_wasapi::WasapiBackend;
use conduit_core::types::{ChannelCount, SampleRate};

/// Un poste sans aucune sortie audio (session distante, VM sans carte son) ne peut
/// pas exercer ces tests : on le dit plutôt que d'échouer.
macro_rules! backend_or_skip {
    () => {
        match WasapiBackend::new() {
            Ok(backend) if !backend.devices().unwrap_or_default().is_empty() => backend,
            Ok(_) => {
                eprintln!("test sauté : aucun endpoint audio actif sur cette machine");
                return;
            }
            Err(e) => panic!("WasapiBackend::new : {e}"),
        }
    };
}

#[test]
fn backend_starts_and_enumerates_plausible_devices() {
    let backend = backend_or_skip!();
    assert_eq!(backend.name(), "wasapi");
    let devices = backend.devices().expect("énumération");
    assert!(!devices.is_empty());
    for device in &devices {
        assert!(
            !device.id.as_str().is_empty(),
            "identifiant vide : {device:?}"
        );
        assert!(!device.name.trim().is_empty(), "nom vide : {device:?}");
        assert!(device.channels >= 1, "aucun canal : {device:?}");
        let hz = device.sample_rate.hz();
        assert!(
            (8_000..=384_000).contains(&hz),
            "fréquence invraisemblable : {device:?}"
        );
        assert!(
            device.supports_rate(device.sample_rate),
            "la fréquence native est toujours acceptée : {device:?}"
        );
        for rate in &device.sample_rates {
            assert!(
                [
                    SampleRate::HZ_44100,
                    SampleRate::HZ_48000,
                    SampleRate::HZ_96000
                ]
                .contains(rate),
                "fréquence hors des valeurs sondées : {device:?}"
            );
        }
        // 10 ms à 8 kHz font 80 trames ; 10 ms à 384 kHz, 3840. La période par
        // défaut du moteur est entre 1 et 100 ms.
        assert!(
            (1..=38_400).contains(&device.default_block),
            "bloc invraisemblable : {device:?}"
        );
        // L'identité du câble vient de la **description** de l'endpoint
        // (`PKEY_Device_DeviceDesc`, « Conduit 1 ») ; le nom publié est la
        // composition que Windows affiche (« Conduit 1 (Conduit — câbles audio
        // virtuels) »). Les deux doivent parler du même câble : sur du matériel
        // réel, c'est la seule vérification qui les confronte.
        assert!(
            device.cable.is_none_or(|cable| {
                // Pas un `starts_with` du seul nom de câble : « Conduit 1 » est le
                // préfixe de « Conduit 16 ».
                let nom = cable.to_string();
                device.name == nom || device.name.starts_with(&format!("{nom} ("))
            }),
            "câble reconnu sur un endpoint dont le nom affiché dit autre chose : {device:?}"
        );
    }
    // Les identifiants d'endpoint sont uniques.
    let mut ids: Vec<_> = devices.iter().map(|d| d.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        devices.len(),
        "identifiants en double : {devices:?}"
    );
}

#[test]
fn default_render_device_is_listed_and_flagged() {
    let backend = backend_or_skip!();
    let devices = backend.devices().expect("énumération");
    for direction in [DeviceDirection::Render, DeviceDirection::Capture] {
        let default = backend.default_device(direction);
        let flagged: Vec<_> = devices
            .iter()
            .filter(|d| d.direction == direction && d.is_default)
            .collect();
        match default {
            Some(id) => {
                assert_eq!(flagged.len(), 1, "un seul défaut par sens : {flagged:?}");
                assert_eq!(flagged[0].id, id);
            }
            None => assert!(
                flagged.is_empty(),
                "défaut marqué sans défaut : {flagged:?}"
            ),
        }
    }
    // Le critère du brief : le rendu a un défaut sur toute machine qui a une sortie.
    if devices
        .iter()
        .any(|d| d.direction == DeviceDirection::Render)
    {
        assert!(backend.default_device(DeviceDirection::Render).is_some());
    }
}

#[test]
fn enumeration_is_stable() {
    let backend = backend_or_skip!();
    let first = backend.devices().expect("première énumération");
    let second = backend.devices().expect("seconde énumération");
    assert_eq!(first, second);
}

/// Un dorsal neuf n'a pas de contrôle des câbles : c'est le démon qui l'installe (M1b-34).
///
/// Le contrôle vit dans `conduit-helper`, qui dépend déjà de ce crate pour le transport
/// KS ; l'inverse ferait un cycle entre paquets. Ce test vérifie les deux moitiés du
/// contrat sans parler au service : rien n'est installé, aucun canal n'est ouvert, et le
/// faux contrôle ne connaît aucun câble.
#[test]
fn cable_control_is_installed_by_the_daemon() {
    let mut backend = backend_or_skip!();
    assert!(
        backend.cable_control().is_none(),
        "un dorsal neuf ne pilote aucun câble"
    );
    assert_eq!(CableControl::max_cables(&backend), 0);
    // Une opération sur un dorsal non câblé dit **le montage manquant**, pas une panne.
    let refus = CableControl::list(&backend).expect_err("aucun contrôle installé");
    assert!(matches!(refus, CableError::Unavailable(_)), "{refus:?}");
    assert!(
        refus.to_string().contains("service d'assistance"),
        "{refus}"
    );

    backend.set_cable_control(Box::new(SansCable));
    assert!(backend.cable_control().is_some());
    assert_eq!(CableControl::max_cables(&backend), 16);
    assert_eq!(CableControl::list(&backend).expect("liste"), Vec::new());
    assert!(matches!(
        CableControl::get(&backend, CableId(1)),
        Err(CableError::NotFound(CableId(1)))
    ));
    // Le format passe par le même câblage que les autres écritures : sans contrôle
    // installé, il dit le montage manquant ; avec le double, il dit le câble manquant.
    assert!(matches!(
        CableControl::set_format(
            &mut backend,
            CableId(1),
            conduit_backend::CableFormat::default()
        ),
        Err(CableError::NotFound(CableId(1)))
    ));
}

/// Un contrôle des câbles qui ne connaît aucun câble : de quoi vérifier le câblage du
/// dorsal sans service, sans pilote et sans rien installer.
#[derive(Debug)]
struct SansCable;

impl CableControl for SansCable {
    fn max_cables(&self) -> usize {
        16
    }
    fn list(&self) -> Result<Vec<CableInfo>, CableError> {
        Ok(Vec::new())
    }
    fn create(&mut self, _spec: CableSpec) -> Result<CableInfo, CableError> {
        Err(CableError::LimitReached { max: 0 })
    }
    fn remove(&mut self, id: CableId) -> Result<(), CableError> {
        Err(CableError::NotFound(id))
    }
    fn set_channels(
        &mut self,
        id: CableId,
        _channels: ChannelCount,
    ) -> Result<CableInfo, CableError> {
        Err(CableError::NotFound(id))
    }
    /// Aucun câble : aucun format à régler. Le trait n'a pas d'implémentation par défaut
    /// pour `set_format`, et c'est ce qui oblige ce double à le dire au lieu de le taire.
    fn set_format(
        &mut self,
        id: CableId,
        _format: conduit_backend::CableFormat,
    ) -> Result<CableInfo, CableError> {
        Err(CableError::NotFound(id))
    }
    fn rename(&mut self, id: CableId, _name: &str) -> Result<CableInfo, CableError> {
        Err(CableError::NotFound(id))
    }
}

/// `DeviceInfo` décrit le format de mixage : tout périphérique dont l'`IAudioClient`
/// s'active annonce les trois fréquences (conversion automatique en mode partagé).
#[test]
fn devices_announce_the_shared_mode_rates() {
    let backend = backend_or_skip!();
    for device in backend.devices().expect("énumération") {
        assert!(
            device.sample_rates.is_empty()
                || device.sample_rates == conduit_backend_wasapi::PROBED_RATES.to_vec(),
            "fréquences inattendues : {device:?}"
        );
        assert!(device.channels >= 1);
    }
}

#[test]
fn subscribe_then_drop_does_not_block() {
    let started = Instant::now();
    let worker = std::thread::spawn(|| {
        let mut backend = match WasapiBackend::new() {
            Ok(backend) => backend,
            Err(e) => panic!("WasapiBackend::new : {e}"),
        };
        let events = backend.subscribe();
        drop(backend);
        // Le fil est parti : le récepteur le voit sans attendre.
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
    });
    // Attente bornée : un `Drop` qui bloquerait ferait échouer le test au lieu de
    // suspendre la suite.
    while !worker.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "la destruction du backend bloque"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.join().expect("le fil de test a paniqué");
}

#[test]
fn several_backends_coexist() {
    // Deux fils MMDevice, deux appartements COM, deux clients de notification.
    let a = backend_or_skip!();
    let b = WasapiBackend::new().expect("second backend");
    assert_eq!(a.devices().unwrap(), b.devices().unwrap());
}

/// Critère « Fait quand » de M1b-30 : brancher un casque USB (ou activer un
/// périphérique dans le panneau Son) pendant les 30 s produit `Added`, le
/// débrancher produit `Removed`.
///
/// ```sh
/// cargo test -p conduit-backend-wasapi --test wasapi -- --ignored --nocapture
/// ```
#[test]
#[ignore = "demande de brancher puis débrancher un périphérique audio à la main"]
fn hotplug_produces_added_then_removed() {
    let mut backend = backend_or_skip!();
    let events = backend.subscribe();
    eprintln!("branchez un périphérique audio dans les 30 s…");
    let added = wait_for(&events, Duration::from_secs(30), |e| match e {
        DeviceEvent::Added(info) => Some(info.clone()),
        _ => None,
    })
    .expect("aucun DeviceEvent::Added en 30 s");
    eprintln!("ajouté : {} — débranchez-le dans les 30 s…", added.name);
    assert!(backend.devices().unwrap().iter().any(|d| d.id == added.id));
    let removed = wait_for(&events, Duration::from_secs(30), |e| match e {
        DeviceEvent::Removed { id } if *id == added.id => Some(()),
        _ => None,
    });
    assert!(
        removed.is_some(),
        "aucun DeviceEvent::Removed pour {}",
        added.id
    );
    assert!(backend.devices().unwrap().iter().all(|d| d.id != added.id));
}

fn wait_for<T>(
    events: &conduit_backend::EventReceiver,
    timeout: Duration,
    mut pick: impl FnMut(&DeviceEvent) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match events.recv_timeout(left) {
            Ok(event) => {
                eprintln!("événement : {event:?}");
                if let Some(value) = pick(&event) {
                    return Some(value);
                }
            }
            Err(_) => return None,
        }
    }
}
