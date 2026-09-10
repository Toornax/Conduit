//! Les **contraintes de taille de paquet** que le pilote déclare au moteur audio
//! (`KSAUDIO_PACKETSIZE_CONSTRAINTS2`, `DEVPKEY_KsAudio_PacketSize_Constraints2`).
//!
//! # Le fait mesuré qui rend ce module nécessaire
//!
//! `IAudioClient3::GetSharedModeEnginePeriod` annonce, sur nos endpoints : défaut 480
//! trames, fondamentale 480, **minimum 480, maximum 480**. Autrement dit 10 ms, et rien
//! d'autre — aucune application ne peut demander une période plus courte, quoi qu'elle
//! fasse. Ce n'est pas une limite du moteur : « Low Latency Audio » range la déclaration
//! de `DEVPKEY_KsAudio_PacketSize_Constraints2` parmi les **obligations** du pilote, et
//! nous ne la déclarions nulle part. Sans elle, Windows retombe sur son défaut historique
//! de 10 ms.
//!
//! Ce module ne pose rien — il ne connaît ni le noyau ni PnP : il **calcule les trois
//! valeurs** que `conduit_kmd::adapter` posera, à partir de ce que le pilote sait
//! réellement faire. C'est la partie qui se teste sans machine.
//!
//! # Les trois valeurs, et d'où elles viennent
//!
//! | Champ | Valeur | Ce qui la fixe |
//! |---|---|---|
//! | `MinPacketPeriodInHns` | [`min_packet_period_hns`] | le plancher `BufferMs` du registre, et le minuteur de 1 ms |
//! | `PacketSizeFileAlignment` | [`PACKET_SIZE_FILE_ALIGNMENT`] | rien : le pilote n'impose aucun alignement en octets |
//! | `MaxPacketSizeInBytes` | [`MAX_PACKET_SIZE_BYTES`] | 10 ms du plus gros format qu'une broche puisse servir |
//!
//! # Le lien entre une période et un tampon, et l'hypothèse qu'il porte
//!
//! Un paquet WaveRT n'est pas le tampon : « Several WaveRT packets (typically 2) are
//! concatenated to form the WaveRT buffer » (documentation de
//! `KSAUDIO_PACKETSIZE_CONSTRAINTS2`). C'est aussi ce que la mesure montre — un client
//! WASAPI exclusif événementiel obtient `AllocateBufferWithNotification` avec
//! **2 × 240 trames**. D'où [`NOTIFICATION_COUNT`] : la période minimale annoncée est la
//! **moitié** du plancher de tampon.
//!
//! Cette moitié est une **hypothèse**, et elle est écrite ici pour qu'on sache où
//! regarder si elle tombe. `AllocateBufferWithNotification` accepte aussi
//! `NotificationCount = 1` ; dans ce cas le tampon vaut une période, et une période de
//! `BufferMs / 2` produirait un tampon sous le plancher — que
//! [`crate::format::buffer_bytes_with_floor`] remonterait alors à `BufferMs`, rendant au
//! moteur audio un tampon **plus grand** que celui qu'il a demandé. Le pilote sait déjà
//! le faire (c'est le comportement d'aujourd'hui) et le client WaveRT lit la taille
//! réellement allouée, mais la période annoncée serait, pour ce client-là, optimiste d'un
//! facteur deux. Aucun `NotificationCount = 1` n'a été observé à ce jour.

use crate::format::{MAX_BUFFER_MS, MIN_BUFFER_MS, SAMPLE_DEPTHS, SAMPLE_RATES};
use crate::ring::FrameLayout;

/// Unités de cent nanosecondes dans une milliseconde.
pub const HNS_PER_MS: u32 = 10_000;

/// Nombre de paquets concaténés qu'on suppose dans un tampon WaveRT.
///
/// Deux : c'est le « typically 2 » de la documentation, et c'est ce que la mesure a rendu
/// (`AllocateBufferWithNotification`, 2 × 240 trames, client WASAPI exclusif
/// événementiel). Voir l'avertissement en tête de module sur le cas `1`.
pub const NOTIFICATION_COUNT: u32 = 2;

