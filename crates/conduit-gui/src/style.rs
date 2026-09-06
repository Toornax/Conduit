//! Styles des widgets `iced` pour le thème « Sericæ ».
//!
//! Chaque fonction de ce module est une closure de style d'`iced` : elle ne
//! reçoit qu'un `&Theme` (et parfois un `Status`) et rend la `Style` du widget.
//! Les couleurs viennent de [`crate::theme::jetons`], jamais d'une constante
//! écrite sur place.
//!
//! Trois invariants tenus ici, et vérifiés par les tests :
//!
//! - **aucune ombre, aucun dégradé** : toutes les `Shadow` sont
//!   `Shadow::default()`, tous les fonds sont des `Background::Color` ;
//! - **les bordures sont des filets de 1 px** ([`FILET`]) ;
//! - **l'or ne fait jamais du texte sur une surface claire** : il ne sert
//!   qu'aux filets, pastilles et barres — et, sur l'encre seulement, au
//!   libellé du bouton primaire (6,0:1).
//!
//! Le survol glisse vers l'or ou la garance, sans changement d'échelle ni
//! ombre ; la pression baisse l'opacité à [`OPACITE_PRESSE`] ; l'état
//! désactivé à [`OPACITE_DESACTIVE`].

use iced::border::Radius;
use iced::widget::overlay::menu;
use iced::widget::{button, container, pick_list, rule, scrollable, slider, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

use crate::theme::{jetons, CIBLE_TACTILE, FILET, OPACITE_DESACTIVE, OPACITE_PRESSE, RAYON};

// --- Outils -----------------------------------------------------------------

/// Une couleur dont l'opacité est multipliée par `facteur`.
fn opacifier(couleur: Color, facteur: f32) -> Color {
    Color {
        a: couleur.a * facteur,
        ..couleur
    }
}

/// Le rayon commun.
fn rayon() -> Radius {
    Radius::from(RAYON)
}

/// Un filet de 1 px, de la couleur donnée, au rayon commun.
fn filet_de(couleur: Color) -> Border {
    Border {
        color: couleur,
        width: FILET,
        radius: rayon(),
    }
}

/// Fond opaque : le seul type de fond du thème (ni dégradé, ni flou).
fn fond(couleur: Color) -> Background {
    Background::Color(couleur)
}

// --- Boutons ----------------------------------------------------------------

/// Hauteur minimale d'un bouton : la cible tactile.
///
/// Le style d'`iced` ne porte pas la hauteur ; les vues appliquent
/// `.height(`[`HAUTEUR_BOUTON`]`).padding([0, `[`PADDING_BOUTON`]`])`.
pub const HAUTEUR_BOUTON: f32 = CIBLE_TACTILE;

/// Padding horizontal d'un bouton.
pub const PADDING_BOUTON: f32 = 28.0;

/// Applique la baisse d'opacité des états pressé et désactivé.
fn finir(status: button::Status, mut style: button::Style) -> button::Style {
    let facteur = match status {
        button::Status::Pressed => OPACITE_PRESSE,
        button::Status::Disabled => OPACITE_DESACTIVE,
        button::Status::Active | button::Status::Hovered => return style,
    };
    style.text_color = opacifier(style.text_color, facteur);
    style.border.color = opacifier(style.border.color, facteur);
    if let Some(Background::Color(c)) = style.background {
        style.background = Some(fond(opacifier(c, facteur)));
    }
    style
}

/// Bouton primaire : fond plein, libellé en accent, filet de la couleur du
/// fond.
///
/// En clair, fond encre et libellé **or** (6,0:1 sur l'encre) qui passe grège
/// au survol. En sombre, les rôles inversés : fond grège, libellé encre qui
/// passe garance au survol.
pub fn bouton_primaire(theme: &Theme, status: button::Status) -> button::Style {
    let j = jetons(theme);
    // `titre` est la matière pleine du bouton : encre en clair, grège en
    // sombre ; `surface` est donc toujours son exact opposé.
    let plein = j.titre;
    let (repos, survol) = if j.sombre {
        (j.surface, j.garance)
    } else {
        (j.or, j.surface)
    };
    finir(
        status,
        button::Style {
            background: Some(fond(plein)),
            text_color: if status == button::Status::Hovered {
                survol
            } else {
                repos
            },
            border: filet_de(plein),
            ..button::Style::default()
        },
    )
}

/// Bouton secondaire : sans fond, filet et libellé de la même couleur.
///
/// En clair, sépia qui glisse vers la garance au survol. En sombre, grège qui
/// glisse vers l'or — la garance ne passe que 2,0:1 sur l'encre.
pub fn bouton_secondaire(theme: &Theme, status: button::Status) -> button::Style {
    let j = jetons(theme);
    let couleur = if status == button::Status::Hovered {
        j.accent_texte
    } else {
        j.texte
    };
    finir(
        status,
        button::Style {
            background: None,
            text_color: couleur,
            border: filet_de(couleur),
            ..button::Style::default()
        },
    )
}

/// Lien : ni fond, ni filet, ni padding — la vue pose `.padding(0)`.
///
/// Le libellé est en accent (garance en clair, or en sombre) et passe au
/// maximum de contraste au survol. `iced` ne sait pas souligner un texte : le
/// soulignement se compose sous le libellé avec [`filet_accent`].
pub fn lien(theme: &Theme, status: button::Status) -> button::Style {
    let j = jetons(theme);
    finir(
        status,
        button::Style {
            background: None,
            text_color: if status == button::Status::Hovered {
                j.titre
            } else {
                j.accent_texte
            },
            border: Border::default(),
            ..button::Style::default()
        },
    )
}

// --- Surfaces ---------------------------------------------------------------

/// Fond de la fenêtre.
pub fn surface(theme: &Theme) -> container::Style {
    let j = jetons(theme);
    container::Style {
        text_color: Some(j.texte),
        background: Some(fond(j.surface)),
        ..container::Style::default()
    }
}

/// Surface posée : fond surface-2, filet de contour, rayon 8.
///
/// Le texte y est le texte courant et non le texte secondaire, qui ne tient
/// pas le contraste sur la surface-2 (voir [`crate::theme::Jetons::texte_2`]).
pub fn carte(theme: &Theme) -> container::Style {
    let j = jetons(theme);
    container::Style {
        text_color: Some(j.texte),
        background: Some(fond(j.surface_2)),
        border: filet_de(j.contour),
        ..container::Style::default()
    }
}

/// Notice d'information : surface posée cernée d'un filet céladon.
///
/// L'accent est le filet, pas le texte : le céladon ne passe que 2,0:1 sur
/// l'encre.
pub fn notice_info(theme: &Theme) -> container::Style {
    let j = jetons(theme);
    container::Style {
        border: filet_de(j.celadon),
        ..carte(theme)
    }
}

/// Notice d'erreur : surface posée cernée d'un filet garance.
pub fn notice_erreur(theme: &Theme) -> container::Style {
    let j = jetons(theme);
    container::Style {
        border: filet_de(j.garance),
        ..carte(theme)
    }
}

/// Pastille pleine de la couleur donnée : un point, jamais du texte.
///
/// À poser sur un conteneur de taille fixe, par exemple
/// `container(space::horizontal()).width(8).height(8).style(pastille(or))`.
pub fn pastille(couleur: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(fond(couleur)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(CIBLE_TACTILE),
        },
        ..container::Style::default()
    }
}

