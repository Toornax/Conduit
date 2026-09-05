//! Horloge virtuelle et positions de flux (`docs/driver-design.md` §5.1).
//!
//! Le pilote n'a qu'un référentiel de temps : le compteur de performance
//! (`KeQueryPerformanceCounter`, fréquence fixe `qpc_freq`). La position d'un flux
//! WaveRT n'est jamais *comptée* par un timer, elle est *calculée* :
//!
//! ```text
//! frames = ((qpc_now − qpc_start) × sample_rate) / qpc_freq      (u128)
//! bytes  = (frames × frame_bytes) mod buffer_bytes
//! ```
//!
//! `GetPosition` et `GetPresentationPosition` sont donc exacts à la trame près.
//! Une pause/reprise accumule les trames des périodes de `RUN` précédentes
//! ([`StreamPosition`]).
//!
//! Tout ici est appelable à `DISPATCH_LEVEL` : aucune allocation, aucune panique,
//! arithmétique 128 bits pour que `qpc_now = u64::MAX` ne déborde pas.

/// Horloge virtuelle : convertit des ticks du compteur de performance en trames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualClock {
    qpc_freq: u64,
    sample_rate: u32,
}

impl VirtualClock {
    /// Crée une horloge de fréquence `qpc_freq` ticks/s pour un flux à `sample_rate`
    /// trames/s. Renvoie `None` si l'une des deux vaut 0 (division impossible).
    pub const fn new(qpc_freq: u64, sample_rate: u32) -> Option<Self> {
        if qpc_freq == 0 || sample_rate == 0 {
            return None;
        }
        Some(Self {
            qpc_freq,
            sample_rate,
        })
    }

    /// Fréquence du compteur de performance, en ticks par seconde.
    pub const fn qpc_freq(&self) -> u64 {
        self.qpc_freq
    }

    /// Fréquence d'échantillonnage, en trames par seconde.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Nombre de trames écoulées entre `qpc_start` et `qpc_now`.
    ///
    /// Calcul `((now − start) × rate) / freq` en `u128` : le produit d'un `u64` par un
    /// `u32` tient toujours. Renvoie 0 si `qpc_now < qpc_start` (compteur lu avant le
    /// départ, ne doit pas arriver) et sature à `u64::MAX` si le quotient dépasse
    /// (impossible avec une fréquence ≥ 1 Hz, mais jamais de panique).
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn frames_between(&self, qpc_start: u64, qpc_now: u64) -> u64 {
        let elapsed = u128::from(qpc_now.saturating_sub(qpc_start));
        let frames = elapsed
            // u64 × u32 < 2^96 : ne sature jamais, mais jamais de débordement silencieux.
            .saturating_mul(u128::from(self.sample_rate))
            .checked_div(u128::from(self.qpc_freq))
            // `qpc_freq ≠ 0` par construction : jamais `None`.
            .unwrap_or(0);
        u64::try_from(frames).unwrap_or(u64::MAX)
    }
}

/// Position d'un flux WaveRT : trames jouées, pauses comprises.
///
/// À `SetState(KSSTATE_RUN)` le pilote appelle [`run`](Self::run) avec le compteur
/// courant ; à `KSSTATE_PAUSE`, [`pause`](Self::pause) ajoute les trames écoulées
/// à l'accumulateur ; à `KSSTATE_STOP`, [`reset`](Self::reset). Entre-temps,
/// [`frames_at`](Self::frames_at) donne la position à n'importe quel instant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamPosition {
    /// Trames jouées pendant les périodes de `RUN` déjà closes.
    accumulated: u64,
    /// Compteur de performance au dernier `run`, `None` si le flux ne tourne pas.
    run_start: Option<u64>,
}

impl StreamPosition {
    /// Flux à l'arrêt, position 0.
    pub const fn new() -> Self {
        Self {
            accumulated: 0,
            run_start: None,
        }
    }

    /// Démarre le flux à l'instant `qpc`. Sans effet si le flux tourne déjà
    /// (un second `SetState(RUN)` ne doit pas décaler la position).
    pub fn run(&mut self, qpc: u64) {
        if self.run_start.is_none() {
            self.run_start = Some(qpc);
        }
    }

    /// Met le flux en pause à l'instant `qpc` : les trames écoulées depuis le dernier
    /// [`run`](Self::run) sont ajoutées à l'accumulateur (saturé, pas de débordement).
    /// Sans effet si le flux ne tourne pas.
    pub fn pause(&mut self, clock: &VirtualClock, qpc: u64) {
        if let Some(start) = self.run_start.take() {
            self.accumulated = self
                .accumulated
                .saturating_add(clock.frames_between(start, qpc));
        }
    }

    /// Arrête le flux et remet la position à 0.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Position en trames à l'instant `qpc` : accumulateur plus, si le flux tourne,
    /// les trames écoulées depuis le dernier [`run`](Self::run).
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn frames_at(&self, clock: &VirtualClock, qpc: u64) -> u64 {
        let running = self
            .run_start
            .map_or(0, |start| clock.frames_between(start, qpc));
        self.accumulated.saturating_add(running)
    }

