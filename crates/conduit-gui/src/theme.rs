//! Thème « Sericæ » : jetons de couleur, échelles et thèmes `iced` (M2-11a).
//!
//! Le vocabulaire est en deux couches :
//!
//! - les **matières**, nommées par la teinte ([`ENCRE`], [`GREGE`], [`OR`],
//!   [`GARANCE`], [`CELADON`], [`SEPIA`]) : elles ne changent jamais ;
//! - les **jetons** ([`Jetons`]), qui donnent un rôle à ces matières selon le
//!   mode clair ([`CLAIR`]) ou sombre ([`SOMBRE`]).
//!
//! `iced::Theme` ne transporte que six couleurs (`Palette`), bien trop peu pour
//! ce vocabulaire : les deux thèmes construits ici ([`clair`], [`sombre`]) ne
//! servent qu'à la plomberie d'`iced`, et les jetons sont retrouvés dans les
//! fonctions de style par [`jetons`], qui lit `extended_palette().is_dark`.
//!
//! Règles que ce module encode :
//!
//! - aucune ombre, aucun dégradé : les bordures sont des filets de 1 px
//!   ([`FILET`]) ;
//! - l'or ne fait jamais du texte sur une surface claire — il ne passe que
//!   2,5:1 sur le grège (voir [`contraste`]) ; il reste aux filets, pastilles,
//!   courbes et barres ;
//! - une seule couleur d'accent visible par surface.

use std::sync::LazyLock;

use iced::font::Weight;
use iced::theme::Palette;
use iced::{Color, Theme};

use crate::i18n::{self, Text};

// --- Matières ---------------------------------------------------------------

/// Encre : le noir chaud des fonds sombres et du texte de titre en clair.
pub const ENCRE: Color = Color::from_rgb8(0x14, 0x12, 0x0F);
/// Encre-2 : l'encre relevée d'un cran, pour les surfaces posées en sombre.
pub const ENCRE_2: Color = Color::from_rgb8(0x1E, 0x1B, 0x16);
/// Grège : le papier, fond du mode clair et texte du mode sombre.
pub const GREGE: Color = Color::from_rgb8(0xED, 0xE6, 0xD6);
/// Grège-2 : le grège assombri d'un cran, pour les surfaces posées en clair.
pub const GREGE_2: Color = Color::from_rgb8(0xE4, 0xDB, 0xC7);
/// Or : le fil conducteur, invariant entre les deux modes. **Jamais du texte
/// sur une surface claire** (2,5:1 sur le grège).
pub const OR: Color = Color::from_rgb8(0xB0, 0x8D, 0x3E);
/// Garance : les liens, le focus et l'alerte. Invariante entre les deux modes.
pub const GARANCE: Color = Color::from_rgb8(0x7A, 0x2E, 0x2B);
/// Céladon : l'accent tranquille des informations.
pub const CELADON: Color = Color::from_rgb8(0x3E, 0x4A, 0x41);
/// Sépia : le texte courant du mode clair.
pub const SEPIA: Color = Color::from_rgb8(0x3A, 0x34, 0x2C);
/// Voile d'or : l'or à 35 %, réservé aux aplats et aux rails.
pub const VOILE_D_OR: Color = Color::from_rgba8(0xB0, 0x8D, 0x3E, 0.35);

/// Texte secondaire du mode clair.
const TEXTE_2_CLAIR: Color = Color::from_rgb8(0x6B, 0x63, 0x53);
/// Texte secondaire du mode sombre.
const TEXTE_2_SOMBRE: Color = Color::from_rgb8(0x9A, 0x8F, 0x76);
/// Contour du mode clair : sépia à 25 %.
const CONTOUR_CLAIR: Color = Color::from_rgba8(0x3A, 0x34, 0x2C, 0.25);
/// Contour du mode sombre : or à 25 %.
const CONTOUR_SOMBRE: Color = Color::from_rgba8(0xB0, 0x8D, 0x3E, 0.25);

// --- Jetons -----------------------------------------------------------------

