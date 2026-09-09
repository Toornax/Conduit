//! Bornes et valeurs par défaut des paramètres de registre (M1b-01,
//! `docs/driver-design.md` §2.1).
//!
//! Au démarrage, `StartDevice` lit la clé `Parameters` du périphérique
//! (`docs/driver-design.md` §4, table des modules : « lit les paramètres (M1b-01) »)
//! et en tire trois valeurs `REG_DWORD` — taille de la réserve de câbles, nombre de
//! canaux, durée du tampon. Ce module les confronte à leurs bornes et rend des
//! paramètres utilisables **quoi qu'il arrive**, plus un rapport de ce qui a été
//! corrigé.
//!
//! # Pourquoi ce module écrête là où [`crate::format`] refuse
//!
//! Les deux voisinent dans ce crate et se comportent à l'inverse l'un de l'autre,
//! délibérément :
//!
//! | | [`crate::format::buffer_bytes`] | [`sanitize`] |
//! |---|---|---|
//! | Hors borne haute | refuse (`None`) | écrête au plafond |
//! | Qui a demandé | une **application** (le moteur audio) | un **administrateur** (le registre) |
//! | Conséquence du refus | un flux ne se crée pas | le pilote ne charge plus |
//!
//! Un tampon trop grand est une demande d'application : la refuser proprement coûte
//! un flux, l'application se rabat sur un autre format, et lui rendre en douce moins
//! que demandé casserait le contrat de `AllocateAudioBuffer` (« at least the requested
//! size »). Un paramètre de registre aberrant est une erreur d'administration : la
//! refuser coûterait le **chargement du pilote**, donc toutes les cartes son virtuelles
//! du poste, pour une faute de frappe dans un `REG_DWORD`. Le critère de M1b-01 est
//! explicite (ROADMAP) : « valeurs hors bornes → valeurs par défaut et journal
//! d'événements, **jamais d'échec de chargement** ».
//!
//! [`sanitize`] est donc **totale** : elle ne rend aucune erreur, ne panique pas, et
//! rend en plus un [`Report`] assez précis pour que le pilote journalise « réserve
//! (ReserveSize) = 99 au-dessus du plafond 16, repli sur 16 » — et pas seulement
//! « paramètre invalide », qui ne sert à rien à 3 h du matin.
//!
//! # Écrêtage plutôt que valeur par défaut
//!
//! Le critère dit « valeurs par défaut » ; ce module écrête vers la borne franchie,
//! ce qui en est la lecture qui respecte l'intention de l'administrateur : qui écrit
//! 12 canaux veut le maximum de canaux, pas les 2 par défaut. Le repli sur la valeur
//! par défaut reste le comportement d'une valeur **absente** (installation neuve,
//! valeur d'un autre type) — ce n'est pas une erreur, et ce n'est donc pas journalisé
//! comme une correction. Pour la réserve, les deux lectures coïncident : le plafond
//! *est* la valeur par défaut (16).
//!
//! # État de câblage
//!
//! - **Réserve** : bornes et défaut complets, effectifs côté pilote dès M1b-01 / M1b-02.
//! - **Tampon** : effectif depuis M1b-05, comme **plancher** du tampon cyclique
//!   ([`crate::format::buffer_bytes_with_floor`]). Il était lu, validé et journalisé
//!   depuis M1b-01 sans agir sur rien.
//! - **Canaux** : **supplanté** par M1b-05, et c'est le seul paramètre dont le statut ait
//!   régressé. Il décrivait un nombre de canaux global ; le pilote en sert désormais un
//!   **par câble**, celui que `CableFormat<n>` fixe
//!   ([`crate::config::CABLE_FORMAT_VALUE_NAMES`]). Deux réglages pour une même chose
//!   seraient un réglage de trop : `Channels` est encore lu, validé et journalisé — l'INF
//!   l'écrit, `regedit` le montre —, mais **plus appliqué**. Le retirer est une question
//!   d'installateur (une valeur d'INF qui disparaît laisse une clé sur les postes déjà
//!   installés), pas de ce module ; jusque-là, mieux vaut le dire ici que le laisser
//!   croire.
//!
//! Tout ici est sans allocation ni panique ; l'appel a lieu à `PASSIVE_LEVEL`
//! (`StartDevice`), mais rien n'interdit un appel à `DISPATCH_LEVEL`.

use core::fmt;

use crate::ring::FrameLayout;

/// Nombre minimal de câbles dans la réserve.
///
/// Un pilote chargé avec zéro câble n'a plus de raison d'être ; il ne doit pas pour
/// autant échouer à charger, d'où l'écrêtage à 1.
pub const MIN_RESERVE: u32 = 1;