    /// Vrai entre [`run`](Self::run) et [`pause`](Self::pause)/[`reset`](Self::reset).
    pub const fn is_running(&self) -> bool {
        self.run_start.is_some()
    }
}

/// Position cyclique en octets d'une trame absolue dans un tampon.
///
/// `(frames × frame_bytes) mod buffer_bytes`, calculé sans débordement en réduisant
/// d'abord `frames` modulo le nombre de trames du tampon. Renvoie `None` si
/// `frame_bytes == 0`, `buffer_bytes == 0` ou si `buffer_bytes` n'est pas un
/// multiple de `frame_bytes` (le tampon alloué par `AllocateAudioBuffer` l'est
/// toujours ; un `None` ici est un bogue de l'appelant, pas un écran bleu).
///
/// IRQL : `<= DISPATCH_LEVEL`.
pub fn byte_offset(frames: u64, frame_bytes: u32, buffer_bytes: u32) -> Option<u32> {
    if frame_bytes == 0 || buffer_bytes == 0 {
        return None;
    }
    if buffer_bytes.checked_rem(frame_bytes)? != 0 {
        return None;
    }
    let buffer_frames = u64::from(buffer_bytes.checked_div(frame_bytes)?);
    let frame_in_buffer = frames.checked_rem(buffer_frames)?;
    let bytes = frame_in_buffer.checked_mul(u64::from(frame_bytes))?;
    // `frame_in_buffer < buffer_frames` donc `bytes < buffer_bytes` : tient en u32.
    u32::try_from(bytes).ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const QPC_10MHZ: u64 = 10_000_000;

    fn clock_48k() -> VirtualClock {
        VirtualClock::new(QPC_10MHZ, 48_000).unwrap()
    }

    #[test]
    fn new_rejects_zero() {
        assert!(VirtualClock::new(0, 48_000).is_none());
        assert!(VirtualClock::new(QPC_10MHZ, 0).is_none());
        let clock = clock_48k();
        assert_eq!(clock.qpc_freq(), QPC_10MHZ);
        assert_eq!(clock.sample_rate(), 48_000);
    }

    #[test]
    fn one_second_is_48000_frames() {
        let clock = clock_48k();
        assert_eq!(clock.frames_between(0, QPC_10MHZ), 48_000);
        assert_eq!(clock.frames_between(1_000, QPC_10MHZ + 1_000), 48_000);
        // 1 ms = 10 000 ticks = 48 trames.
        assert_eq!(clock.frames_between(0, 10_000), 48);
    }

    #[test]
    fn one_tick_is_zero_frames() {
        let clock = clock_48k();
        assert_eq!(clock.frames_between(0, 1), 0);
        // 208 ticks = 20,8 µs < 1 trame (20,83 µs) ; 209 ticks ≥ 1 trame.
        assert_eq!(clock.frames_between(0, 208), 0);
        assert_eq!(clock.frames_between(0, 209), 1);
    }

    #[test]
    fn now_before_start_is_zero() {
        let clock = clock_48k();
        assert_eq!(clock.frames_between(1_000, 999), 0);
        assert_eq!(clock.frames_between(u64::MAX, 0), 0);
    }

    #[test]
    fn no_overflow_at_u64_max() {
        let clock = clock_48k();
        // (2^64 − 1) × 48 000 / 10^7 ≈ 8,85 × 10^16 : tient dans un u64.
        let frames = clock.frames_between(0, u64::MAX);
        assert_eq!(frames, 88_544_371_553_805_847);
        // Fréquence 1 Hz et rate maximal : le quotient dépasse u64 → saturation.
        let extreme = VirtualClock::new(1, u32::MAX).unwrap();
        assert_eq!(extreme.frames_between(0, u64::MAX), u64::MAX);
        // 2^63 ticks à 24 MHz (fréquence QPC courante sur les machines récentes).
        let clock_24m = VirtualClock::new(24_000_000, 192_000).unwrap();
        assert_eq!(clock_24m.frames_between(0, 1 << 63), (1u64 << 63) / 125);
    }

    #[test]
    fn stream_position_run_pause_run() {
        let clock = clock_48k();
        let mut pos = StreamPosition::new();
        assert!(!pos.is_running());
        assert_eq!(pos.frames_at(&clock, 12_345), 0);

        pos.run(1_000_000);
        assert!(pos.is_running());
        // Second `run` idempotent : le départ reste à 1 000 000.
        pos.run(5_000_000);
        assert_eq!(pos.frames_at(&clock, 1_000_000 + QPC_10MHZ), 48_000);

        // Pause après 1 s : 48 000 trames accumulées, position figée.
        pos.pause(&clock, 1_000_000 + QPC_10MHZ);
        assert!(!pos.is_running());
        assert_eq!(pos.frames_at(&clock, 1_000_000 + 5 * QPC_10MHZ), 48_000);
        // Pause à répétition : sans effet.
        pos.pause(&clock, 1_000_000 + 5 * QPC_10MHZ);
        assert_eq!(pos.frames_at(&clock, u64::MAX), 48_000);

        // Reprise 10 s plus tard : la position repart de 48 000.
        let restart = 1_000_000 + 11 * QPC_10MHZ;
        pos.run(restart);
        assert_eq!(pos.frames_at(&clock, restart), 48_000);
        assert_eq!(pos.frames_at(&clock, restart + QPC_10MHZ / 2), 72_000);

        pos.reset();
        assert!(!pos.is_running());
        assert_eq!(pos.frames_at(&clock, restart + QPC_10MHZ), 0);
        assert_eq!(pos, StreamPosition::default());
    }

    #[test]
    fn stream_position_saturates() {
        let clock = VirtualClock::new(1, u32::MAX).unwrap();
        let mut pos = StreamPosition::new();
        pos.run(0);
        pos.pause(&clock, u64::MAX);
        pos.run(0);
        assert_eq!(pos.frames_at(&clock, u64::MAX), u64::MAX);
    }

    #[test]
    fn byte_offset_wraps() {
        // Stéréo F32 : 8 octets par trame, tampon de 480 trames.
        assert_eq!(byte_offset(0, 8, 3_840), Some(0));
        assert_eq!(byte_offset(1, 8, 3_840), Some(8));
        assert_eq!(byte_offset(479, 8, 3_840), Some(3_832));
        assert_eq!(byte_offset(480, 8, 3_840), Some(0));
        assert_eq!(byte_offset(481, 8, 3_840), Some(8));
        assert_eq!(
            byte_offset(u64::MAX, 8, 3_840),
            Some((u64::MAX % 480 * 8) as u32)
        );
        // Tampon d'une seule trame.
        assert_eq!(byte_offset(7, 4, 4), Some(0));
    }

    #[test]
    fn byte_offset_rejects_bad_sizes() {
        assert_eq!(byte_offset(10, 0, 3_840), None);
        assert_eq!(byte_offset(10, 8, 0), None);
        assert_eq!(byte_offset(10, 8, 3_841), None);
        assert_eq!(byte_offset(10, 7, 3_840), None);
    }

    proptest! {
        /// `qpc` croissant ⇒ trames non décroissantes.
        #[test]
        fn frames_monotonic(
            freq in 1u64..=u64::MAX,
            rate in 1u32..=u32::MAX,
            start in any::<u64>(),
            a in any::<u64>(),
            b in any::<u64>(),
        ) {
            let clock = VirtualClock::new(freq, rate).unwrap();
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(clock.frames_between(start, lo) <= clock.frames_between(start, hi));
        }

        /// Exactitude à ±1 trame près contre un calcul flottant de référence.
        #[test]
        fn frames_match_reference(
            freq in 1_000u64..=100_000_000,
            rate in 8_000u32..=384_000,
            elapsed in 0u64..=1u64 << 40,
        ) {
            let clock = VirtualClock::new(freq, rate).unwrap();
            let got = clock.frames_between(0, elapsed) as f64;
            let expected = elapsed as f64 * rate as f64 / freq as f64;
            prop_assert!((got - expected).abs() <= 1.0, "{got} vs {expected}");
        }

        /// Une pause suivie d'une reprise ne perd ni n'ajoute de trames.
        #[test]
        fn pause_resume_is_additive(
            t1 in 0u64..1u64 << 40,
            d1 in 0u64..1u64 << 30,
            gap in 0u64..1u64 << 30,
            d2 in 0u64..1u64 << 30,
        ) {
            let clock = clock_48k();
            let mut pos = StreamPosition::new();
            pos.run(t1);
            pos.pause(&clock, t1 + d1);
            pos.run(t1 + d1 + gap);
            let got = pos.frames_at(&clock, t1 + d1 + gap + d2);
            let expected = clock.frames_between(t1, t1 + d1)
                + clock.frames_between(t1 + d1 + gap, t1 + d1 + gap + d2);
            prop_assert_eq!(got, expected);
        }

        /// `byte_offset` est toujours un multiple de la trame, strictement inférieur
        /// au tampon.
        #[test]
        fn byte_offset_in_range(
            frames in any::<u64>(),
            frame_bytes in 1u32..=32,
            buffer_frames in 1u32..=100_000,
        ) {
            let buffer_bytes = frame_bytes * buffer_frames;
            let off = byte_offset(frames, frame_bytes, buffer_bytes).unwrap();
            prop_assert!(off < buffer_bytes);
            prop_assert_eq!(off % frame_bytes, 0);
            prop_assert_eq!(u64::from(off / frame_bytes), frames % u64::from(buffer_frames));
        }
    }
}