/// Plancher **absolu** de la période annoncée : 2 ms.
///
/// Le minuteur de la boucle locale bat à 1 ms (`conduit_kmd::timer`) : une période de 1 ms
/// se retrouverait à la merci d'un seul tick en retard, et un tick perdu sur deux périodes
/// s'entend. Deux millisecondes laissent un tick de marge, et c'est aussi ce que déclare
/// l'exemple `SysvadWaveRtPacketSizeConstraintsRender` de la documentation Microsoft.
///
/// Ce plancher ne s'applique que **vers le bas** : si `BufferMs` impose davantage, c'est
/// `BufferMs` qui gagne ([`min_packet_period_hns`]).
pub const MIN_PACKET_PERIOD_FLOOR_HNS: u32 = 2u32.saturating_mul(HNS_PER_MS);

/// `PacketSizeFileAlignment` : **`FILE_BYTE_ALIGNMENT`**, c'est-à-dire aucune contrainte.
///
/// # Pourquoi zéro, et pas une taille de page
///
/// Le champ est un **masque** d'alignement (`wdm.h` : `FILE_BYTE_ALIGNMENT` = 0,
/// `FILE_WORD_ALIGNMENT` = 1, … `FILE_512_BYTE_ALIGNMENT` = 0x1ff), pas une taille : la
/// taille d'un paquet doit être un multiple de `masque + 1`. La documentation de
/// `KSAUDIO_PACKETSIZE_CONSTRAINTS2` énumère les dix valeurs admises, et la plus grande
/// vaut 512 octets ; 4096 n'en fait pas partie.
///
/// Surtout, un alignement de 4096 octets **interdirait** ce que ce module cherche à
/// obtenir. À 48 kHz sur deux canaux en flottant (8 octets par trame), le plus petit
/// paquet multiple de 4096 octets ferait 512 trames, soit 10,67 ms : plus long que la
/// période de 10 ms qu'on essaie précisément de faire descendre. Déclarer une contrainte
/// que le pilote n'a pas coûterait donc exactement la latence qu'on vient chercher.
///
/// Le pilote, lui, n'exige rien de tel : sa copie cyclique travaille en **trames**
/// (`conduit_kmd_core::ring`), et le moteur audio calcule déjà ses paquets en trames. Zéro
/// est donc la valeur vraie, et c'est celle que l'exemple Microsoft déclare lui aussi
/// (`FILE_BYTE_ALIGNMENT`, « 1 byte packet size alignment »).
pub const PACKET_SIZE_FILE_ALIGNMENT: u32 = 0;

/// Durée, en millisecondes, du paquet le plus long que le pilote annonce pouvoir servir.
///
/// Dix, parce que la documentation l'exige : « This size should at least be large enough
/// to support 10 ms buffer of any format supported by the pin ».
pub const MAX_PACKET_MS: u32 = 10;

/// La plus haute fréquence d'échantillonnage qu'une broche puisse servir, en hertz.
///
/// Lue dans [`SAMPLE_RATES`] plutôt que recopiée : une quatrième fréquence ajoutée un jour
/// agrandirait [`MAX_PACKET_SIZE_BYTES`] toute seule, au lieu de laisser une constante
/// mentir.
const fn max_sample_rate() -> u32 {
    // Motif de tranche plutôt qu'indexation : le crate interdit `a[i]`.
    let mut restantes: &[u32] = &SAMPLE_RATES;
    let mut max = 0;
    while let [premiere, reste @ ..] = restantes {
        if *premiere > max {
            max = *premiere;
        }
        restantes = reste;
    }
    max
}

/// Le plus gros échantillon qu'une broche puisse servir, en octets.
///
/// Lu dans [`SAMPLE_DEPTHS`], pour la raison de [`max_sample_rate`].
const fn max_sample_bytes() -> u32 {
    let mut restantes: &[crate::ring::SampleFormat] = &SAMPLE_DEPTHS;
    let mut max = 0;
    while let [premiere, reste @ ..] = restantes {
        let octets = premiere.bytes_per_sample();
        if octets > max {
            max = octets;
        }
        restantes = reste;
    }
    max
}