// --- Filets -----------------------------------------------------------------

/// Filet de contour, sur toute la longueur du parent.
///
/// `rule::horizontal(1)` prend l'**épaisseur** : la longueur remplit le
/// parent, donc un filet de 96 px s'écrit
/// `container(rule::horizontal(1)).width(96)`.
pub fn filet(theme: &Theme) -> rule::Style {
    trait_de(jetons(theme).contour)
}

/// Le fil d'or : filet d'accent, invariant entre les deux modes.
pub fn filet_or(theme: &Theme) -> rule::Style {
    trait_de(jetons(theme).or)
}

/// Filet de la couleur d'accent lisible du mode : sert de soulignement aux
/// liens (voir [`lien`]).
pub fn filet_accent(theme: &Theme) -> rule::Style {
    trait_de(jetons(theme).accent_texte)
}

/// Un filet plein, sans rayon.
fn trait_de(couleur: Color) -> rule::Style {
    rule::Style {
        color: couleur,
        radius: Radius::from(0.0),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

// --- Saisie -----------------------------------------------------------------

/// Champ de saisie « souligné » : sans fond, cerné d'un filet sans rayon.
///
/// `iced` ne sait pas border un seul côté : le champ porte donc un filet
/// complet de 1 px, sans rayon, qui joue le rôle du soulignement. Le filet
/// glisse vers l'or au survol et vers la garance au focus.
pub fn champ_souligne(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let j = jetons(theme);
    let couleur = match status {
        text_input::Status::Active => j.contour,
        text_input::Status::Hovered => j.or,
        text_input::Status::Focused { .. } => j.garance,
        text_input::Status::Disabled => opacifier(j.contour, OPACITE_DESACTIVE),
    };
    let atone = matches!(status, text_input::Status::Disabled);
    text_input::Style {
        background: fond(Color::TRANSPARENT),
        border: Border {
            color: couleur,
            width: FILET,
            radius: Radius::from(0.0),
        },
        icon: j.texte_2,
        placeholder: j.texte_2,
        value: if atone {
            opacifier(j.texte, OPACITE_DESACTIVE)
        } else {
            j.texte
        },
        selection: j.voile_d_or,
    }
}

/// Curseur de gain : rail en deux temps, or à gauche du poignet, contour à
/// droite ; poignet rond cerné d'un filet qui passe à la garance dès qu'on le
/// touche.
pub fn curseur_gain(theme: &Theme, status: slider::Status) -> slider::Style {
    let j = jetons(theme);
    let bordure_poignet = match status {
        slider::Status::Active => j.or,
        slider::Status::Hovered | slider::Status::Dragged => j.garance,
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (fond(j.or), fond(j.contour)),
            width: 2.0,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(1.0),
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: fond(j.surface),
            border_width: FILET,
            border_color: bordure_poignet,
        },
    }
}

/// Liste déroulante fermée : surface posée, filet de contour qui glisse vers
/// l'or au survol et vers la garance à l'ouverture.
///
/// La liste **ouverte** est un `overlay` distinct : sans
/// `PickList::menu_style(`[`menu_deroulant`]`)`, elle garde le thème `iced`
/// par défaut.
pub fn liste_deroulante(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let j = jetons(theme);
    let couleur = match status {
        pick_list::Status::Active => j.contour,
        pick_list::Status::Hovered => j.or,
        pick_list::Status::Opened { .. } => j.garance,
    };
    pick_list::Style {
        text_color: j.texte,
        placeholder_color: j.texte_2,
        handle_color: j.or,
        background: fond(j.surface_2),
        border: filet_de(couleur),
    }
}

/// Liste déroulante ouverte : la même surface posée, l'option survolée
/// marquée par le voile d'or.
pub fn menu_deroulant(theme: &Theme) -> menu::Style {
    let j = jetons(theme);
    menu::Style {
        background: fond(j.surface_2),
        border: filet_de(j.contour),
        text_color: j.texte,
        selected_text_color: j.texte,
        selected_background: fond(j.voile_d_or),
        shadow: Shadow::default(),
    }
}

/// Défilement : rail invisible, poignet en voile d'or qui prend l'or au
/// survol.
pub fn defilement(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let j = jetons(theme);
    let survole = match status {
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered,
            is_horizontal_scrollbar_hovered,
            ..
        } => is_vertical_scrollbar_hovered || is_horizontal_scrollbar_hovered,
        scrollable::Status::Dragged { .. } => true,
        _ => false,
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: fond(if survole { j.or } else { j.voile_d_or }),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(2.0),
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: fond(j.surface_2),
            border: filet_de(j.contour),
            shadow: Shadow::default(),
            icon: j.texte_2,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::theme::{contraste, seuil_contraste, CORPS_META, GREGE, OR};

    /// Une closure de style de bouton, nommée pour le rapport d'échec.
    type StyleBouton = (&'static str, fn(&Theme, button::Status) -> button::Style);
    /// Une closure de style de conteneur, nommée pour le rapport d'échec.
    type StyleConteneur = (&'static str, fn(&Theme) -> container::Style);

    /// Les deux thèmes, pour les parcours de vérification.
    fn themes() -> [Theme; 2] {
        [crate::theme::clair(), crate::theme::sombre()]
    }

    /// Les états d'un bouton où le contraste doit être tenu : l'opacité des
    /// états pressé et désactivé est une atténuation voulue.
    const ETATS_OPAQUES: [button::Status; 2] = [button::Status::Active, button::Status::Hovered];

    /// Tous les couples (texte, fond) que ce module produit, sous la forme
    /// `(quoi, texte, fond)`.
    fn couples(theme: &Theme) -> Vec<(&'static str, Color, Color)> {
        let j = jetons(theme);
        let mut couples = Vec::new();
        let boutons: [StyleBouton; 3] = [
            ("primaire", bouton_primaire),
            ("secondaire", bouton_secondaire),
            ("lien", lien),
        ];
        for (quoi, style) in boutons {
            for status in ETATS_OPAQUES {
                let s = style(theme, status);
                // Un bouton sans fond se lit sur la surface de la fenêtre.
                let derriere = match s.background {
                    Some(Background::Color(c)) => c,
                    _ => j.surface,
                };
                couples.push((quoi, s.text_color, derriere));
            }
        }
        let conteneurs: [StyleConteneur; 4] = [
            ("surface", surface),
            ("carte", carte),
            ("notice_info", notice_info),
            ("notice_erreur", notice_erreur),
        ];
        for (quoi, style) in conteneurs {
            let s = style(theme);
            if let (Some(texte), Some(Background::Color(f))) = (s.text_color, s.background) {
                couples.push((quoi, texte, f));
            }
        }
        let liste = liste_deroulante(theme, pick_list::Status::Active);
        if let Background::Color(f) = liste.background {
            couples.push(("liste_deroulante", liste.text_color, f));
        }
        let menu = menu_deroulant(theme);
        if let Background::Color(f) = menu.background {
            couples.push(("menu", menu.text_color, f));
            couples.push(("menu_selection", menu.selected_text_color, f));
        }
        let champ = champ_souligne(theme, text_input::Status::Active);
        couples.push(("champ.valeur", champ.value, j.surface));
        couples.push(("champ.invite", champ.placeholder, j.surface));
        couples
    }

    /// Aucun couple de texte ne descend sous le seuil WCAG de son corps.
    #[test]
    fn tous_les_couples_de_texte_tiennent_le_seuil_wcag() {
        let seuil = seuil_contraste(CORPS_META);
        assert_eq!(seuil, 4.5);
        for theme in themes() {
            for (quoi, texte, fond) in couples(&theme) {
                let ratio = contraste(texte, fond);
                assert!(
                    ratio >= seuil,
                    "{theme} / {quoi} : {ratio:.2}:1, seuil {seuil}"
                );
            }
        }
    }

    /// L'or ne fait jamais du texte sur une surface claire : il n'y passe que
    /// 2,5:1. Sur l'encre du bouton primaire, il en passe 6,0.
    #[test]
    fn l_or_ne_fait_jamais_du_texte_sur_une_surface_claire() {
        for theme in themes() {
            for (quoi, texte, fond) in couples(&theme) {
                assert!(
                    texte != OR || contraste(OR, GREGE) > 4.5 || fond != GREGE,
                    "{quoi} : de l'or en texte sur du grège"
                );
                assert!(
                    texte != OR || contraste(texte, fond) >= 4.5,
                    "{quoi} : de l'or en texte insuffisamment contrasté"
                );
            }
        }
    }

    /// Ni ombre, ni dégradé, ni fond translucide sous du texte.
    #[test]
    fn aucune_ombre_ni_degrade() {
        for theme in themes() {
            for status in [
                button::Status::Active,
                button::Status::Hovered,
                button::Status::Pressed,
                button::Status::Disabled,
            ] {
                for style in [
                    bouton_primaire(&theme, status),
                    bouton_secondaire(&theme, status),
                    lien(&theme, status),
                ] {
                    assert_eq!(style.shadow, Shadow::default());
                    assert!(matches!(
                        style.background,
                        None | Some(Background::Color(_))
                    ));
                }
            }
            for style in [surface(&theme), carte(&theme), notice_info(&theme)] {
                assert_eq!(style.shadow, Shadow::default());
            }
            assert_eq!(menu_deroulant(&theme).shadow, Shadow::default());
            assert_eq!(
                defilement(
                    &theme,
                    scrollable::Status::Active {
                        is_horizontal_scrollbar_disabled: true,
                        is_vertical_scrollbar_disabled: false,
                    }
                )
                .auto_scroll
                .shadow,
                Shadow::default()
            );
        }
    }

    /// Toutes les bordures visibles sont des filets de 1 px.
    #[test]
    fn les_bordures_sont_des_filets_d_un_pixel() {
        for theme in themes() {
            let mut bordures = vec![
                carte(&theme).border,
                notice_info(&theme).border,
                notice_erreur(&theme).border,
                liste_deroulante(&theme, pick_list::Status::Active).border,
                menu_deroulant(&theme).border,
                champ_souligne(&theme, text_input::Status::Active).border,
            ];
            for status in ETATS_OPAQUES {
                bordures.push(bouton_primaire(&theme, status).border);
                bordures.push(bouton_secondaire(&theme, status).border);
            }
            for b in bordures {
                assert_eq!(b.width, FILET, "filet de {} px", b.width);
            }
            // Le lien n'a pas de bordure du tout.
            assert_eq!(lien(&theme, button::Status::Active).border.width, 0.0);
        }
    }

    /// Le survol glisse vers un accent, la pression et la désactivation ne
    /// jouent que sur l'opacité.
    #[test]
    fn le_survol_glisse_et_la_pression_attenue() {
        for theme in themes() {
            let repos = bouton_secondaire(&theme, button::Status::Active);
            let survol = bouton_secondaire(&theme, button::Status::Hovered);
            assert_ne!(repos.text_color, survol.text_color);
            assert_eq!(survol.text_color, jetons(&theme).accent_texte);

            let presse = bouton_primaire(&theme, button::Status::Pressed);
            let actif = bouton_primaire(&theme, button::Status::Active);
            assert_eq!(presse.text_color.a, actif.text_color.a * OPACITE_PRESSE);
            let eteint = bouton_primaire(&theme, button::Status::Disabled);
            assert_eq!(eteint.text_color.a, actif.text_color.a * OPACITE_DESACTIVE);
            // Ni échelle, ni ombre : seule l'opacité bouge.
            assert_eq!(presse.border.radius, actif.border.radius);
        }
    }

    /// Le fil d'or et la garance sont les mêmes dans les deux modes.
    #[test]
    fn les_filets_d_accent_sont_invariants() {
        assert_eq!(
            filet_or(&crate::theme::clair()).color,
            filet_or(&crate::theme::sombre()).color
        );
        assert_ne!(
            filet(&crate::theme::clair()).color,
            filet(&crate::theme::sombre()).color
        );
    }

    /// Un curseur touché passe à la garance ; le rail reste en or.
    #[test]
    fn le_curseur_marque_la_saisie_par_la_garance() {
        for theme in themes() {
            let j = jetons(&theme);
            let repos = curseur_gain(&theme, slider::Status::Active);
            assert_eq!(repos.handle.border_color, j.or);
            assert_eq!(repos.rail.backgrounds.0, Background::Color(j.or));
            let tire = curseur_gain(&theme, slider::Status::Dragged);
            assert_eq!(tire.handle.border_color, j.garance);
        }
    }
}