/// Nombre maximal de câbles dans la réserve (SPEC F-06, §5.4).
///
/// **Le plafond est la limite fonctionnelle, pas un garde-fou de ressources.** F-06
/// est catégorique — « Limite v1 : 16 câbles sur Windows (réserve fixe) » — et §5.4
/// en donne la raison : la création vraiment dynamique demanderait un pilote de bus
/// avec un PDO par câble, reportée en v2 (D-04, ADR-004). Au-delà de 16, ce n'est pas
/// « raisonnable mais gourmand », c'est **hors spécification** : M1b-02 n'enregistre
/// que 16 emplacements, et les descripteurs, les noms d'endpoint et le budget de
/// chargement (< 2 s) sont dimensionnés pour ce nombre. Relever la réserve est un
/// changement de spécification (décision ouverte §11.1), pas un réglage de registre.
///
/// Le paramètre ne peut donc que **réduire** la réserve — ce qui reste utile : un
/// poste de test ou une machine contrainte peut n'enregistrer que 2 câbles et gagner
/// 56 sous-périphériques. Et comme on écrête, un `ReserveSize = 99` donne 16 câbles
/// et une ligne de journal, jamais un pilote qui ne charge pas.
pub const MAX_RESERVE: u32 = 16;

/// Taille de réserve par défaut (SPEC F-06, §5.4, D-04) : 16 câbles.
pub const DEFAULT_RESERVE: u32 = 16;

/// Nombre minimal de canaux par câble (SPEC F-03 : « de 1 à 8 canaux »).
pub const MIN_CHANNELS: u32 = 1;

/// Nombre maximal de canaux par câble (SPEC F-03), aligné sur
/// [`FrameLayout::MAX_CHANNELS`].
///
/// Ce n'est pas un garde-fou arbitraire : au-delà, [`FrameLayout::new`] rend `None`
/// et plus aucune trame ne se décrit. Les deux constantes doivent rester égales, ce
/// que vérifie une assertion à la compilation plus bas.
pub const MAX_CHANNELS: u32 = FrameLayout::MAX_CHANNELS as u32;

/// Nombre de canaux par défaut : 2.
///
/// C'est le nombre de canaux d'un câble neuf, celui de
/// [`crate::config::CABLE_FORMAT_DEFAULT`] (48 kHz stéréo, SPEC §5.4). Ce n'est plus la
/// seule valeur que le pilote sache servir — il les sert toutes de 1 à 8 depuis M1b-05,
/// une par câble — mais c'est celle sur laquelle tout se replie : l'INF l'écrit, le pilote
/// s'y rabat, et le service la propose.
pub const DEFAULT_CHANNELS: u32 = 2;

/// Durée minimale du tampon, en millisecondes.
///
/// Volontairement identique à [`crate::format::MIN_BUFFER_MS`] : la DPC de copie a une
/// période de 1 ms et ne saurait pas suivre un tampon plus court.
pub const MIN_BUFFER_MS: u32 = crate::format::MIN_BUFFER_MS;

/// Durée maximale du tampon, en millisecondes.
///
/// Volontairement identique à [`crate::format::MAX_BUFFER_MS`], et c'est la propriété
/// qui compte : une valeur écrêtée par ce module tombe toujours dans la fenêtre où
/// [`crate::format::buffer_bytes`] accepte. Un plafond plus bas ici ferait diverger
/// deux bornes qui décrivent le même tampon ; un plafond plus haut laisserait le
/// registre proposer une taille que le chemin de création de flux refuserait ensuite.
///
/// 500 ms rejettent l'absurde et pas le raisonnable, pour la raison détaillée dans
/// [`crate::format::MAX_BUFFER_MS`] : la mémoire non paginée reste bornée (au pire
/// absolu, 16 câbles × 2 flux × 1,5 Mio = 48 Mio), et une station de travail audio qui
/// demande 200 ms de tampon — courant en mode exclusif — n'est pas absurde.
pub const MAX_BUFFER_MS: u32 = crate::format::MAX_BUFFER_MS;

/// Durée du tampon par défaut, en millisecondes : 10 ms (SPEC §5.6, §11.2).
///
/// **Ambiguïté tranchée.** SPEC §5.6 annonce, pour le chemin application → câble →
/// application, « ≤ 2 tampons pilote (~20 ms par défaut, réglable à 10 ms) », ce qui
/// se lit de deux façons : un tampon de 20 ms réglable à 10, ou deux tampons de 10 ms
/// réglables à deux fois 5. C'est la seconde. Deux sources la fixent :
/// `docs/driver-design.md` §5.3 — « Latence de traversée = période du moteur audio
/// (10 ms en mode partagé) + avance de copie (2 ms). Le tampon interne "2 × 10 ms" de
/// SPEC §5.3 est la taille par défaut demandée au moteur, pas un tampon supplémentaire
/// du pilote » — et SPEC §11.2, décision ouverte : « Tampon interne du pilote : 10 ms
/// par défaut, réglable ? ». Le paramètre décrit donc **un** tampon (une période), pas
/// la latence totale : 10 ms par défaut, les ~20 ms de §5.6 en étant le double, et le
/// « réglable à 10 ms » de §5.6 parlant du chemin complet, soit 5 ms de tampon — d'où
/// l'importance que le plancher descende bien sous 5 ms.
pub const DEFAULT_BUFFER_MS: u32 = 10;