/// La plus grosse trame qu'une broche puisse servir, en octets : le maximum de canaux fois
/// le plus gros échantillon (8 × 4 = 32 octets).
pub const MAX_FRAME_BYTES: u32 =
    (FrameLayout::MAX_CHANNELS as u32).saturating_mul(max_sample_bytes());

/// `MaxPacketSizeInBytes` : [`MAX_PACKET_MS`] du plus gros format qu'une broche puisse
/// servir, arrondi à [`PACKET_SIZE_FILE_ALIGNMENT`].
///
/// 96 000 Hz × 8 canaux × 4 octets × 10 ms = **30 720 octets**. La valeur est un
/// **maximum**, pas une promesse d'allocation : le pilote sait allouer jusqu'à
/// [`MAX_BUFFER_MS`] (500 ms) et n'aura donc jamais à refuser un paquet qu'il a annoncé.
///
/// Elle ne dépend **pas** du format réglé sur le câble. La contrainte se pose sur
/// l'interface du filtre, une fois, et la documentation parle de « any format supported by
/// the pin » : la borne doit couvrir la plus large des variantes, pas celle du jour.
pub const MAX_PACKET_SIZE_BYTES: u32 = arrondi_alignement(
    max_sample_rate()
        .saturating_mul(MAX_FRAME_BYTES)
        .saturating_mul(MAX_PACKET_MS)
        .saturating_div(1_000),
    PACKET_SIZE_FILE_ALIGNMENT,
);

/// Arrondit `octets` **vers le haut** au multiple de `masque + 1`.
///
/// `masque` est un masque d'alignement au sens de `wdm.h` (`FILE_*_ALIGNMENT`), donc
/// toujours de la forme `2^k − 1` ; zéro n'arrondit rien. La saturation de l'addition ne
/// peut arrondir vers le bas que sur une valeur déjà proche de `u32::MAX`, que ce module
/// ne produit jamais (le plus gros paquet fait quelques dizaines de kilo-octets).
const fn arrondi_alignement(octets: u32, masque: u32) -> u32 {
    octets.saturating_add(masque) & !masque
}

/// `MinPacketPeriodInHns` pour un plancher de tampon de `buffer_ms` millisecondes.
///
/// Deux bornes, et la plus grande gagne :
///
/// - **le minuteur** : jamais moins de [`MIN_PACKET_PERIOD_FLOOR_HNS`] (2 ms) ;
/// - **le plancher de tampon** : `conduit_kmd::stream` remonte tout tampon à `BufferMs`
///   (`buffer_bytes_with_floor`), et un tampon vaut [`NOTIFICATION_COUNT`] paquets ;
///   annoncer une période dont le double serait sous le plancher ferait promettre au
///   moteur audio une taille que le pilote corrigerait ensuite dans son dos.
///
/// `buffer_ms` est écrêté dans `MIN_BUFFER_MS..=MAX_BUFFER_MS`, comme le fait
/// [`crate::params::sanitize`] : la valeur vient du registre et un appelant distrait ne
/// doit pas pouvoir faire annoncer une période aberrante.
///
/// Avec le défaut `BufferMs = 10` : `max(20 000, 10 × 10 000 / 2)` = **50 000 hns**, soit
/// 5 ms — la moitié des 10 ms d'aujourd'hui.
#[must_use]
pub const fn min_packet_period_hns(buffer_ms: u32) -> u32 {
    let buffer_ms = if buffer_ms < MIN_BUFFER_MS {
        MIN_BUFFER_MS
    } else if buffer_ms > MAX_BUFFER_MS {
        MAX_BUFFER_MS
    } else {
        buffer_ms
    };
    let demi_plancher = buffer_ms
        .saturating_mul(HNS_PER_MS)
        .saturating_div(NOTIFICATION_COUNT);
    if demi_plancher > MIN_PACKET_PERIOD_FLOOR_HNS {
        demi_plancher
    } else {
        MIN_PACKET_PERIOD_FLOOR_HNS
    }
}

