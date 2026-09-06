//! Typographie du thème « Sericæ » : trois familles, trois voix.
//!
//! - **Fraunces** parle : display et titres ([`display`]) ;
//! - **Spectral** raconte : texte courant ([`texte`]) ;
//! - **Inter** renseigne : interface, métadonnées, chiffres ([`interface`]).
//!
//! Une sérif ne descend jamais sous [`CORPS_MINIMAL_SERIF`] en interface :
//! [`texte_a`] repasse à Inter en dessous.
//!
//! # Trois écarts de fidélité assumés
//!
//! `cosmic-text`, le moteur de texte d'`iced`, applique l'axe `wght` des
//! polices variables mais **aucun autre axe**, et n'expose **aucune
//! fonctionnalité OpenType**. Il en découle :
//!
//! 1. `opsz` reste à sa valeur par défaut du fichier : la compensation
//!    optique des grands corps est obtenue en jouant sur la graisse
//!    ([`display`]) ;
//! 2. `tnum` (chiffres tabulaires) est inaccessible : l'alignement des
//!    colonnes de chiffres s'obtient en **fixant la largeur des cellules**
//!    (`text(…).width(72).align_x(Right)`), pas par la police ;
//! 3. `SOFT` et `WONK` de Fraunces restent à 0, ce qui est exactement le
//!    dialecte « Boutique » recherché.
//!
//! `iced` n'a par ailleurs pas de `letter-spacing` : les capitales espacées
//! sont composées caractère par caractère par [`petites_capitales`].

use iced::font::Weight;
use iced::widget::{row, space, text, Row};
use iced::{Element, Font};

use crate::theme::{CAPITALES_ESPACEMENT, CORPS_MINIMAL_SERIF};

// --- Fichiers embarqués -----------------------------------------------------

/// Fraunces variable (`SOFT`, `WONK`, `opsz`, `wght`), sous OFL-1.1 (ADR-014).
pub static POLICE_FRAUNCES: &[u8] =
    include_bytes!("../assets/fonts/Fraunces[SOFT,WONK,opsz,wght].ttf");
/// Inter variable (`opsz`, `wght`), sous OFL-1.1 (ADR-014).
pub static POLICE_INTER: &[u8] = include_bytes!("../assets/fonts/Inter[opsz,wght].ttf");
/// Spectral Regular, sous OFL-1.1 (ADR-014).
pub static POLICE_SPECTRAL: &[u8] = include_bytes!("../assets/fonts/Spectral-Regular.ttf");
/// Spectral Light, sous OFL-1.1 (ADR-014). Spectral n'est pas variable : la
/// graisse 300 du mode sombre demande son propre fichier.
pub static POLICE_SPECTRAL_LIGHT: &[u8] = include_bytes!("../assets/fonts/Spectral-Light.ttf");

/// Les quatre fichiers à charger au démarrage, dans l'ordre.
pub const POLICES: [&[u8]; 4] = [
    POLICE_FRAUNCES,
    POLICE_INTER,
    POLICE_SPECTRAL,
    POLICE_SPECTRAL_LIGHT,
];

// --- Familles ---------------------------------------------------------------

/// Nom de la famille qui parle.
pub const FRAUNCES: &str = "Fraunces";
/// Nom de la famille qui renseigne.
pub const INTER: &str = "Inter";
/// Nom de la famille qui raconte.
pub const SPECTRAL: &str = "Spectral";

/// Police par défaut de l'application : Inter, qui porte toute l'interface.
pub fn defaut() -> Font {
    interface()
}

/// Police d'un display ou d'un titre : Fraunces.
///
/// L'axe `opsz` n'étant pas pilotable, la compensation optique passe par la
/// graisse : plus le corps est grand, plus le dessin est léger.
pub fn display(taille: f32) -> Font {
    let graisse = if taille >= 40.0 {
        Weight::Medium
    } else if taille >= 24.0 {
        Weight::Semibold
    } else {
        Weight::Bold
    };
    Font {
        weight: graisse,
        ..Font::with_name(FRAUNCES)
    }
}

/// Police du texte courant : Spectral, à la graisse du mode
/// (400 en clair, 300 en sombre).
pub fn texte(graisse: Weight) -> Font {
    Font {
        weight: graisse,
        ..Font::with_name(SPECTRAL)
    }
}

/// Police du texte courant à un corps donné.
///
/// En dessous de [`CORPS_MINIMAL_SERIF`], la sérif cède la place à Inter :
/// une sérif ne descend jamais sous 13 px en interface.
pub fn texte_a(graisse: Weight, taille: f32) -> Font {
    if taille < CORPS_MINIMAL_SERIF {
        interface_graisse(graisse)
    } else {
        texte(graisse)
    }
}

/// Police de l'interface, des métadonnées et des chiffres : Inter.
pub fn interface() -> Font {
    Font::with_name(INTER)
}

/// Police de l'interface à une graisse donnée.
pub fn interface_graisse(graisse: Weight) -> Font {
    Font {
        weight: graisse,
        ..Font::with_name(INTER)
    }
}

// --- Capitales espacées -----------------------------------------------------

/// Un morceau de capitales espacées.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segment {
    /// Un caractère, rendu par son propre `text`.
    Lettre(char),
    /// Une coupure entre deux mots, rendue par une espace de largeur fixe.
    Espace,
}