/// Les matières distribuées en rôles, pour un mode donné.
///
/// Les deux jeux existants sont [`CLAIR`] et [`SOMBRE`] ; [`jetons`] choisit
/// entre les deux à partir d'un `iced::Theme`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jetons {
    /// Vrai pour le mode sombre.
    pub sombre: bool,
    /// Fond de la fenêtre.
    pub surface: Color,
    /// Fond des surfaces posées : cartes, notices, listes déroulantes.
    pub surface_2: Color,
    /// Couleur des titres (Fraunces).
    pub titre: Color,
    /// Couleur du texte courant.
    pub texte: Color,
    /// Couleur des métadonnées. **Réservée à [`surface`](Self::surface)** :
    /// sur [`surface_2`](Self::surface_2) elle tombe à 4,3:1 en mode clair.
    pub texte_2: Color,
    /// Couleur des filets et des bordures.
    pub contour: Color,
    /// Le fil d'or, invariant entre les deux modes.
    pub or: Color,
    /// La garance des liens et du focus, invariante entre les deux modes.
    pub garance: Color,
    /// L'accent des informations.
    pub celadon: Color,
    /// L'or à 35 %, pour les aplats et les rails.
    pub voile_d_or: Color,
    /// Couleur d'accent lisible en texte sur [`surface`](Self::surface) :
    /// garance en clair, or en sombre — la garance ne passe que 2,0:1 sur
    /// l'encre, l'or 6,0:1.
    pub accent_texte: Color,
    /// Graisse du texte courant : 400 en clair, 300 en sombre (le blanc sur
    /// fond sombre grossit optiquement).
    pub graisse_texte: Weight,
}

/// Mode clair : le papier grège, l'encre en titre, le sépia en texte.
pub const CLAIR: Jetons = Jetons {
    sombre: false,
    surface: GREGE,
    surface_2: GREGE_2,
    titre: ENCRE,
    texte: SEPIA,
    texte_2: TEXTE_2_CLAIR,
    contour: CONTOUR_CLAIR,
    or: OR,
    garance: GARANCE,
    celadon: CELADON,
    voile_d_or: VOILE_D_OR,
    accent_texte: GARANCE,
    graisse_texte: Weight::Normal,
};

/// Mode sombre : l'encre en fond, le grège en titre et en texte.
pub const SOMBRE: Jetons = Jetons {
    sombre: true,
    surface: ENCRE,
    surface_2: ENCRE_2,
    titre: GREGE,
    texte: GREGE,
    texte_2: TEXTE_2_SOMBRE,
    contour: CONTOUR_SOMBRE,
    or: OR,
    garance: GARANCE,
    celadon: CELADON,
    voile_d_or: VOILE_D_OR,
    accent_texte: OR,
    graisse_texte: Weight::Light,
};

/// Les jetons du thème donné.
///
/// Les closures de style d'`iced` ne reçoivent qu'un `&Theme` : le seul
/// discriminant fiable est `extended_palette().is_dark`, calculé depuis la
/// couleur de fond de la palette. Le grège donne « clair », l'encre « sombre ».
pub fn jetons(theme: &Theme) -> &'static Jetons {
    if theme.extended_palette().is_dark {
        &SOMBRE
    } else {
        &CLAIR
    }
}

// --- Thèmes `iced` ----------------------------------------------------------

/// Palette `iced` d'un jeu de jetons.
///
/// Six couleurs seulement : elles ne servent qu'aux widgets non stylés et à
/// déterminer `is_dark`. Tout le reste passe par [`Jetons`].
fn palette(j: &Jetons) -> Palette {
    Palette {
        background: j.surface,
        text: j.texte,
        primary: j.garance,
        success: j.celadon,
        warning: j.or,
        danger: j.garance,
    }
}

/// Le thème « Sericæ clair ».
pub fn clair() -> Theme {
    static THEME: LazyLock<Theme> =
        LazyLock::new(|| Theme::custom(i18n::t(Text::ThemeLight), palette(&CLAIR)));
    THEME.clone()
}

/// Le thème « Sericæ sombre ».
pub fn sombre() -> Theme {
    static THEME: LazyLock<Theme> =
        LazyLock::new(|| Theme::custom(i18n::t(Text::ThemeDark), palette(&SOMBRE)));
    THEME.clone()
}

/// Le thème correspondant au mode du système ; [`Mode::None`] retombe sur le
/// mode clair.
///
/// [`Mode::None`]: iced::theme::Mode::None
pub fn selon(mode: iced::theme::Mode) -> Theme {
    match mode {
        iced::theme::Mode::Dark => sombre(),
        _ => clair(),
    }
}

