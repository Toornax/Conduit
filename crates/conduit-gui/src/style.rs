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
use iced::widget::{button, container, pick_list, rule, scrollable, slider, text, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

use crate::theme::{
    jetons, Jetons, CIBLE_TACTILE, FILET, OPACITE_DESACTIVE, OPACITE_PRESSE, RAYON,
};

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

/// Corps du libellé d'un bouton : Inter 13 px, en capitales espacées.
pub const CORPS_BOUTON: f32 = 13.0;

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

/// Onglet de la barre latérale : ni fond, ni filet, ni rayon — le libellé
/// seul, souligné d'un filet d'or par la vue quand l'onglet est actif.
///
/// L'onglet actif porte l'accent lisible du mode (garance en clair, or en
/// sombre) ; l'inactif porte le texte courant et glisse vers l'accent au
/// survol.
pub fn onglet(actif: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let j = jetons(theme);
        let accent = actif || status == button::Status::Hovered;
        finir(
            status,
            button::Style {
                background: None,
                text_color: if accent { j.accent_texte } else { j.texte },
                border: Border::default(),
                ..button::Style::default()
            },
        )
    }
}

/// Bouton de pas d'une grandeur — les « − » et « + » des canaux : carré,
/// sans fond, cerné d'un filet de contour.
///
/// Le libellé porte le texte courant : le nombre de canaux se lit, il ne
/// s'efface pas. Le survol glisse vers l'accent lisible du mode, filet
/// compris.
pub fn bouton_pas(theme: &Theme, status: button::Status) -> button::Style {
    let j = jetons(theme);
    let survole = status == button::Status::Hovered;
    finir(
        status,
        button::Style {
            background: None,
            text_color: if survole { j.accent_texte } else { j.texte },
            border: filet_de(if survole { j.accent_texte } else { j.contour }),
            ..button::Style::default()
        },
    )
}

/// Bouton discret : ni fond, ni filet — la croix de suppression et les
/// libellés d'appoint d'une ligne.
///
/// Le libellé porte le texte secondaire des surfaces posées
/// ([`crate::theme::Jetons::texte_2_carte`]) : c'est une action d'appoint, elle
/// s'efface devant le contenu de la ligne. Le survol glisse vers l'accent
/// lisible du mode : garance en clair, or en sombre, où la garance ne passe que
/// 2,0:1.
pub fn bouton_discret(theme: &Theme, status: button::Status) -> button::Style {
    let j = jetons(theme);
    finir(
        status,
        button::Style {
            background: None,
            text_color: if status == button::Status::Hovered {
                j.accent_texte
            } else {
                // Discret : il vit dans une ligne, donc sur une surface posée.
                j.texte_2_carte
            },
            border: Border::default(),
            ..button::Style::default()
        },
    )
}

/// Rayon du bouton de coupure : 24 × 22 px ne portent pas le rayon commun de
/// 8 sans devenir une pastille.
pub const RAYON_MUET: f32 = 6.0;

/// Bouton de coupure d'un gain — le « M » du pied d'une carte de nœud et du
/// réglage de lien de l'en-tête.
///
/// Au repos, il ressemble au [bouton de pas](bouton_pas) : ni fond, filet de
/// contour, libellé en texte secondaire des surfaces posées, survol qui glisse
/// vers l'accent du mode. **Coupé**, il devient un aplat de garance — la
/// matière de l'alerte, invariante entre les deux modes — dont le libellé
/// prend le grège : c'est le maximum de contraste disponible sur la garance
/// (7,5:1, contre 2,0:1 pour l'encre), et il ne peut donc pas suivre le mode.
///
/// Le survol n'ajoute rien à l'état coupé : c'est déjà l'état le plus marqué
/// de la carte, et une seconde couleur d'accent y contredirait le thème.
pub fn bouton_muet(coupe: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let j = jetons(theme);
        let survole = status == button::Status::Hovered;
        let (fond_bouton, encre, filet) = if coupe {
            // `CLAIR.surface` et non `j.surface` : la garance ne change pas
            // d'un mode à l'autre, son partenaire lisible non plus.
            (
                Some(fond(j.garance)),
                crate::theme::CLAIR.surface,
                j.garance,
            )
        } else if survole {
            (None, j.accent_texte, j.accent_texte)
        } else {
            (None, j.texte_2_carte, j.contour)
        };
        finir(
            status,
            button::Style {
                background: fond_bouton,
                text_color: encre,
                border: Border {
                    radius: Radius::from(RAYON_MUET),
                    ..filet_de(filet)
                },
                ..button::Style::default()
            },
        )
    }
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

