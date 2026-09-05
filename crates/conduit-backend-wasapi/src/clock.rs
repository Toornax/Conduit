//! Horloge d'un flux : `IAudioClock` et compteur de performance (QPC).
//!
//! `IAudioClient::GetService(IAudioClock)` donne accès à l'horloge du flux :
//!
//! - `GetFrequency` rend la fréquence de l'horloge en **unités par seconde**, dans
//!   les mêmes unités que la position. Microsoft ne fixe pas ces unités : en mode
//!   partagé le moteur audio annonce des **octets par seconde** — du format de
//!   mixage sur le chemin `IAudioClient3` (`nSamplesPerSec × nBlockAlign`, 384 000
//!   pour 48 kHz stéréo float32), du **format client** sur le chemin par conversion
//!   (176 400 pour 44,1 kHz mono float32) —, certains pilotes des trames par
//!   seconde, d'autres une valeur quelconque. [`ClockScale::new`] classe la
//!   fréquence ([`ClockUnits`]) pour le diagnostic et convertit toujours par le
//!   rapport générique `position × fréquence_demandée / fréquence_horloge`, exact
//!   dans tous les cas : la position publiée est en **trames du format livré** au
//!   rappel, quelle que soit l'unité du pilote ;
//! - `GetPosition(&position, &qpc)` rend la position **de lecture** (rendu) ou
//!   d'écriture (capture) du matériel, et la valeur du compteur de performance au
//!   moment de cette lecture, en unités de 100 ns. Ce compteur est le
//!   `QueryPerformanceCounter` du système : tous les flux du processus (et tous
//!   ses fils) partagent cette base, ce qui permet de comparer deux cartes.
//!
//! Le repli, quand `IAudioClock` manque ou que `GetFrequency` échoue, est le
//! compteur de trames livrées, horodaté par [`Qpc::now_ns`] pour garder la même
//! base de temps.
//!
//! Rien ici n'alloue : les conversions sont des entiers, `QueryPerformanceCounter`
//! lit un registre.

use core::fmt;

use conduit_backend::BackendError;
use windows::Win32::Media::Audio::IAudioClock;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::com::platform_error;

/// Unités dans lesquelles `IAudioClock` compte, déduites de `GetFrequency`.
///
/// Observé sur ce poste : `MixBytes` sur le chemin basse latence
/// (`IAudioClient3`, 48 000 × 8 = 384 000), `StreamBytes` sur le chemin par
/// conversion (44,1 kHz mono float32 : 44 100 × 4 = 176 400).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockUnits {
    /// Trames par seconde du format de mixage ou du format livré
    /// (`fréquence == mix_rate` ou `== sample_rate`).
    Frames,
    /// Octets par seconde du format de mixage
    /// (`fréquence == mix_rate × nBlockAlign` du mixage).
    MixBytes,
    /// Octets par seconde du format livré au rappel, float32 entrelacé
    /// (`fréquence == sample_rate × channels × 4`).
    StreamBytes,
    /// Autre chose : converti par le rapport générique.
    Other,
}

impl fmt::Display for ClockUnits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ClockUnits::Frames => "trames/s",
            ClockUnits::MixBytes => "octets/s du format de mixage",
            ClockUnits::StreamBytes => "octets/s du format livré",
            ClockUnits::Other => "unités/s (rapport générique)",
        })
    }
}

/// D'où vient la position d'horloge d'un flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    /// `IAudioClock` indisponible : compteur de trames livrées, horodaté par le
    /// compteur de performance.
    Counter,
    /// `IAudioClock::GetPosition` à chaque rappel.
    AudioClock {
        /// `GetFrequency`, unités par seconde.
        frequency: u64,
        /// Lecture de cette fréquence.
        units: ClockUnits,
    },
}

impl fmt::Display for ClockSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClockSource::Counter => f.write_str("compteur de trames (IAudioClock indisponible)"),
            ClockSource::AudioClock { frequency, units } => {
                write!(f, "IAudioClock à {frequency} {units}")
            }
        }
    }
}

