//! Périodes de notification d'un flux WaveRT (`docs/driver-design.md` §5.3, étape 5).
//!
//! Un flux alloué par `AllocateBufferWithNotification(count, …)` doit signaler ses
//! événements `count` fois par tour de tampon (`count` ∈ {1, 2} pour PortCls : fin de
//! tampon, ou milieu et fin). Le pilote n'a pas de DMA qui l'interrompt : une DPC
//! périodique (1 ms) lit la position calculée par l'horloge ([`crate::position`]) et
//! demande à [`Notifier`] si une frontière de période a été franchie depuis le dernier
//! signal.
//!
//! Le raisonnement se fait sur la **position absolue en trames** (monotone, jamais
//! réduite modulo le tampon) : les frontières sont les multiples de
//! `period_frames = buffer_frames / count`. Le tampon étant un multiple de la période
//! ([`crate::format::buffer_bytes_for_notifications`] l'impose), la fin du tampon est
//! elle-même une frontière : le bouclage de la position cyclique ne demande aucun cas
//! particulier, et une DPC en retard qui saute plusieurs frontières ne signale qu'une
//! fois (l'événement est un `KEVENT` : le moteur audio relit la position, pas un
//! compteur).
//!
//! Tout ici est appelable à `DISPATCH_LEVEL` : aucune allocation, aucune panique.

/// Suivi des périodes de notification d'un tampon cyclique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notifier {
    /// Trames par période de notification.
    period_frames: u64,
    /// Indice de la période (`frames / period_frames`) au dernier signal (0 au départ :
    /// la première période ne se signale qu'une fois achevée).
    last_period: u64,
}

impl Notifier {
    /// Suivi pour un tampon de `buffer_frames` trames signalé `count` fois par tour.
    ///
    /// Renvoie `None` si `count == 0`, si `buffer_frames == 0` ou si `buffer_frames`
    /// n'est pas un multiple de `count` (les frontières ne tomberaient pas sur des
    /// trames entières et la fin du tampon n'en serait pas une).
    pub const fn new(buffer_frames: u32, count: u32) -> Option<Self> {
        if count == 0 || buffer_frames == 0 {
            return None;
        }
        // `count ≠ 0` : les deux opérations sont définies.
        let (Some(rem), Some(period)) = (
            buffer_frames.checked_rem(count),
            buffer_frames.checked_div(count),
        ) else {
            return None;
        };
        if rem != 0 || period == 0 {
            return None;
        }
        Some(Self {
            period_frames: period as u64,
            last_period: 0,
        })
    }

    /// Trames par période de notification (`buffer_frames / count`).
    pub const fn period_frames(&self) -> u64 {
        self.period_frames
    }

    /// Oublie le dernier signal : à `KSSTATE_STOP`, quand la position repart de 0.
    pub fn reset(&mut self) {
        self.last_period = 0;
    }

    /// Indice de la période contenant la trame absolue `frames`.
    fn period_of(&self, frames: u64) -> u64 {
        // `period_frames ≥ 1` par construction : jamais `None`.
        frames.checked_div(self.period_frames).unwrap_or(0)
    }

    /// Fait avancer la position absolue à `frames` ; vrai si au moins une frontière de
    /// période a été franchie depuis le dernier signal (ou depuis le départ), auquel
    /// cas ce signal est mémorisé.
    ///
    /// Une position qui recule (ne doit pas arriver, l'horloge est monotone) ne signale
    /// pas mais réaligne le suivi, pour ne pas signaler deux fois la même frontière.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn advance(&mut self, frames: u64) -> bool {
        let period = self.period_of(frames);
        if period > self.last_period {
            self.last_period = period;
            true
        } else {
            self.last_period = period;
            false
        }
    }
}