/// Découpe un texte en segments de capitales : une lettre par segment, les
/// suites d'espaces réduites à une seule coupure.
pub(crate) fn segments(source: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    for c in source.chars() {
        if c.is_whitespace() {
            // Ni deux coupures de suite, ni coupure en tête.
            if !matches!(segments.last(), None | Some(Segment::Espace)) {
                segments.push(Segment::Espace);
            }
        } else {
            segments.extend(c.to_uppercase().map(Segment::Lettre));
        }
    }
    // Ni coupure en queue.
    if segments.last() == Some(&Segment::Espace) {
        segments.pop();
    }
    segments
}

/// Capitales espacées de +0,08 em, composées caractère par caractère.
///
/// `iced` n'a pas de `letter-spacing` : l'espacement est obtenu par une `Row`
/// d'un `text` par caractère, dont l'espacement vaut
/// `taille × ` [`CAPITALES_ESPACEMENT`]. Les coupures entre mots ajoutent une
/// espace de `taille × 0,35` à cet espacement.
///
/// À réserver aux libellés courts — onglets, boutons, sur-titres : un
/// paragraphe entier composé ainsi ne se coupe pas en fin de ligne.
pub fn petites_capitales<'a, M: 'a>(source: &str, taille: f32) -> Element<'a, M> {
    let mut ligne: Row<'a, M> = row![].spacing(taille * CAPITALES_ESPACEMENT);
    for segment in segments(source) {
        ligne = match segment {
            Segment::Lettre(c) => ligne.push(text(c.to_string()).size(taille).font(interface())),
            Segment::Espace => ligne.push(space::horizontal().width(taille * 0.35)),
        };
    }
    ligne.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use iced::font::Family;

    /// Les quatre fichiers sont bien embarqués, distincts et non vides.
    ///
    /// Ce test garde aussi le filtre de sources Nix : s'il cessait d'inclure
    /// `crates/conduit-gui/assets/`, la compilation échouerait ici.
    #[test]
    fn les_quatre_polices_sont_embarquees() {
        assert_eq!(POLICES.len(), 4);
        for police in POLICES {
            assert!(police.len() > 10_000, "police tronquée : {}", police.len());
            // En-tête d'une police TrueType : `\0\x01\0\0`.
            assert_eq!(&police[..4], b"\x00\x01\x00\x00");
        }
        assert_ne!(POLICE_SPECTRAL, POLICE_SPECTRAL_LIGHT);
    }

    /// Chaque voix a sa famille.
    #[test]
    fn les_trois_voix_ont_leur_famille() {
        assert_eq!(display(40.0).family, Family::Name(FRAUNCES));
        assert_eq!(texte(Weight::Normal).family, Family::Name(SPECTRAL));
        assert_eq!(interface().family, Family::Name(INTER));
        assert_eq!(defaut().family, Family::Name(INTER));
    }

    /// À défaut d'`opsz`, la graisse s'allège quand le corps grandit.
    #[test]
    fn le_display_s_allege_quand_le_corps_grandit() {
        assert_eq!(display(68.0).weight, Weight::Medium);
        assert_eq!(display(40.0).weight, Weight::Medium);
        assert_eq!(display(24.0).weight, Weight::Semibold);
        assert_eq!(display(17.0).weight, Weight::Bold);
    }

    /// Le texte courant porte la graisse du mode.
    #[test]
    fn le_texte_courant_porte_la_graisse_du_mode() {
        assert_eq!(texte(Weight::Normal).weight, Weight::Normal);
        assert_eq!(texte(Weight::Light).weight, Weight::Light);
    }

    /// Une sérif ne descend pas sous 13 px en interface.
    #[test]
    fn la_serif_cede_a_inter_sous_treize_pixels() {
        assert_eq!(texte_a(Weight::Normal, 17.0).family, Family::Name(SPECTRAL));
        assert_eq!(texte_a(Weight::Normal, 13.0).family, Family::Name(SPECTRAL));
        assert_eq!(texte_a(Weight::Normal, 12.0).family, Family::Name(INTER));
        assert_eq!(texte_a(Weight::Light, 12.0).weight, Weight::Light);
    }

    /// Une lettre par élément, une coupure par blanc, rien aux extrémités.
    #[test]
    fn les_capitales_donnent_un_element_par_caractere() {
        assert_eq!(segments("Câbles").len(), 6);
        assert_eq!(segments("Câbles")[0], Segment::Lettre('C'));
        assert_eq!(segments("Câbles")[1], Segment::Lettre('Â'));
        // « Nœud » : deux mots, une coupure, huit lettres.
        let deux_mots = segments("Nœud actif");
        assert_eq!(deux_mots.len(), 10);
        assert_eq!(
            deux_mots.iter().filter(|s| **s == Segment::Espace).count(),
            1
        );
        assert_eq!(deux_mots[1], Segment::Lettre('Œ'));
        // Blancs multiples et blancs aux extrémités : une seule coupure, pas
        // de segment en tête ni en queue.
        let serre = segments("  a \t b  ");
        assert_eq!(
            serre,
            vec![Segment::Lettre('A'), Segment::Espace, Segment::Lettre('B')]
        );
        assert!(segments("   ").is_empty());
    }
}