/// Conversion d'une position `IAudioClock` en trames du format livré. `Copy` et
/// sans allocation : vit sur le fil du flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockScale {
    /// Unités par seconde de l'horloge (`GetFrequency`), jamais nulle.
    frequency: u64,
    /// Trames par seconde du format livré au rappel.
    sample_rate: u32,
}

impl ClockScale {
    /// Classe `frequency` par rapport au format de mixage (`mix_rate`,
    /// `mix_block_align`) et au format livré (`sample_rate`, `channels` float32),
    /// et construit la conversion vers `sample_rate`.
    pub(crate) fn new(
        frequency: u64,
        sample_rate: u32,
        channels: u16,
        mix_rate: u32,
        mix_block_align: u16,
    ) -> (Self, ClockUnits) {
        let stream_block_align = u64::from(channels) * 4;
        let units = if frequency == u64::from(mix_rate) || frequency == u64::from(sample_rate) {
            ClockUnits::Frames
        } else if frequency == u64::from(mix_rate) * u64::from(mix_block_align) {
            ClockUnits::MixBytes
        } else if frequency == u64::from(sample_rate) * stream_block_align {
            ClockUnits::StreamBytes
        } else {
            ClockUnits::Other
        };
        (
            Self {
                frequency: frequency.max(1),
                sample_rate,
            },
            units,
        )
    }

    /// Conversion identité : le compteur de trames livrées est déjà en trames.
    pub(crate) fn identity(sample_rate: u32) -> Self {
        Self {
            frequency: u64::from(sample_rate.max(1)),
            sample_rate,
        }
    }

    /// `position × sample_rate / frequency`, en 128 bits pour ne jamais déborder.
    pub(crate) fn frames(self, position: u64) -> u64 {
        if self.frequency == u64::from(self.sample_rate) {
            return position;
        }
        let frames =
            u128::from(position) * u128::from(self.sample_rate) / u128::from(self.frequency);
        u64::try_from(frames).unwrap_or(u64::MAX)
    }
}

/// Compteur de performance du système : la base de temps commune à tous les flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Qpc {
    /// Ticks par seconde (`QueryPerformanceFrequency`), jamais nul.
    frequency: u64,
}

impl Qpc {
    /// Lit la fréquence du compteur (constante pendant la vie du système).
    pub(crate) fn query() -> Result<Self, BackendError> {
        let mut frequency = 0i64;
        // SAFETY: `frequency` est une locale vivante ; l'appel n'a pas d'autre effet.
        unsafe { QueryPerformanceFrequency(&mut frequency) }
            .map_err(|e| platform_error("QueryPerformanceFrequency", &e))?;
        Ok(Self {
            frequency: u64::try_from(frequency).unwrap_or(0).max(1),
        })
    }

    /// Nanosecondes du compteur, maintenant. Sans allocation ni verrou.
    pub(crate) fn now_ns(&self) -> u64 {
        let mut ticks = 0i64;
        // SAFETY: `ticks` est une locale vivante. Depuis Windows XP l'appel ne peut
        // pas échouer ; en cas d'échec on rend 0 plutôt que de paniquer.
        if unsafe { QueryPerformanceCounter(&mut ticks) }.is_err() {
            return 0;
        }
        let ticks = u64::try_from(ticks).unwrap_or(0);
        ticks_to_ns(ticks, self.frequency)
    }
}

