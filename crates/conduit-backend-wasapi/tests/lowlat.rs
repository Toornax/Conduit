//! Ouverture partagée **faible latence** (`IAudioClient3`) sur le matériel de la
//! machine.
//!
//! Ces tests **ouvrent de vrais flux** : ils sont donc `#[ignore]`, et ne se lancent
//! qu'à la main (`cargo test -p conduit-backend-wasapi --test lowlat -- --ignored
//! --nocapture`) sur la machine où la mesure a un sens — celle où le pilote Conduit
//! est installé. Aucun n'émet de son : les rappels n'écrivent que du silence, et
//! [`shared_period_default_changes_nothing`] ne démarre même pas les flux.
//!
//! Ce qu'ils documentent est la question de M1b : le moteur audio de Windows monte
//! peut-être les **notifications** du pilote (paquets WaveRT) seulement pour un flux
//! faible latence. La réponse ne se lit pas ici mais dans le pilote, par
//! `conduit-looptest --cable-transport`, pendant qu'une passe
//! `conduit-looptest --faible-latence` tourne.

#![cfg(windows)]

use std::time::Duration;

use conduit_backend::{Backend, DeviceDirection, DeviceHandle, StreamFormat};
use conduit_backend_wasapi::{InitPath, SharedPeriod, WasapiBackend};

/// Un rappel qui n'écrit que du silence : rien ne s'entend.
fn silence() -> conduit_backend::AudioCallback {
    Box::new(|io, _| io.silence_output())
}

/// La politique par défaut est bien le comportement de toujours, et cela se vérifie
/// **sans ouvrir aucun flux** : c'est la garantie que `conduitd` n'a rien changé.
#[test]
fn shared_period_default_changes_nothing() {
    let Ok(backend) = WasapiBackend::new() else {
        eprintln!("test sauté : backend WASAPI indisponible");
        return;
    };
    assert_eq!(backend.shared_period(), SharedPeriod::Default);
    assert!(!backend.shared_period().forces());
    assert!(format!("{backend:?}").contains("shared_period"));
}

/// **Ouvre un flux** sur le rendu par défaut, en période minimale, et imprime ce que
/// le moteur a accordé : les quatre périodes annoncées, celle qu'il sert, le tampon.
///
/// Le flux est ouvert au **format de mixage** de l'endpoint — la politique forcée
/// l'exige, faute de conversion automatique sur ce chemin —, et démarré une demi-
/// seconde pour que le pilote voie passer des réveils : c'est pendant ce temps que
/// `conduit-looptest --cable-transport` a quelque chose à relever.
#[test]
#[ignore = "ouvre un vrai flux WASAPI sur le rendu par défaut : à lancer à la main (cargo test … -- --ignored)"]
fn minimal_period_reports_what_the_engine_grants() {
    let mut backend = match WasapiBackend::new() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("test sauté : backend WASAPI indisponible ({e})");
            return;
        }
    };
    let Some(id) = backend.default_device(DeviceDirection::Render) else {
        eprintln!("test sauté : aucun périphérique de rendu par défaut");
        return;
    };
    // Le format de mixage de cet endpoint, tel que l'énumération le publie : c'est
    // le seul que la politique forcée accepte.
    let Some(device) = backend
        .devices()
        .ok()
        .and_then(|d| d.into_iter().find(|d| d.id == id))
    else {
        eprintln!("test sauté : endpoint de rendu par défaut non décrit");
        return;
    };
    let format = StreamFormat {
        sample_rate: device.sample_rate,
        channels: device.channels,
        block_frames: 480,
    };

    backend.set_shared_period(SharedPeriod::Minimal);
    let mut handle = match backend.open_handle(&id, format, silence()) {
        Ok(handle) => handle,
        Err(e) => {
            // Un refus est un fait à consigner, pas un échec de Conduit : tous les
            // pilotes n'exposent pas `IAudioClient3`, et un autre flux peut déjà
            // tenir la périodicité du moteur.
            eprintln!("« {} » refuse la faible latence : {e}", device.name);
            return;
        }
    };
    let latency = handle.latency();
    let (periods, current) = handle
        .engine_periods()
        .expect("un flux faible latence publie ses périodes");
    eprintln!(
        "« {} » : périodes du moteur (trames) défaut {}, fondamentale {}, min {}, max {} ; \
         demandée {} ; servie {current} ; tampon {} trames ; {:?}",
        device.name,
        periods.default,
        periods.fundamental,
        periods.min,
        periods.max,
        latency.period_frames,
        latency.buffer_frames,
        latency.path
    );
    assert!(matches!(latency.path, InitPath::LowLatency { .. }));
    assert_eq!(latency.period_frames, periods.min as usize);
    assert!(latency.buffer_frames >= latency.period_frames);

    handle.start().expect("démarrage du flux faible latence");
    std::thread::sleep(Duration::from_millis(500));
    handle.stop().expect("arrêt du flux faible latence");
}