/// Nombre de frontières de période (multiples de `period_frames`) strictement
/// comprises dans `]previous ; current]`, positions absolues en trames. 0 si
/// `current <= previous` ou si `period_frames == 0`.
///
/// C'est la référence dont [`Notifier::advance`] est l'approximation « au plus un
/// signal par appel » : sur des pas plus courts qu'une période, les deux coïncident.
pub fn boundaries_crossed(previous: u64, current: u64, period_frames: u64) -> u64 {
    if period_frames == 0 || current <= previous {
        return 0;
    }
    // `period_frames ≠ 0` : jamais `None`.
    let before = previous.checked_div(period_frames).unwrap_or(0);
    let after = current.checked_div(period_frames).unwrap_or(0);
    after.saturating_sub(before)
}

/// Arrondit `frames` **vers le haut** au multiple de `count` (pour que le tampon soit
/// un multiple de la période de notification). `None` si `count == 0` ou si le
/// résultat ne tient pas dans un `u32`.
pub fn align_frames(frames: u32, count: u32) -> Option<u32> {
    if count == 0 {
        return None;
    }
    let rem = frames.checked_rem(count)?;
    if rem == 0 {
        return Some(frames);
    }
    frames.checked_add(count.checked_sub(rem)?)
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
    use crate::position::byte_offset;
    use proptest::prelude::*;

    #[test]
    fn new_rejects_bad_inputs() {
        assert!(Notifier::new(0, 1).is_none());
        assert!(Notifier::new(480, 0).is_none());
        assert!(Notifier::new(481, 2).is_none());
        assert_eq!(Notifier::new(480, 1).unwrap().period_frames(), 480);
        assert_eq!(Notifier::new(480, 2).unwrap().period_frames(), 240);
        assert_eq!(Notifier::new(1, 1).unwrap().period_frames(), 1);
    }

    #[test]
    fn signals_once_per_period_boundary() {
        let mut n = Notifier::new(480, 2).unwrap();
        assert!(!n.advance(0));
        assert!(!n.advance(100));
        assert!(!n.advance(239));
        // Milieu du tampon.
        assert!(n.advance(240));
        assert!(!n.advance(240));
        assert!(!n.advance(479));
        // Fin du tampon = bouclage de la position cyclique : c'est une frontière.
        assert!(n.advance(480));
        assert!(!n.advance(500));
        // Deux frontières sautées d'un coup : un seul signal.
        assert!(n.advance(1_000));
        assert!(!n.advance(1_100));
        assert!(n.advance(1_200));
    }

    #[test]
    fn reset_forgets_last_signal() {
        let mut n = Notifier::new(480, 1).unwrap();
        assert!(n.advance(480));
        n.reset();
        assert!(!n.advance(0));
        assert!(n.advance(480));
    }

    #[test]
    fn backwards_position_realigns_without_signal() {
        let mut n = Notifier::new(480, 1).unwrap();
        assert!(n.advance(960));
        assert!(!n.advance(500));
        // Repasser la frontière 960 la signale à nouveau (elle a été « refranchie »).
        assert!(n.advance(960));
    }

    #[test]
    fn boundaries_crossed_table() {
        assert_eq!(boundaries_crossed(0, 0, 240), 0);
        assert_eq!(boundaries_crossed(0, 239, 240), 0);
        assert_eq!(boundaries_crossed(0, 240, 240), 1);
        assert_eq!(boundaries_crossed(239, 240, 240), 1);
        assert_eq!(boundaries_crossed(240, 240, 240), 0);
        assert_eq!(boundaries_crossed(0, 1_000, 240), 4);
        assert_eq!(boundaries_crossed(500, 100, 240), 0);
        assert_eq!(boundaries_crossed(0, 100, 0), 0);
        assert_eq!(boundaries_crossed(0, u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn align_frames_table() {
        assert_eq!(align_frames(480, 1), Some(480));
        assert_eq!(align_frames(480, 2), Some(480));
        assert_eq!(align_frames(481, 2), Some(482));
        assert_eq!(align_frames(0, 2), Some(0));
        assert_eq!(align_frames(7, 4), Some(8));
        assert_eq!(align_frames(480, 0), None);
        assert_eq!(align_frames(u32::MAX, 2), None);
        assert_eq!(align_frames(u32::MAX - 1, 2), Some(u32::MAX - 1));
    }

    proptest! {
        /// Sur des pas plus courts qu'une période, `advance` signale exactement le
        /// nombre de frontières franchies : au total `final / period`.
        #[test]
        fn advance_matches_boundaries_on_small_steps(
            buffer_frames in 2u32..=20_000,
            count in 1u32..=2,
            steps in prop::collection::vec(0u64..2_000, 1..200),
        ) {
            let buffer_frames = buffer_frames - buffer_frames % count;
            prop_assume!(buffer_frames > 0);
            let mut n = Notifier::new(buffer_frames, count).unwrap();
            let period = n.period_frames();
            let mut pos = 0u64;
            let mut signals = 0u64;
            let mut expected = 0u64;
            for step in steps {
                let step = step.min(period - 1);
                let next = pos + step;
                expected += boundaries_crossed(pos, next, period);
                if n.advance(next) {
                    signals += 1;
                }
                pos = next;
            }
            prop_assert_eq!(signals, expected);
            prop_assert_eq!(signals, pos / period);
        }

        /// Sur des pas quelconques, `advance` signale au plus une fois par appel, jamais
        /// plus que les frontières franchies, et au moins une fois dès qu'une frontière
        /// l'a été depuis le dernier signal.
        #[test]
        fn advance_bounded_by_boundaries(
            buffer_frames in 2u32..=20_000,
            count in 1u32..=2,
            steps in prop::collection::vec(0u64..100_000, 1..100),
        ) {
            let buffer_frames = buffer_frames - buffer_frames % count;
            prop_assume!(buffer_frames > 0);
            let mut n = Notifier::new(buffer_frames, count).unwrap();
            let period = n.period_frames();
            let mut pos = 0u64;
            for step in steps {
                let next = pos + step;
                let crossed = boundaries_crossed(pos, next, period);
                let signalled = n.advance(next);
                prop_assert_eq!(signalled, crossed > 0);
                pos = next;
            }
        }

        /// Point de vue cyclique : entre deux positions séparées de moins d'un tampon,
        /// une frontière est franchie si et seulement si l'octet de la frontière
        /// (multiple de `period_bytes`) est atteint ou dépassé, bouclage compris.
        #[test]
        fn boundaries_agree_with_ring_offsets(
            buffer_frames in 2u32..=20_000,
            count in 1u32..=2,
            frame_bytes in 1u32..=32,
            start in 0u64..1u64 << 40,
            step in 0u64..20_000,
        ) {
            let buffer_frames = buffer_frames - buffer_frames % count;
            prop_assume!(buffer_frames > 0);
            let step = step.min(u64::from(buffer_frames) - 1);
            let period = u64::from(buffer_frames / count);
            let end = start + step;
            let crossed = boundaries_crossed(start, end, period) > 0;

            // Même question posée en octets cycliques : la frontière suivante après
            // `start` est-elle dans `]start ; end]`, en tenant compte du bouclage ?
            let buffer_bytes = buffer_frames * frame_bytes;
            let period_bytes = u64::from(buffer_bytes / count);
            let a = u64::from(byte_offset(start, frame_bytes, buffer_bytes).unwrap());
            let b = u64::from(byte_offset(end, frame_bytes, buffer_bytes).unwrap());
            let next_boundary = (a / period_bytes + 1) * period_bytes % u64::from(buffer_bytes);
            let advanced = (b + u64::from(buffer_bytes) - a) % u64::from(buffer_bytes);
            let distance = (next_boundary + u64::from(buffer_bytes) - a) % u64::from(buffer_bytes);
            // `distance == 0` quand la frontière suivante est le bouclage complet (un
            // tampon entier) : jamais atteint avec `step < buffer_frames`.
            let ring_crossed = distance != 0 && advanced >= distance;
            prop_assert_eq!(crossed, ring_crossed);
        }

        /// `align_frames` : multiple de `count`, au moins `frames`, moins de `count`
        /// au-dessus.
        #[test]
        fn align_frames_in_range(frames in 0u32..1 << 30, count in 1u32..=8) {
            let aligned = align_frames(frames, count).unwrap();
            prop_assert_eq!(aligned % count, 0);
            prop_assert!(aligned >= frames);
            prop_assert!(aligned - frames < count);
        }
    }
}
