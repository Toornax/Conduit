//! Capture en **écho** du rendu (`AUDCLNT_STREAMFLAGS_LOOPBACK`), contre les
//! cartes son de la machine.
//!
//! Contrairement aux autres tests de flux, [`echo_entend_le_sinus`] **émet un
//! son** : c'est tout son propos — on ne peut pas prouver que l'écho prélève le
//! mélange du moteur sans rien mélanger. Le sinus est à **2 % d'amplitude**
//! (−34 dBFS) pendant une seconde et demie sur le rendu par défaut : audible si
//! le volume est haut, inoffensif.
//!
//! Le test se saute proprement s'il n'y a pas de périphérique de rendu par défaut.

#![cfg(windows)]

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use conduit_backend::{
    Backend, BackendError, DeviceDirection, DeviceHandle, StreamFormat, StreamIo,
};
use conduit_backend_wasapi::{ExclusivePolicy, ShareMode, WasapiBackend};
use conduit_core::types::SampleRate;

/// Amplitude crête du sinus joué pendant l'écho.
const AMPLITUDE: f64 = 0.02;
/// Fréquence du sinus.
const FREQ_HZ: f64 = 440.0;
/// Durée d'une passe d'écho.
const PLAY: Duration = Duration::from_millis(1_500);

/// Un poste sans le périphérique demandé ne peut pas exercer le test.
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

/// Ce que le rappel d'écho mesure : trames reçues et somme des carrés (× 2^20,
/// pour tenir dans un atomique entier sans allocation ni verrou).
#[derive(Default)]
struct Echo {
    frames: AtomicUsize,
    energy: AtomicU64,
    peak_millis: AtomicU64,
}

impl Echo {
    fn rms(&self) -> f64 {
        let frames = self.frames.load(Ordering::Relaxed);
        if frames == 0 {
            return 0.0;
        }
        let energy = self.energy.load(Ordering::Relaxed) as f64 / f64::from(1u32 << 20);
        (energy / frames as f64).sqrt()
    }

    fn peak(&self) -> f64 {
        self.peak_millis.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

#[test]
#[ignore = "émet un son sur le rendu par défaut : à lancer à la main (cargo test … -- --ignored)"]
fn echo_entend_le_sinus() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let info = backend
        .devices()
        .expect("énumération")
        .into_iter()
        .find(|d| d.id == id)
        .expect("le rendu par défaut est énuméré");
    // Le format de mixage du moteur : ce que l'écho prélève sans conversion.
    let format = StreamFormat {
        sample_rate: info.sample_rate,
        channels: info.channels,
        block_frames: 480,
    };
    let channels = info.channels.max(1);

    let echo = Arc::new(Echo::default());
    let sink = Arc::clone(&echo);
    let mut capture = backend
        .open_loopback(
            &id,
            format,
            Box::new(move |io: &mut StreamIo<'_>, _| {
                let Some(input) = io.input else { return };
                let mut energy = 0.0f64;
                let mut peak = 0.0f64;
                for &sample in input {
                    let sample = f64::from(sample);
                    energy += sample * sample;
                    peak = peak.max(sample.abs());
                }
                // Un seul canal compte dans la moyenne : l'énergie est ramenée à la
                // trame.
                sink.frames
                    .fetch_add(input.len() / channels, Ordering::Relaxed);
                sink.energy.fetch_add(
                    (energy / channels as f64 * f64::from(1u32 << 20)) as u64,
                    Ordering::Relaxed,
                );
                sink.peak_millis
                    .fetch_max((peak * 1000.0) as u64, Ordering::Relaxed);
            }),
        )
        .expect("ouverture de l'écho du rendu par défaut");

    // L'écho porte le `DeviceInfo` de l'endpoint de rendu, et reste partagé.
    assert_eq!(capture.info().direction, DeviceDirection::Render);
    assert_eq!(capture.share_mode(), ShareMode::Shared);
    assert!(
        capture.latency().path.is_loopback(),
        "chemin d'ouverture inattendu : {:?}",
        capture.latency().path
    );

    let omega = TAU * FREQ_HZ / f64::from(format.sample_rate.hz());
    let mut phase = 0.0f64;
    let mut render = backend
        .open_handle(
            &id,
            format,
            Box::new(move |io: &mut StreamIo<'_>, _| {
                let Some(out) = io.output.as_deref_mut() else {
                    return;
                };
                for frame in out.chunks_mut(channels) {
                    let value = (AMPLITUDE * phase.sin()) as f32;
                    frame.fill(value);
                    phase = (phase + omega) % TAU;
                }
            }),
        )
        .expect("ouverture du rendu par défaut");

    capture.start().expect("démarrage de l'écho");
    render.start().expect("démarrage du rendu");
    sleep(PLAY);
    render.stop().expect("arrêt du rendu");
    sleep(Duration::from_millis(50));
    capture.stop().expect("arrêt de l'écho");

    let frames = echo.frames.load(Ordering::Relaxed);
    let rms = echo.rms();
    eprintln!(
        "écho de « {} » : {frames} trames, RMS {rms:.4} (attendu ≈ {:.4}), crête {:.4}",
        info.name,
        AMPLITUDE / 2f64.sqrt(),
        echo.peak()
    );
    // Une seconde et demie de mélange : le moteur en a livré l'essentiel.
    assert!(
        frames > format.sample_rate.hz() as usize,
        "l'écho n'a livré que {frames} trames en {} ms : l'événement du tampon ne se \
         déclenche-t-il pas ?",
        PLAY.as_millis()
    );
    // Le sinus est là. La marge est large : le mélangeur de volume de Windows peut
    // atténuer ce qui passe dans l'écho, mais pas l'effacer.
    let attendu = AMPLITUDE / 2f64.sqrt();
    assert!(
        rms > attendu / 4.0,
        "l'écho est (presque) silencieux : RMS {rms:.5} pour {attendu:.5} attendus — le moteur \
         audio ne délivre rien vers cet endpoint, ou son volume est à zéro"
    );
}

#[test]
fn echo_refuse_le_mode_exclusif() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Render);
    let format = StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 480,
    };
    for policy in [ExclusivePolicy::Preferred, ExclusivePolicy::Required] {
        backend.set_exclusive_policy(policy);
        let error = backend
            .open_loopback(
                &id,
                format,
                Box::new(|io: &mut StreamIo<'_>, _| io.silence_output()),
            )
            .expect_err("écho + exclusif");
        let text = error.to_string();
        assert!(text.contains("mode partagé"), "{text}");
        assert!(
            matches!(error, BackendError::UnsupportedFormat { .. }),
            "{text}"
        );
    }
}

#[test]
fn echo_refuse_un_endpoint_de_capture() {
    let mut backend = backend();
    let id = default_or_skip!(backend, DeviceDirection::Capture);
    let format = StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 480,
    };
    let error = backend
        .open_loopback(
            &id,
            format,
            Box::new(|io: &mut StreamIo<'_>, _| io.silence_output()),
        )
        .expect_err("écho sur une capture");
    assert!(error.to_string().contains("endpoint de rendu"), "{error}");
}