/// Les trois valeurs de `KSAUDIO_PACKETSIZE_CONSTRAINTS2`, dans l'ordre de la structure du
/// WDK.
///
/// Une structure **portable** : elle ne mentionne aucun type du WDK, ce qui la rend
/// calculable et testable en mode utilisateur. `portcls::adapter` la recopie dans la vraie
/// `KSAUDIO_PACKETSIZE_CONSTRAINTS2`, dont la disposition est vérifiée contre `cl.exe`
/// (`layout.golden`).
///
/// Le quatrième champ du WDK, `NumProcessingModeConstraints`, n'est pas ici : il vaut
/// **zéro** et la documentation l'autorise (« This value can be 0 »). Une contrainte par
/// mode de traitement est une variable de plus, qu'on ne bougera qu'après avoir mesuré
/// l'effet de celle-ci.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketConstraints {
    /// `MinPacketPeriodInHns` : la période la plus courte que le pilote accepte de servir.
    pub min_packet_period_hns: u32,
    /// `PacketSizeFileAlignment` : le masque d'alignement en octets d'un paquet.
    pub packet_size_file_alignment: u32,
    /// `MaxPacketSizeInBytes` : le plus gros paquet que le pilote accepte de servir.
    pub max_packet_size_bytes: u32,
}

impl PacketConstraints {
    /// Les contraintes du pilote pour un plancher de tampon de `buffer_ms` millisecondes.
    #[must_use]
    pub const fn new(buffer_ms: u32) -> Self {
        Self {
            min_packet_period_hns: min_packet_period_hns(buffer_ms),
            packet_size_file_alignment: PACKET_SIZE_FILE_ALIGNMENT,
            max_packet_size_bytes: MAX_PACKET_SIZE_BYTES,
        }
    }

    /// La période minimale annoncée, en **trames** à `sample_rate` hertz.
    ///
    /// Ce que `IAudioClient3::GetSharedModeEnginePeriod` finira par rendre en
    /// `minPeriodInFrames`, et donc la seule forme de cette valeur qui se compare aux 480
    /// trames mesurées. Arrondie **vers le haut** : une période un peu plus longue reste
    /// servable, une période un peu plus courte ne l'est pas.
    ///
    /// `None` si `sample_rate` est nul (aucune trame ne dure alors quoi que ce soit).
    #[must_use]
    pub const fn min_period_frames(&self, sample_rate: u32) -> Option<u32> {
        if sample_rate == 0 {
            return None;
        }
        // trames = ceil(période_hns × fréquence / 10 000 000), en 64 bits : à 96 kHz et
        // 500 ms, le produit dépasse largement 32 bits.
        let hns = self.min_packet_period_hns as u64;
        let par_seconde = HNS_PER_MS.saturating_mul(1_000) as u64;
        let numerateur = match hns.checked_mul(sample_rate as u64) {
            Some(n) => n,
            None => return None,
        };
        let arrondi = numerateur.saturating_add(par_seconde.saturating_sub(1));
        // `checked_div` plutôt que `saturating_div` : le diviseur est une constante non
        // nulle, mais le lint anti-panique du crate ne le sait pas, et une division dont le
        // diviseur ne se lit pas sur place mérite d'être écrite faillible.
        let Some(trames) = arrondi.checked_div(par_seconde) else {
            return None;
        };
        if trames > u32::MAX as u64 {
            None
        } else {
            Some(trames as u32)
        }
    }
}