// --- Échelles ---------------------------------------------------------------

/// Échelle d'espacement, en points logiques. Aucune autre valeur n'est admise.
pub const ESPACES: [f32; 8] = [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0];

/// Espacement 4 : l'interstice, entre deux éléments d'un même mot visuel.
pub const ESPACE_XS: f32 = ESPACES[0];
/// Espacement 8 : entre deux éléments d'une même ligne.
pub const ESPACE_S: f32 = ESPACES[1];
/// Espacement 12 : entre deux lignes d'une liste.
pub const ESPACE_M: f32 = ESPACES[2];
/// Espacement 16 : marge intérieure ordinaire.
pub const ESPACE_L: f32 = ESPACES[3];
/// Espacement 24 : entre deux blocs.
pub const ESPACE_XL: f32 = ESPACES[4];
/// Espacement 32 : entre deux sections.
pub const ESPACE_XXL: f32 = ESPACES[5];
/// Espacement 48 : respiration d'une page.
pub const ESPACE_3XL: f32 = ESPACES[6];
/// Espacement 64 : marge d'une page d'accueil.
pub const ESPACE_4XL: f32 = ESPACES[7];

/// Rayon des coins, unique dans toute l'interface.
pub const RAYON: f32 = 8.0;
/// Hauteur minimale d'une cible tactile.
pub const CIBLE_TACTILE: f32 = 44.0;
/// Épaisseur d'un filet ; il n'en existe pas d'autre.
pub const FILET: f32 = 1.0;
/// Décalage du filet de focus autour de l'élément.
pub const FOCUS_OFFSET: f32 = 3.0;
/// Opacité d'un élément désactivé.
pub const OPACITE_DESACTIVE: f32 = 0.45;
/// Opacité d'un élément pressé.
pub const OPACITE_PRESSE: f32 = 0.85;
/// Interligne du texte courant.
pub const INTERLIGNE: f32 = 1.6;
/// Espacement des capitales, en em (voir [`crate::typo::petites_capitales`]).
pub const CAPITALES_ESPACEMENT: f32 = 0.08;

/// Échelle de corps, en points logiques.
pub const CORPS: [f32; 6] = [12.0, 14.0, 17.0, 24.0, 40.0, 68.0];

/// Corps 12 : métadonnées (Inter uniquement — une sérif ne descend pas là).
pub const CORPS_META: f32 = CORPS[0];
/// Corps 14 : interface.
pub const CORPS_INTERFACE: f32 = CORPS[1];
/// Corps 17 : texte courant.
pub const CORPS_TEXTE: f32 = CORPS[2];
/// Corps 24 : titre de section.
pub const CORPS_TITRE: f32 = CORPS[3];
/// Corps 40 : titre de page.
pub const CORPS_DISPLAY: f32 = CORPS[4];
/// Corps 68 : affiche.
pub const CORPS_AFFICHE: f32 = CORPS[5];

/// Corps en deçà duquel une sérif n'est plus employée en interface.
pub const CORPS_MINIMAL_SERIF: f32 = 13.0;

// --- Contraste --------------------------------------------------------------

/// Luminance relative WCAG 2.1 d'une couleur (canal alpha ignoré).
pub fn luminance_relative(c: Color) -> f32 {
    fn lineaire(v: f32) -> f32 {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lineaire(c.r) + 0.7152 * lineaire(c.g) + 0.0722 * lineaire(c.b)
}

/// Rapport de contraste WCAG 2.1 entre deux couleurs, de 1,0 à 21,0.
///
/// L'ordre des arguments est indifférent. Le canal alpha n'est pas composé :
/// n'appelle cette fonction que sur des couleurs opaques.
pub fn contraste(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance_relative(a), luminance_relative(b));
    let (haut, bas) = if a >= b { (a, b) } else { (b, a) };
    (haut + 0.05) / (bas + 0.05)
}