// Cohérences que le compilateur peut vérifier : le défaut est dans ses bornes, et le
// plafond des canaux ne dépasse pas ce que `FrameLayout` sait décrire.
const _: () = assert!(MIN_RESERVE <= DEFAULT_RESERVE && DEFAULT_RESERVE <= MAX_RESERVE);
const _: () = assert!(MIN_CHANNELS <= DEFAULT_CHANNELS && DEFAULT_CHANNELS <= MAX_CHANNELS);
const _: () = assert!(MIN_BUFFER_MS <= DEFAULT_BUFFER_MS && DEFAULT_BUFFER_MS <= MAX_BUFFER_MS);
const _: () = assert!(MAX_CHANNELS == FrameLayout::MAX_CHANNELS as u32);
// Les valeurs retenues tiennent dans le `u8` de `Params` (voir `narrow`).
const _: () = assert!(MAX_RESERVE <= u8::MAX as u32);
const _: () = assert!(MAX_CHANNELS <= u8::MAX as u32);

/// Type `REG_DWORD` de `winnt.h` : entier de 32 bits, stocké dans le boutisme de la
/// machine (`REG_DWORD` et `REG_DWORD_LITTLE_ENDIAN` sont la même valeur, 4).
///
/// La constante est **recopiée** plutôt qu'importée de `wdk-sys` : ce crate est
/// portable, sans dépendance, et se teste depuis Linux et macOS. Une divergence entre
/// les deux serait silencieuse et coûteuse — toutes les valeurs seraient rejetées et le
/// pilote prendrait ses défauts sans qu'aucun test ne bronche — d'où l'assertion à la
/// compilation `conduit_kmd::registry::_REG_DWORD_IDENTIQUE_AU_WDK`, qui les compare
/// côté pilote.
pub const REG_DWORD: u32 = 4;

/// Taille, en octets, de la charge utile d'un `REG_DWORD`.
pub const REG_DWORD_BYTES: usize = 4;

/// Décode la charge utile d'une valeur de registre en `u32`, ou rend `None`.
///
/// `kind` est le champ `Type` de `KEY_VALUE_PARTIAL_INFORMATION`, `data` sa charge utile
/// tronquée à `DataLength` octets. Rend `None` — c'est-à-dire « le registre ne fournit
/// rien d'exploitable, prends la valeur par défaut » — dans les trois cas que
/// l'administrateur peut provoquer :
///
/// - **mauvais type** : un `REG_SZ` (« 16 ») là où un `REG_DWORD` est attendu, la faute
///   la plus courante dans `regedit` ; `REG_DWORD_BIG_ENDIAN` est refusé de la même
///   façon, c'est un autre type ;
/// - **taille incohérente** : un `REG_DWORD` dont `DataLength` ne vaut pas exactement
///   quatre octets, qu'il soit tronqué (valeur écrite à la main dans une ruche, lecture
///   coupée par un tampon trop court) ou trop long ;
/// - **données vides** : `DataLength` nul, ce que produit une valeur créée sans contenu.
///
/// # Pourquoi cette fonction vit ici et pas dans le pilote
///
/// C'est la partie la plus facile à casser du chemin de lecture — un décalage d'octet,
/// un boutisme, une taille acceptée trop généreusement — et la seule qui ne demande
/// aucun appel noyau. Isolée ici, elle est couverte par `cargo test -p
/// conduit-kmd-core` ; laissée dans `conduit-kmd`, elle ne serait testable qu'en machine
/// virtuelle, car `wdk-sys` lie les bibliothèques noyau jusque sous `cargo test`.
pub const fn decode_dword(kind: u32, data: &[u8]) -> Option<u32> {
    if kind != REG_DWORD || data.len() != REG_DWORD_BYTES {
        return None;
    }
    match data.first_chunk::<REG_DWORD_BYTES>() {
        // Boutisme de la machine : `REG_DWORD` == `REG_DWORD_LITTLE_ENDIAN`, et Windows
        // ne tourne que sur des architectures petit-boutistes (x64, ARM64).
        Some(octets) => Some(u32::from_le_bytes(*octets)),
        None => None,
    }
}