// Les valeurs constantes du module sont ce que le pilote déclarera : une assertion à la
// compilation vaut mieux qu'un test qu'on oublierait de lancer.
const _: () = assert!(MAX_FRAME_BYTES == 32, "8 canaux × 4 octets");
const _: () = assert!(
    MAX_PACKET_SIZE_BYTES == 30_720,
    "96 kHz × 32 octets × 10 ms"
);
const _: () = assert!(MIN_PACKET_PERIOD_FLOOR_HNS == 20_000);
// Le maximum annoncé couvre bien dix millisecondes du plus gros format, sans quoi la
// contrainte serait plus étroite que ce que la documentation exige.
const _: () = assert!(MAX_PACKET_SIZE_BYTES >= 96_000u32.saturating_mul(32).saturating_div(100));

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

    #[test]
    fn le_defaut_annonce_cinq_millisecondes() {
        let contraintes = PacketConstraints::new(10);
        assert_eq!(contraintes.min_packet_period_hns, 50_000);
        assert_eq!(contraintes.packet_size_file_alignment, 0);
        assert_eq!(contraintes.max_packet_size_bytes, 30_720);
    }

    #[test]
    fn le_plancher_de_deux_millisecondes_tient_sous_le_plus_petit_tampon() {
        // 1 ms de tampon voudrait 0,5 ms de période : le minuteur de 1 ms ne suit pas.
        assert_eq!(min_packet_period_hns(1), MIN_PACKET_PERIOD_FLOOR_HNS);
        assert_eq!(min_packet_period_hns(2), MIN_PACKET_PERIOD_FLOOR_HNS);
        assert_eq!(min_packet_period_hns(3), MIN_PACKET_PERIOD_FLOOR_HNS);
        // 4 ms de tampon : 2 ms de période, soit exactement le plancher.
        assert_eq!(min_packet_period_hns(4), MIN_PACKET_PERIOD_FLOOR_HNS);
        // 5 ms : 2,5 ms, le plancher de tampon prend le dessus.
        assert_eq!(min_packet_period_hns(5), 25_000);
    }

    #[test]
    fn la_periode_annoncee_est_la_moitie_du_plancher_de_tampon() {
        for buffer_ms in [6, 10, 20, 50, 100, 500] {
            assert_eq!(
                min_packet_period_hns(buffer_ms),
                buffer_ms * HNS_PER_MS / NOTIFICATION_COUNT,
                "tampon {buffer_ms} ms"
            );
        }
    }

    #[test]
    fn la_duree_du_registre_est_ecretee_comme_dans_params() {
        assert_eq!(
            min_packet_period_hns(0),
            min_packet_period_hns(MIN_BUFFER_MS)
        );
        assert_eq!(
            min_packet_period_hns(u32::MAX),
            min_packet_period_hns(MAX_BUFFER_MS)
        );
    }

    #[test]
    fn la_periode_ne_recule_jamais_quand_le_plancher_monte() {
        let mut precedente = 0;
        for buffer_ms in MIN_BUFFER_MS..=MAX_BUFFER_MS {
            let periode = min_packet_period_hns(buffer_ms);
            assert!(periode >= precedente, "tampon {buffer_ms} ms");
            precedente = periode;
        }
    }

    #[test]
    fn le_paquet_maximal_couvre_dix_millisecondes_de_chaque_variante() {
        for rate in SAMPLE_RATES {
            for depth in SAMPLE_DEPTHS {
                let trame = u32::from(FrameLayout::MAX_CHANNELS) * depth.bytes_per_sample();
                let dix_ms = rate * trame / 100;
                assert!(
                    MAX_PACKET_SIZE_BYTES >= dix_ms,
                    "{rate} Hz, {depth:?} : {dix_ms} octets"
                );
            }
        }
    }

    #[test]
    fn la_periode_se_lit_en_trames_comme_le_moteur_audio_la_rendra() {
        let contraintes = PacketConstraints::new(10);
        // 5 ms à 48 kHz : 240 trames, exactement la moitié des 480 mesurées.
        assert_eq!(contraintes.min_period_frames(48_000), Some(240));
        assert_eq!(contraintes.min_period_frames(96_000), Some(480));
        // 44,1 kHz : 220,5 trames, arrondies vers le haut.
        assert_eq!(contraintes.min_period_frames(44_100), Some(221));
        assert_eq!(contraintes.min_period_frames(0), None);
    }

    #[test]
    fn arrondir_a_un_masque_nul_ne_change_rien() {
        for octets in [0, 1, 7, 4095, 30_720] {
            assert_eq!(arrondi_alignement(octets, 0), octets);
        }
        // Un masque de 512 octets arrondit bien vers le haut.
        assert_eq!(arrondi_alignement(1, 0x1ff), 512);
        assert_eq!(arrondi_alignement(512, 0x1ff), 512);
        assert_eq!(arrondi_alignement(513, 0x1ff), 1024);
    }
}