/// `ticks × 1e9 / frequency`, en 128 bits.
fn ticks_to_ns(ticks: u64, frequency: u64) -> u64 {
    let ns = u128::from(ticks) * 1_000_000_000 / u128::from(frequency.max(1));
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// Nanosecondes d'un horodatage `IAudioClock` (unités de 100 ns).
pub(crate) fn hns_to_ns(hns: u64) -> u64 {
    hns.saturating_mul(100)
}

/// `IAudioClock::GetPosition` : position brute et horodatage QPC en 100 ns.
/// Sans allocation (deux sorties sur la pile).
pub(crate) fn read_position(clock: &IAudioClock) -> Result<(u64, u64), windows::core::Error> {
    let mut position = 0u64;
    let mut qpc = 0u64;
    // SAFETY: les deux sorties sont des locales vivantes ; le flux est initialisé
    // (l'horloge vient de son `IAudioClient`).
    unsafe { clock.GetPosition(&mut position, Some(&mut qpc)) }?;
    Ok((position, qpc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_is_classified_against_both_formats() {
        let (scale, units) = ClockScale::new(48_000, 48_000, 2, 48_000, 8);
        assert_eq!(units, ClockUnits::Frames);
        assert_eq!(scale.frames(4_800), 4_800);
        // Trames/s du format livré (44,1 kHz demandé sur un mixage 48 kHz).
        let (scale, units) = ClockScale::new(44_100, 44_100, 1, 48_000, 8);
        assert_eq!(units, ClockUnits::Frames);
        assert_eq!(scale.frames(4_410), 4_410);

        // Chemin basse latence observé : octets/s du mixage.
        let (scale, units) = ClockScale::new(384_000, 48_000, 2, 48_000, 8);
        assert_eq!(units, ClockUnits::MixBytes);
        assert_eq!(scale.frames(384_000), 48_000);
        assert_eq!(scale.frames(8), 1);

        // Chemin par conversion observé : octets/s du format livré (mono float32).
        let (scale, units) = ClockScale::new(176_400, 44_100, 1, 48_000, 8);
        assert_eq!(units, ClockUnits::StreamBytes);
        assert_eq!(scale.frames(176_400), 44_100);
        assert_eq!(scale.frames(4), 1);

        let (scale, units) = ClockScale::new(10_000_000, 48_000, 2, 48_000, 8);
        assert_eq!(units, ClockUnits::Other);
        assert_eq!(scale.frames(10_000_000), 48_000);
    }

    #[test]
    fn positions_are_expressed_in_the_delivered_format() {
        // Mixage 48 kHz stéréo float (octets/s), flux livré en 44,1 kHz stéréo :
        // une seconde d'horloge vaut 44 100 trames livrées.
        let (scale, units) = ClockScale::new(384_000, 44_100, 2, 48_000, 8);
        assert_eq!(units, ClockUnits::MixBytes);
        assert_eq!(scale.frames(384_000), 44_100);
        // Une heure sans déborder, et une position énorme non plus.
        assert_eq!(scale.frames(384_000 * 3_600), 44_100 * 3_600);
        let expected = u128::from(u64::MAX) * 44_100 / 384_000;
        assert_eq!(u128::from(scale.frames(u64::MAX)), expected);
    }

    #[test]
    fn identity_and_degenerate_frequencies() {
        assert_eq!(ClockScale::identity(48_000).frames(123), 123);
        let (scale, _) = ClockScale::new(0, 48_000, 2, 48_000, 8);
        assert_eq!(scale.frames(7), 7 * 48_000);
        assert_eq!(ticks_to_ns(10_000_000, 10_000_000), 1_000_000_000);
        assert_eq!(ticks_to_ns(5, 0), 5_000_000_000);
        assert_eq!(hns_to_ns(3), 300);
        assert_eq!(hns_to_ns(u64::MAX), u64::MAX);
    }

    #[test]
    fn qpc_is_monotone_and_close_to_instant() {
        let qpc = Qpc::query().expect("QueryPerformanceFrequency");
        let a = qpc.now_ns();
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let elapsed = started.elapsed().as_nanos() as u64;
        let b = qpc.now_ns();
        assert!(b > a);
        let measured = b - a;
        assert!(
            measured.abs_diff(elapsed) < 10_000_000,
            "QPC {measured} ns contre Instant {elapsed} ns"
        );
    }

    #[test]
    fn sources_display() {
        assert!(ClockSource::Counter.to_string().contains("compteur"));
        let s = ClockSource::AudioClock {
            frequency: 384_000,
            units: ClockUnits::MixBytes,
        };
        assert_eq!(
            s.to_string(),
            "IAudioClock à 384000 octets/s du format de mixage"
        );
        assert!(ClockUnits::Other.to_string().contains("générique"));
    }
}