/// Un des trois paramètres de registre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Param {
    /// Nombre de câbles enregistrés au démarrage.
    Reserve,
    /// Nombre de canaux par câble.
    Channels,
    /// Durée du tampon, en millisecondes.
    BufferMs,
}

impl Param {
    /// Les trois paramètres, dans l'ordre où [`Report`] range leurs corrections.
    pub const ALL: [Self; 3] = [Self::Reserve, Self::Channels, Self::BufferMs];

    /// Nom de la valeur dans la clé `Parameters` du périphérique.
    ///
    /// Ce nom part dans le journal : il doit être celui que l'administrateur voit dans
    /// `regedit`, pas une traduction.
    pub const fn value_name(self) -> &'static str {
        match self {
            Self::Reserve => "ReserveSize",
            Self::Channels => "Channels",
            Self::BufferMs => "BufferMs",
        }
    }

    /// Nom lisible du paramètre, en français, pour le journal.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Reserve => "réserve",
            Self::Channels => "canaux",
            Self::BufferMs => "tampon (ms)",
        }
    }

    /// Bornes inclusives `(minimum, maximum)`.
    pub const fn bounds(self) -> (u32, u32) {
        match self {
            Self::Reserve => (MIN_RESERVE, MAX_RESERVE),
            Self::Channels => (MIN_CHANNELS, MAX_CHANNELS),
            Self::BufferMs => (MIN_BUFFER_MS, MAX_BUFFER_MS),
        }
    }

    /// Valeur retenue quand le registre ne fournit rien d'exploitable.
    pub const fn default_value(self) -> u32 {
        match self {
            Self::Reserve => DEFAULT_RESERVE,
            Self::Channels => DEFAULT_CHANNELS,
            Self::BufferMs => DEFAULT_BUFFER_MS,
        }
    }
}

impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.label(), self.value_name())
    }
}

/// Sens de la correction appliquée à une valeur hors bornes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fix {
    /// Valeur sous le plancher : remontée au minimum.
    TooLow,
    /// Valeur au-dessus du plafond : ramenée au maximum.
    TooHigh,
}

/// Ce qu'une validation a corrigé sur un paramètre : de quoi journaliser une ligne
/// utile sans avoir à relire le registre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Correction {
    /// Paramètre fautif.
    pub param: Param,
    /// Valeur trouvée dans le registre, telle quelle.
    pub found: u32,
    /// Valeur retenue à la place.
    pub applied: u32,
    /// Borne franchie.
    pub fix: Fix,
}

impl fmt::Display for Correction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (min, max) = self.param.bounds();
        let (relation, bound) = match self.fix {
            Fix::TooLow => ("sous le plancher", min),
            Fix::TooHigh => ("au-dessus du plafond", max),
        };
        write!(
            f,
            "{} = {} {} {}, repli sur {}",
            self.param, self.found, relation, bound, self.applied
        )
    }
}

/// Corrections appliquées par [`sanitize`] : de zéro à trois, une par paramètre.
///
/// Taille fixe, aucune allocation. L'ordre est celui de [`Param::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    corrections: [Option<Correction>; 3],
}

impl Report {
    /// Vrai si le registre était entièrement dans les bornes : rien à journaliser.
    pub fn is_empty(&self) -> bool {
        self.corrections.iter().all(Option::is_none)
    }

    /// Nombre de paramètres corrigés (0 à 3).
    pub fn len(&self) -> usize {
        self.corrections.iter().flatten().count()
    }

    /// Les corrections, dans l'ordre de [`Param::ALL`] — une ligne de journal chacune.
    pub fn corrections(&self) -> impl Iterator<Item = Correction> + '_ {
        self.corrections.iter().flatten().copied()
    }

    /// La correction appliquée à `param`, s'il y en a une.
    pub fn get(&self, param: Param) -> Option<Correction> {
        self.corrections().find(|c| c.param == param)
    }
}

/// Les trois valeurs telles que lues dans le registre.
///
/// `None` = valeur absente, d'un autre type que `REG_DWORD`, ou illisible : ces trois
/// cas se traitent pareil (repli sur la valeur par défaut, sans correction à
/// journaliser), et distinguer « absente » de « `REG_SZ` » n'apprendrait rien de plus
/// à l'administrateur que ce que le pilote a déjà journalisé au moment de la lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RawParams {
    /// `ReserveSize` : nombre de câbles à enregistrer.
    pub reserve: Option<u32>,
    /// `Channels` : nombre de canaux par câble.
    pub channels: Option<u32>,
    /// `BufferMs` : durée du tampon, en millisecondes.
    pub buffer_ms: Option<u32>,
}

impl RawParams {
    /// Clé `Parameters` vide : tout part sur les valeurs par défaut.
    pub const MISSING: Self = Self {
        reserve: None,
        channels: None,
        buffer_ms: None,
    };
}