/// Seuil de contraste exigé à un corps donné : 3,0 à partir de 24 px (« grand
/// texte » au sens WCAG), 4,5 en dessous.
pub fn seuil_contraste(corps: f32) -> f32 {
    if corps >= CORPS_TITRE {
        3.0
    } else {
        4.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les jetons sont les matières annoncées par le design system.
    #[test]
    fn les_jetons_distribuent_les_matieres_annoncees() {
        assert_eq!(CLAIR.surface, GREGE);
        assert_eq!(CLAIR.surface_2, GREGE_2);
        assert_eq!(CLAIR.titre, ENCRE);
        assert_eq!(CLAIR.texte, SEPIA);
        assert_eq!(CLAIR.graisse_texte, Weight::Normal);
        assert_eq!(SOMBRE.surface, ENCRE);
        assert_eq!(SOMBRE.surface_2, ENCRE_2);
        assert_eq!(SOMBRE.titre, GREGE);
        assert_eq!(SOMBRE.texte, GREGE);
        assert_eq!(SOMBRE.graisse_texte, Weight::Light);
        // Les invariants entre les deux modes.
        assert_eq!(CLAIR.or, SOMBRE.or);
        assert_eq!(CLAIR.garance, SOMBRE.garance);
        assert_eq!(CLAIR.voile_d_or, SOMBRE.voile_d_or);
        assert_eq!(VOILE_D_OR.a, 0.35);
    }

    /// `jetons()` suit `is_dark`, calculé par `iced` depuis le fond.
    #[test]
    fn les_jetons_suivent_le_mode_du_theme() {
        assert!(!clair().extended_palette().is_dark);
        assert!(sombre().extended_palette().is_dark);
        assert_eq!(*jetons(&clair()), CLAIR);
        assert_eq!(*jetons(&sombre()), SOMBRE);
        assert_eq!(*jetons(&selon(iced::theme::Mode::Dark)), SOMBRE);
        assert_eq!(*jetons(&selon(iced::theme::Mode::Light)), CLAIR);
        // Sans préférence connue, on reste sur le papier.
        assert_eq!(*jetons(&selon(iced::theme::Mode::None)), CLAIR);
    }

    /// Les thèmes portent leur nom traduit et ne sont construits qu'une fois.
    #[test]
    fn les_themes_portent_leur_nom() {
        assert_eq!(clair().to_string(), i18n::t(Text::ThemeLight));
        assert_eq!(sombre().to_string(), i18n::t(Text::ThemeDark));
    }

    /// Repères WCAG : noir sur blanc = 21:1, une couleur sur elle-même = 1:1.
    #[test]
    fn le_contraste_suit_wcag() {
        assert!((contraste(Color::BLACK, Color::WHITE) - 21.0).abs() < 0.01);
        assert!((contraste(GREGE, GREGE) - 1.0).abs() < 0.001);
        // L'ordre des arguments est indifférent.
        assert_eq!(contraste(ENCRE, GREGE), contraste(GREGE, ENCRE));
    }

    /// La raison d'être de la règle « l'or ne fait jamais du texte » : 2,5:1
    /// sur le grège, contre 6,0:1 sur l'encre.
    #[test]
    fn l_or_est_illisible_sur_le_grege_et_lisible_sur_l_encre() {
        let sur_grege = contraste(OR, GREGE);
        assert!(
            (2.4..2.6).contains(&sur_grege),
            "or sur grège : {sur_grege}"
        );
        let sur_encre = contraste(OR, ENCRE);
        assert!(
            (5.8..6.2).contains(&sur_encre),
            "or sur encre : {sur_encre}"
        );
    }

    /// Le texte secondaire tient sur la surface, pas sur la surface posée :
    /// c'est ce qui justifie la restriction portée par `Jetons::texte_2`.
    #[test]
    fn le_texte_secondaire_est_reserve_a_la_surface() {
        assert!(contraste(CLAIR.texte_2, CLAIR.surface) >= 4.5);
        assert!(contraste(SOMBRE.texte_2, SOMBRE.surface) >= 4.5);
        assert!(
            contraste(CLAIR.texte_2, CLAIR.surface_2) < 4.5,
            "si la surface-2 change, la restriction de `texte_2` doit être revue"
        );
    }

    /// Les échelles n'ont pas de trou et le seuil suit le corps.
    #[test]
    fn les_echelles_sont_ordonnees() {
        assert!(ESPACES.is_sorted());
        assert!(CORPS.is_sorted());
        assert_eq!(seuil_contraste(CORPS_META), 4.5);
        assert_eq!(seuil_contraste(CORPS_TEXTE), 4.5);
        assert_eq!(seuil_contraste(CORPS_TITRE), 3.0);
        assert_eq!(seuil_contraste(CORPS_AFFICHE), 3.0);
    }
}