/// Ligne d'une liste : la surface posée de [`carte`], **sans filet**.
///
/// Une liste de lignes cernées chacune d'un filet ferait une grille de
/// tableau : les lignes se détachent du fond par leur seule matière.
pub fn ligne(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: rayon(),
            ..Border::default()
        },
        ..carte(theme)
    }
}

/// Zone bordée : un simple filet de contour au rayon commun, sans fond.
///
/// Elle délimite une aire de travail — la scène du patchbay — sans la poser
/// sur une surface : les cartes qui s'y promènent, elles, sont posées.
pub fn zone(theme: &Theme) -> container::Style {
    container::Style {
        border: filet_de(jetons(theme).contour),
        ..container::Style::default()
    }
}

/// Opacité d'une carte de nœud suspendu : le nœud existe encore, ses liens
/// aussi, mais son périphérique est absent — la carte s'efface sans
/// disparaître.
pub const OPACITE_SUSPENDU: f32 = 0.5;

/// Carte d'un nœud du patchbay : la surface posée de [`carte`], cernée d'un
/// filet **d'or** quand le nœud pilote le graphe.
///
/// `opacite` voile l'ensemble — fond, filet et texte — pour un nœud suspendu
/// ([`OPACITE_SUSPENDU`]) ; elle vaut 1 partout ailleurs. `iced` n'a pas de
/// widget d'opacité : le voile se fait donc dans les couleurs.
pub fn carte_noeud(pilote: bool, opacite: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let j = jetons(theme);
        let filet = if pilote { j.or } else { j.contour };
        container::Style {
            text_color: Some(opacifier(j.texte, opacite)),
            background: Some(fond(opacifier(j.surface_2, opacite))),
            border: filet_de(opacifier(filet, opacite)),
            ..container::Style::default()
        }
    }
}

/// Rayon d'une barre de niveau : une barre de 4 px de haut ne peut pas porter
/// un rayon de 8 (voir aussi [`RAYON_MUET`]).
pub const RAYON_BARRE: f32 = 2.0;

/// Rail d'une barre de niveau : le fond de la fenêtre creusé dans la ligne.
pub fn barre_rail(theme: &Theme) -> container::Style {
    barre(jetons(theme).surface)
}

/// Remplissage d'une barre de niveau : l'or, qui ne fait ici qu'un aplat.
pub fn barre_remplie(theme: &Theme) -> container::Style {
    barre(jetons(theme).or)
}