/// Paramètres validés : chaque champ est dans ses bornes, par construction.
///
/// Seule [`sanitize`] en produit ; les champs sont publics en lecture mais tout
/// assemblage à la main court-circuiterait les bornes, d'où le constructeur unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Params {
    /// Nombre de câbles à enregistrer, dans `MIN_RESERVE..=MAX_RESERVE`.
    pub reserve: u8,
    /// Nombre de canaux par câble, dans `MIN_CHANNELS..=MAX_CHANNELS`.
    ///
    /// **Supplanté par `CableFormat<n>`** depuis M1b-05 : lu et validé, plus appliqué.
    /// Voir la note d'état de câblage en tête de module.
    pub channels: u8,
    /// Durée du tampon en millisecondes, dans `MIN_BUFFER_MS..=MAX_BUFFER_MS`.
    pub buffer_ms: u32,
}

impl Params {
    /// Les valeurs par défaut : 16 câbles, 2 canaux, 10 ms.
    pub const DEFAULT: Self = Self {
        reserve: narrow(DEFAULT_RESERVE),
        channels: narrow(DEFAULT_CHANNELS),
        buffer_ms: DEFAULT_BUFFER_MS,
    };
}

impl Default for Params {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Rétrécit une valeur **déjà écrêtée** à son plafond en `u8`.
///
/// Les plafonds de la réserve (16) et des canaux (8) tiennent dans un `u8`, ce qu'une
/// assertion à la compilation vérifie ; la saturation n'est donc jamais atteinte, elle
/// remplace seulement un `unwrap` interdit par les lints du crate.
const fn narrow(value: u32) -> u8 {
    if value > u8::MAX as u32 {
        u8::MAX
    } else {
        value as u8
    }
}

/// Écrête une valeur brute dans les bornes de `param`.
///
/// Rend la valeur retenue et, le cas échéant, la correction à journaliser.
const fn clamp(param: Param, found: Option<u32>) -> (u32, Option<Correction>) {
    let (min, max) = param.bounds();
    let found = match found {
        Some(value) => value,
        // Absence : repli par défaut, sans correction — ce n'est pas une faute.
        None => return (param.default_value(), None),
    };
    if found < min {
        (
            min,
            Some(Correction {
                param,
                found,
                applied: min,
                fix: Fix::TooLow,
            }),
        )
    } else if found > max {
        (
            max,
            Some(Correction {
                param,
                found,
                applied: max,
                fix: Fix::TooHigh,
            }),
        )
    } else {
        (found, None)
    }
}

/// Valide les paramètres lus dans le registre : **jamais d'échec**.
///
/// Chaque valeur hors bornes est écrêtée vers la borne franchie et signalée dans le
/// [`Report`] ; chaque valeur absente prend sa valeur par défaut, silencieusement. Le
/// pilote journalise une ligne par [`Correction`] (`Display` est prévu pour ça) puis
/// continue son `StartDevice` : c'est le critère de M1b-01, « jamais d'échec de
/// chargement ».
///
/// C'est l'inverse de [`crate::format::buffer_bytes`], qui refuse au-delà de sa borne ;
/// la raison de cette divergence est en tête de module.
///
/// Aucune allocation, aucune panique, aucune arithmétique susceptible de déborder :
/// appelable à n'importe quel IRQL.
pub const fn sanitize(raw: RawParams) -> (Params, Report) {
    let (reserve, reserve_fix) = clamp(Param::Reserve, raw.reserve);
    let (channels, channels_fix) = clamp(Param::Channels, raw.channels);
    let (buffer_ms, buffer_fix) = clamp(Param::BufferMs, raw.buffer_ms);
    (
        Params {
            reserve: narrow(reserve),
            channels: narrow(channels),
            buffer_ms,
        },
        Report {
            corrections: [reserve_fix, channels_fix, buffer_fix],
        },
    )
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
    use std::string::ToString;

    /// Une clé où seul `reserve` est renseigné.
    const fn reserve(value: Option<u32>) -> RawParams {
        RawParams {
            reserve: value,
            ..RawParams::MISSING
        }
    }

    /// Une clé où seul `channels` est renseigné.
    const fn channels(value: Option<u32>) -> RawParams {
        RawParams {
            channels: value,
            ..RawParams::MISSING
        }
    }

    /// Une clé où seul `buffer_ms` est renseigné.
    const fn buffer_ms(value: Option<u32>) -> RawParams {
        RawParams {
            buffer_ms: value,
            ..RawParams::MISSING
        }
    }

    #[test]
    fn reserve_table() {
        // Bornes : 1 à 16 (F-06, §5.4), défaut 16. Valeurs frontières des deux côtés.
        let cases: [(Option<u32>, u8, Option<Fix>); 8] = [
            // Absente : défaut, sans correction (installation neuve).
            (None, 16, None),
            // Sous le plancher.
            (Some(0), 1, Some(Fix::TooLow)),
            // Pile au plancher.
            (Some(1), 1, None),
            // Dans les bornes : un poste de test qui n'enregistre que 2 câbles.
            (Some(2), 2, None),
            (Some(15), 15, None),
            // Pile au plafond, qui est aussi le défaut.
            (Some(16), 16, None),
            // Un de trop : la limite v1 de F-06, pas un garde-fou de ressources.
            (Some(17), 16, Some(Fix::TooHigh)),
            // Saturation.
            (Some(u32::MAX), 16, Some(Fix::TooHigh)),
        ];
        for (found, expected, fix) in cases {
            let (params, report) = sanitize(reserve(found));
            assert_eq!(params.reserve, expected, "réserve {found:?}");
            assert_eq!(
                report.get(Param::Reserve).map(|c| c.fix),
                fix,
                "réserve {found:?}"
            );
            // Les autres paramètres restent au défaut et ne sont pas signalés.
            assert_eq!(params.channels, Params::DEFAULT.channels);
            assert_eq!(params.buffer_ms, Params::DEFAULT.buffer_ms);
            assert_eq!(report.len(), usize::from(fix.is_some()));
        }
    }

    #[test]
    fn channels_table() {
        // Bornes : 1 à 8 (F-03, `FrameLayout::MAX_CHANNELS`), défaut 2. La borne est
        // écrite et testée, mais rien n'en dépend avant M1b-05.
        let cases: [(Option<u32>, u8, Option<Fix>); 7] = [
            (None, 2, None),
            // Sous le plancher.
            (Some(0), 1, Some(Fix::TooLow)),
            // Pile au plancher, puis le défaut.
            (Some(1), 1, None),
            (Some(2), 2, None),
            // Pile au plafond.
            (Some(8), 8, None),
            // Un de trop : au-delà, `FrameLayout::new` ne sait plus décrire la trame.
            (Some(9), 8, Some(Fix::TooHigh)),
            (Some(u32::MAX), 8, Some(Fix::TooHigh)),
        ];
        for (found, expected, fix) in cases {
            let (params, report) = sanitize(channels(found));
            assert_eq!(params.channels, expected, "canaux {found:?}");
            assert_eq!(
                report.get(Param::Channels).map(|c| c.fix),
                fix,
                "canaux {found:?}"
            );
            // Toute valeur retenue décrit une trame valide.
            assert!(FrameLayout::new(params.channels, crate::ring::SampleFormat::F32).is_some());
        }
    }

    #[test]
    fn buffer_ms_table() {
        // Bornes : 1 à 500 ms, alignées sur `format` ; défaut 10 ms (§5.6, §11.2).
        let cases: [(Option<u32>, u32, Option<Fix>); 9] = [
            (None, 10, None),
            // Sous le plancher : la DPC a une période de 1 ms.
            (Some(0), 1, Some(Fix::TooLow)),
            // Pile au plancher.
            (Some(1), 1, None),
            // 5 ms : le « réglable à 10 ms » de §5.6 sur le chemin complet.
            (Some(5), 5, None),
            (Some(10), 10, None),
            // 200 ms en mode exclusif : raisonnable, donc accepté.
            (Some(200), 200, None),
            // Pile au plafond, puis un de trop.
            (Some(500), 500, None),
            (Some(501), 500, Some(Fix::TooHigh)),
            (Some(u32::MAX), 500, Some(Fix::TooHigh)),
        ];
        for (found, expected, fix) in cases {
            let (params, report) = sanitize(buffer_ms(found));
            assert_eq!(params.buffer_ms, expected, "tampon {found:?}");
            assert_eq!(
                report.get(Param::BufferMs).map(|c| c.fix),
                fix,
                "tampon {found:?}"
            );
        }
    }

    #[test]
    fn cle_vide_et_cle_entierement_fausse() {
        let (params, report) = sanitize(RawParams::MISSING);
        assert_eq!(params, Params::DEFAULT);
        assert!(report.is_empty());
        assert_eq!(report.len(), 0);
        assert_eq!(report.corrections().count(), 0);

        // Les trois hors bornes en même temps : trois corrections, dans l'ordre de
        // `Param::ALL`, et un pilote qui charge quand même.
        let (params, report) = sanitize(RawParams {
            reserve: Some(99),
            channels: Some(0),
            buffer_ms: Some(60_000),
        });
        assert_eq!(
            params,
            Params {
                reserve: 16,
                channels: 1,
                buffer_ms: 500
            }
        );
        assert_eq!(report.len(), 3);
        let mut signales = report.corrections();
        assert_eq!(signales.next().unwrap().param, Param::Reserve);
        assert_eq!(signales.next().unwrap().param, Param::Channels);
        assert_eq!(signales.next().unwrap().param, Param::BufferMs);
        assert!(signales.next().is_none());
    }

    #[test]
    fn le_rapport_nomme_le_parametre_fautif_et_sa_valeur() {
        // « paramètre invalide » ne sert à rien à 3 h du matin : la ligne doit dire
        // quel paramètre, quelle valeur lue, quelle borne, quelle valeur retenue.
        let (_, report) = sanitize(reserve(Some(99)));
        let ligne = report.get(Param::Reserve).unwrap().to_string();
        assert!(ligne.contains("ReserveSize"), "{ligne}");
        assert!(ligne.contains("réserve"), "{ligne}");
        assert!(ligne.contains("99"), "{ligne}");
        assert!(ligne.contains("16"), "{ligne}");
        assert!(ligne.contains("plafond"), "{ligne}");

        let (_, report) = sanitize(buffer_ms(Some(0)));
        let ligne = report.get(Param::BufferMs).unwrap().to_string();
        assert!(ligne.contains("BufferMs"), "{ligne}");
        assert!(ligne.contains("tampon"), "{ligne}");
        assert!(ligne.contains("plancher"), "{ligne}");
        assert!(ligne.contains("repli sur 1"), "{ligne}");

        // Le rapport d'un paramètre ne parle pas d'un autre.
        assert!(report.get(Param::Reserve).is_none());
        assert!(report.get(Param::Channels).is_none());
    }

    /// Autres types de `winnt.h` cités dans les cas de test.
    const REG_SZ: u32 = 1;
    const REG_BINARY: u32 = 3;
    const REG_DWORD_BIG_ENDIAN: u32 = 5;
    const REG_QWORD: u32 = 11;

    #[test]
    fn decodage_du_registre() {
        // Bon type, bonne taille : la valeur, en petit-boutiste.
        assert_eq!(decode_dword(REG_DWORD, &[16, 0, 0, 0]), Some(16));
        assert_eq!(decode_dword(REG_DWORD, &[0, 0, 0, 0]), Some(0));
        assert_eq!(
            decode_dword(REG_DWORD, &[0xFF, 0xFF, 0xFF, 0xFF]),
            Some(u32::MAX)
        );
        // Le boutisme n'est pas une supposition : 0x0000_0010, pas 0x1000_0000.
        assert_eq!(decode_dword(REG_DWORD, &[0x10, 0, 0, 0]), Some(0x10));

        // Bon type, taille tronquée ou trop longue : rien d'exploitable. Accepter trois
        // octets en complétant de zéros rendrait 16 pour un `10 00 00` amputé, une
        // valeur plausible et fausse.
        assert_eq!(decode_dword(REG_DWORD, &[16, 0, 0]), None);
        assert_eq!(decode_dword(REG_DWORD, &[16]), None);
        assert_eq!(decode_dword(REG_DWORD, &[16, 0, 0, 0, 0]), None);

        // Données vides : valeur créée sans contenu.
        assert_eq!(decode_dword(REG_DWORD, &[]), None);

        // Mauvais type, même quand les octets auraient un sens. `REG_SZ` est la faute
        // courante de `regedit` (« Valeur chaîne » au lieu de « Valeur DWORD »).
        for kind in [REG_SZ, REG_BINARY, REG_DWORD_BIG_ENDIAN, REG_QWORD, 0] {
            assert_eq!(decode_dword(kind, &[16, 0, 0, 0]), None, "type {kind}");
            assert_eq!(decode_dword(kind, &[]), None, "type {kind}");
        }
        // « 16 » en `REG_SZ` : six octets d'UTF-16 terminés par NUL, refusés.
        assert_eq!(decode_dword(REG_SZ, &[0x31, 0, 0x36, 0, 0, 0]), None);

        // Utilisable dans un contexte constant, comme le reste du module.
        const LUE: Option<u32> = decode_dword(REG_DWORD, &[2, 0, 0, 0]);
        assert_eq!(LUE, Some(2));
    }

    #[test]
    fn le_decodage_alimente_la_validation() {
        // Le chemin complet tel que le pilote l'enchaîne : octets du registre →
        // `decode_dword` → `sanitize`. Une valeur d'un type inattendu ne se distingue
        // plus d'une valeur absente, et donne le défaut sans correction signalée.
        let raw = RawParams {
            reserve: decode_dword(REG_DWORD, &[2, 0, 0, 0]),
            channels: decode_dword(REG_SZ, &[0x32, 0, 0, 0]),
            buffer_ms: decode_dword(REG_DWORD, &[0xF4, 0x01, 0, 0]),
        };
        let (params, report) = sanitize(raw);
        assert_eq!(params.reserve, 2);
        assert_eq!(params.channels, DEFAULT_CHANNELS as u8);
        assert_eq!(params.buffer_ms, 500);
        assert!(report.is_empty());
    }

    #[test]
    fn bornes_coherentes_avec_le_voisin() {
        // Les bornes du tampon sont celles de `format`, pas une deuxième opinion.
        assert_eq!(MIN_BUFFER_MS, crate::format::MIN_BUFFER_MS);
        assert_eq!(MAX_BUFFER_MS, crate::format::MAX_BUFFER_MS);
        assert_eq!(MAX_CHANNELS, u32::from(FrameLayout::MAX_CHANNELS));
        for param in Param::ALL {
            let (min, max) = param.bounds();
            assert!(min <= param.default_value() && param.default_value() <= max);
            assert!(!param.value_name().is_empty());
            assert!(!param.label().is_empty());
        }
        // `sanitize` est utilisable dans un contexte constant.
        const CONFIG: (Params, Report) = sanitize(RawParams::MISSING);
        assert_eq!(CONFIG.0, Params::default());
    }

    proptest! {
        /// Le décodage rend exactement ce que le registre contenait, et rien d'autre :
        /// aller-retour sur tout le domaine, et refus de toute charge utile qui n'est
        /// pas un `REG_DWORD` de quatre octets — quelle que soit sa longueur, sans
        /// panique ni indexation hors bornes.
        #[test]
        fn decodage_aller_retour(
            valeur in any::<u32>(),
            kind in any::<u32>(),
            octets in proptest::collection::vec(any::<u8>(), 0..24),
        ) {
            prop_assert_eq!(decode_dword(REG_DWORD, &valeur.to_le_bytes()), Some(valeur));

            let attendu = kind == REG_DWORD && octets.len() == REG_DWORD_BYTES;
            prop_assert_eq!(decode_dword(kind, &octets).is_some(), attendu);
        }

        /// Critère de M1b-01 : quelle que soit la clé `Parameters` — n'importe quelle
        /// valeur sur tout le domaine de chaque champ, présente ou absente — la sortie
        /// est dans les bornes et rien ne panique.
        #[test]
        fn toujours_dans_les_bornes(
            reserve in proptest::option::of(any::<u32>()),
            channels in proptest::option::of(any::<u32>()),
            buffer_ms in proptest::option::of(any::<u32>()),
        ) {
            let (params, _) = sanitize(RawParams { reserve, channels, buffer_ms });
            prop_assert!((MIN_RESERVE..=MAX_RESERVE).contains(&u32::from(params.reserve)));
            prop_assert!((MIN_CHANNELS..=MAX_CHANNELS).contains(&u32::from(params.channels)));
            prop_assert!((MIN_BUFFER_MS..=MAX_BUFFER_MS).contains(&params.buffer_ms));
        }

        /// Le rapport dit la vérité : une correction exactement quand la valeur lue
        /// était hors bornes, et elle porte la valeur lue et la valeur retenue.
        #[test]
        fn le_rapport_correspond_a_ce_qui_a_ete_applique(
            reserve in proptest::option::of(any::<u32>()),
            channels in proptest::option::of(any::<u32>()),
            buffer_ms in proptest::option::of(any::<u32>()),
        ) {
            let raw = RawParams { reserve, channels, buffer_ms };
            let (params, report) = sanitize(raw);
            let applied = [
                (Param::Reserve, reserve, u32::from(params.reserve)),
                (Param::Channels, channels, u32::from(params.channels)),
                (Param::BufferMs, buffer_ms, params.buffer_ms),
            ];
            for (param, found, applied) in applied {
                let (min, max) = param.bounds();
                match (found, report.get(param)) {
                    (Some(found), Some(correction)) => {
                        prop_assert!(found < min || found > max, "correction dans les bornes");
                        prop_assert_eq!(correction.found, found);
                        prop_assert_eq!(correction.applied, applied);
                        prop_assert_eq!(
                            correction.fix,
                            if found < min { Fix::TooLow } else { Fix::TooHigh }
                        );
                    }
                    (Some(found), None) => {
                        prop_assert!((min..=max).contains(&found), "hors bornes non signalé");
                        prop_assert_eq!(applied, found);
                    }
                    (None, correction) => {
                        prop_assert!(correction.is_none(), "absence signalée comme faute");
                        prop_assert_eq!(applied, param.default_value());
                    }
                }
            }
            // `is_empty` et `len` racontent la même chose que l'itérateur.
            prop_assert_eq!(report.is_empty(), report.corrections().count() == 0);
            prop_assert_eq!(report.len(), report.corrections().count());
        }
    }
}