/// Un aplat de barre, au [rayon des barres](RAYON_BARRE).
fn barre(couleur: Color) -> container::Style {
    container::Style {
        background: Some(fond(couleur)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(RAYON_BARRE),
        },
        ..container::Style::default()
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

/// Pastille dont la couleur est choisie parmi les jetons du mode.
///
/// Sert aux états qui n'ont pas de matière invariante : l'état inactif d'un
/// câble, par exemple, prend le texte secondaire du mode.
pub fn pastille_de(choix: fn(&Jetons) -> Color) -> impl Fn(&Theme) -> container::Style {
    pastille_voilee(choix, 1.0)
}

/// Pastille des jetons du mode, voilée du même facteur que la carte qui la
/// porte (voir [`carte_noeud`]).
pub fn pastille_voilee(
    choix: fn(&Jetons) -> Color,
    opacite: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |theme| pastille(opacifier(choix(jetons(theme)), opacite))(theme)
}

// --- Encres -----------------------------------------------------------------

/// Couleur d'un texte, choisie parmi les jetons du mode.
///
/// La closure de style est le seul endroit où le thème est connu :
/// `text(…).style(`[`texte_en`]`(|j| j.titre))` suit donc le mode sans que la
/// vue ait à le lire.
pub fn texte_en(choix: fn(&Jetons) -> Color) -> impl Fn(&Theme) -> text::Style {
    texte_voile(choix, 1.0)
}

/// Couleur d'un texte, voilée du même facteur que la carte qui le porte (voir
/// [`carte_noeud`]).
pub fn texte_voile(choix: fn(&Jetons) -> Color, opacite: f32) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style {
        color: Some(opacifier(choix(jetons(theme)), opacite)),
    }
}

/// Conteneur qui n'impose qu'une couleur de texte, sans fond ni filet.
///
/// Sert aux capitales espacées : [`crate::typo::petites_capitales`] compose
/// une `Row` de `text` qui ne portent pas de couleur, et un conteneur la leur
/// donne (`iced` propage `text_color` à ce qu'il contient).
pub fn encre_de(choix: fn(&Jetons) -> Color) -> impl Fn(&Theme) -> container::Style {
    encre_voilee(choix, 1.0)
}

/// Conteneur qui n'impose qu'une couleur de texte, voilée du même facteur que
/// la carte qui le porte (voir [`carte_noeud`]).
pub fn encre_voilee(
    choix: fn(&Jetons) -> Color,
    opacite: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        text_color: Some(opacifier(choix(jetons(theme)), opacite)),
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
/// Champ de saisie sans filet, souligné par un `rule` posé dessous.
///
/// La maquette veut un simple soulignement ; `iced` ne borde qu'un rectangle
/// entier. Le champ est donc rendu nu et l'appelant pose le filet, ce qui lui
/// permet au passage d'en changer la couleur selon l'édition en cours.
pub fn champ_nu(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let j = jetons(theme);
    let atone = matches!(status, text_input::Status::Disabled);
    text_input::Style {
        background: fond(Color::TRANSPARENT),
        // Aucun filet : `iced` ne sait border qu'un rectangle entier, alors que
        // le dessin veut un simple soulignement. Celui-ci est un `rule` posé
        // sous le champ par l'appelant, dont la couleur suit l'édition en cours
        // (voir `cables::alias`).
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(0.0),
        },
        icon: j.texte_2_carte,
        placeholder: j.texte_2_carte,
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

/// Curseur de gain voilé du même facteur que la carte qui le porte (voir
/// [`carte_noeud`]).
///
/// `iced` n'a pas d'état désactivé pour une glissière : le voile d'un nœud
/// suspendu est tout ce qui le dit à l'œil, et c'est la réduction du geste qui
/// le tient pour de bon (voir [`crate::patchbay::State::reduire`]).
pub fn curseur_voile(opacite: f32) -> impl Fn(&Theme, slider::Status) -> slider::Style {
    move |theme, status| {
        let mut style = curseur_gain(theme, status);
        let (gauche, droite) = style.rail.backgrounds;
        style.rail.backgrounds = (voiler(gauche, opacite), voiler(droite, opacite));
        style.handle.background = voiler(style.handle.background, opacite);
        style.handle.border_color = opacifier(style.handle.border_color, opacite);
        style
    }
}

/// Un fond voilé du facteur donné ; les fonds du thème sont tous des aplats.
fn voiler(arriere_plan: Background, opacite: f32) -> Background {
    match arriere_plan {
        Background::Color(c) => fond(opacifier(c, opacite)),
        autre => autre,
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

    use crate::theme::{contraste, seuil_contraste, CLAIR, CORPS_META, GARANCE, GREGE, OR, SOMBRE};

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
        // Les boutons d'une ligne se lisent sur la surface posée, et non sur
        // celle de la fenêtre.
        let boutons_de_ligne: [StyleBouton; 2] = [("pas", bouton_pas), ("discret", bouton_discret)];
        for (quoi, style) in boutons_de_ligne {
            for status in ETATS_OPAQUES {
                couples.push((quoi, style(theme, status).text_color, j.surface_2));
            }
        }
        // Le bouton de coupure : au repos sur la surface qui le porte — la
        // carte d'un nœud, ou le fond de la fenêtre pour un lien —, coupé sur
        // son propre aplat de garance.
        for status in ETATS_OPAQUES {
            let repos = bouton_muet(false)(theme, status);
            couples.push(("muet", repos.text_color, j.surface_2));
            couples.push(("muet.entête", repos.text_color, j.surface));
            let coupe = bouton_muet(true)(theme, status);
            if let Some(Background::Color(f)) = coupe.background {
                couples.push(("muet.coupé", coupe.text_color, f));
            }
        }
        for actif in [false, true] {
            let style = onglet(actif);
            for status in ETATS_OPAQUES {
                // Un onglet n'a pas de fond : il se lit sur la barre latérale.
                couples.push(("onglet", style(theme, status).text_color, j.surface));
            }
        }
        for choix in [
            ("encre.titre", j.titre),
            ("encre.texte", j.texte),
            ("encre.texte_2", j.texte_2),
        ] {
            couples.push((choix.0, choix.1, j.surface));
        }
        let conteneurs: [StyleConteneur; 5] = [
            ("surface", surface),
            ("carte", carte),
            ("ligne", ligne),
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
        // La carte d'un nœud du patchbay : son libellé, son étiquette d'état
        // et le nom de ses ports se lisent sur la surface posée.
        for (quoi, encre) in [
            ("noeud.titre", j.titre),
            ("noeud.etiquette", j.accent_texte),
            ("noeud.port", j.texte_2_carte),
        ] {
            couples.push((quoi, encre, j.surface_2));
        }
        // Le champ d'alias vit dans une ligne, donc sur une surface posée.
        let champ = champ_nu(theme, text_input::Status::Active);
        couples.push(("champ.valeur", champ.value, j.surface_2));
        couples.push(("champ.invite", champ.placeholder, j.surface_2));
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
                    bouton_pas(&theme, status),
                    bouton_discret(&theme, status),
                    bouton_muet(false)(&theme, status),
                    bouton_muet(true)(&theme, status),
                    lien(&theme, status),
                ] {
                    assert_eq!(style.shadow, Shadow::default());
                    assert!(matches!(
                        style.background,
                        None | Some(Background::Color(_))
                    ));
                }
            }
            for style in [
                surface(&theme),
                carte(&theme),
                ligne(&theme),
                barre_rail(&theme),
                barre_remplie(&theme),
                notice_info(&theme),
                carte_noeud(false, 1.0)(&theme),
                carte_noeud(true, OPACITE_SUSPENDU)(&theme),
            ] {
                assert_eq!(style.shadow, Shadow::default());
                assert!(matches!(
                    style.background,
                    None | Some(Background::Color(_))
                ));
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
                // `champ_nu` n'a délibérément aucun filet : son soulignement
                // est un `rule` posé par l'appelant.
            ];
            for status in ETATS_OPAQUES {
                bordures.push(bouton_primaire(&theme, status).border);
                bordures.push(bouton_secondaire(&theme, status).border);
                bordures.push(bouton_pas(&theme, status).border);
                bordures.push(bouton_muet(false)(&theme, status).border);
                bordures.push(bouton_muet(true)(&theme, status).border);
            }
            for b in bordures {
                assert_eq!(b.width, FILET, "filet de {} px", b.width);
            }
            // Le lien, le bouton discret, la ligne et les barres n'ont pas de
            // bordure du tout.
            for sans in [
                lien(&theme, button::Status::Active).border,
                bouton_discret(&theme, button::Status::Active).border,
                ligne(&theme).border,
                barre_rail(&theme).border,
                barre_remplie(&theme).border,
            ] {
                assert_eq!(sans.width, 0.0);
            }
            // Une barre de 4 px de haut porte le seul rayon dérogatoire.
            assert_eq!(barre_rail(&theme).border.radius, Radius::from(RAYON_BARRE));
            assert_eq!(ligne(&theme).border.radius, rayon());
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

    /// L'onglet actif porte l'accent du mode, l'inactif le texte courant, et
    /// ni l'un ni l'autre de fond ou de filet.
    #[test]
    fn l_onglet_actif_porte_l_accent() {
        for theme in themes() {
            let j = jetons(&theme);
            let actif = onglet(true)(&theme, button::Status::Active);
            let inactif = onglet(false)(&theme, button::Status::Active);
            assert_eq!(actif.text_color, j.accent_texte);
            assert_eq!(inactif.text_color, j.texte);
            assert!(actif.background.is_none() && inactif.background.is_none());
            assert_eq!(actif.border.width, 0.0);
            // Le survol d'un onglet inactif glisse vers l'accent.
            assert_eq!(
                onglet(false)(&theme, button::Status::Hovered).text_color,
                j.accent_texte
            );
        }
    }

    /// Les encres nommées rendent bien le jeton demandé.
    #[test]
    fn les_encres_rendent_le_jeton_demande() {
        for theme in themes() {
            let j = jetons(&theme);
            assert_eq!(texte_en(|j| j.titre)(&theme).color, Some(j.titre));
            assert_eq!(encre_de(|j| j.texte_2)(&theme).text_color, Some(j.texte_2));
            // Une encre ne pose ni fond ni filet.
            let encre = encre_de(|j| j.texte)(&theme);
            assert!(encre.background.is_none());
            assert_eq!(encre.border.width, 0.0);
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

    /// Le voile d'un nœud suspendu descend jusque dans son curseur.
    #[test]
    fn le_curseur_se_voile_comme_la_carte_qui_le_porte() {
        for theme in themes() {
            let j = jetons(&theme);
            let plein = curseur_voile(1.0)(&theme, slider::Status::Active);
            assert_eq!(plein, curseur_gain(&theme, slider::Status::Active));
            let voile = curseur_voile(OPACITE_SUSPENDU)(&theme, slider::Status::Active);
            assert_eq!(
                voile.rail.backgrounds.0,
                Background::Color(opacifier(j.or, OPACITE_SUSPENDU))
            );
            assert_eq!(
                voile.handle.border_color.a,
                j.or.a * OPACITE_SUSPENDU,
                "le poignet se voile aussi"
            );
            // Le voile n'est qu'une opacité : les couleurs ne changent pas.
            assert_eq!(voile.handle.shape, plein.handle.shape);
        }
    }

    /// Le bouton de coupure : filet de contour au repos, aplat de garance
    /// quand la coupure est active — et son libellé sur la matière claire, qui
    /// est le seul ton lisible sur la garance dans les deux modes.
    #[test]
    fn le_bouton_muet_passe_a_la_garance_quand_il_coupe() {
        for theme in themes() {
            let j = jetons(&theme);
            let repos = bouton_muet(false)(&theme, button::Status::Active);
            assert!(repos.background.is_none());
            assert_eq!(repos.text_color, j.texte_2_carte);
            assert_eq!(repos.border.color, j.contour);
            assert_eq!(repos.border.radius, Radius::from(RAYON_MUET));

            let coupe = bouton_muet(true)(&theme, button::Status::Active);
            assert_eq!(coupe.background, Some(Background::Color(j.garance)));
            assert_eq!(coupe.border.color, j.garance);
            assert_eq!(coupe.text_color, CLAIR.surface);
            // Invariant entre les deux modes : la garance l'est, son partenaire
            // lisible doit l'être aussi.
            assert_eq!(coupe.text_color, GREGE);
            assert!(
                contraste(coupe.text_color, GARANCE) >= 4.5,
                "la matière claire doit tenir sur la garance"
            );
            assert!(
                contraste(SOMBRE.surface, GARANCE) < 4.5,
                "si l'encre passait, ce choix serait à revoir"
            );
            // Le survol n'ajoute rien à l'état coupé.
            assert_eq!(coupe, bouton_muet(true)(&theme, button::Status::Hovered));
            // Au repos, en revanche, il glisse vers l'accent du mode.
            let survol = bouton_muet(false)(&theme, button::Status::Hovered);
            assert_eq!(survol.text_color, j.accent_texte);
            assert_eq!(survol.border.color, j.accent_texte);
        }
    }
}
