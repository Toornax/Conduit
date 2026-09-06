//! Coquille de la fenêtre : barre latérale, en-tête de contenu et bandeau de
//! notice.
//!
//! Ce module ne connaît aucune vue : il pose le décor et reçoit de l'appelant
//! le contenu de la vue courante et ses actions d'en-tête, s'il y en a.
//!
//! Deux couches, comme partout dans ce crate :
//!
//! - les fonctions **qui décident** — [`EtatDemon::depuis`], [`sous_titre`],
//!   [`chiffres`], [`pilote_plateforme`] — sont pures et testées sans fenêtre
//!   ni moteur de rendu ;
//! - les fonctions **qui composent** rendent des `Element` et ne sont pas
//!   testables autrement qu'à l'œil.
//!
//! Les mesures de la maquette (28, 20, 18, 11, 10…) ne sont pas toutes sur
//! l'échelle d'espacement de [`crate::theme`] : elles sont nommées ici, en un
//! seul endroit, plutôt qu'écrites au fil des vues.

use conduit_protocol::{DriverStatus, EngineStatus};
use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{button, column, container, row, rule, space, stack, text};
use iced::{Bottom, Center, Color, Element, Fill, Padding};

use crate::app::{Connection, Message, Notice};
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::theme::{
    CELADON, CORPS_INTERFACE, CORPS_META, ESPACE_L, ESPACE_S, ESPACE_XL, ESPACE_XS, FILET, GARANCE,
    OR,
};
use crate::{format, style, typo};

// --- Mesures de la maquette -------------------------------------------------

/// Largeur de la barre latérale.
pub const LARGEUR_BARRE: f32 = 208.0;
/// Corps du display : la marque et le titre de la vue.
const CORPS_DISPLAY: f32 = 28.0;
/// Corps du surtitre de la marque, en petites capitales.
const CORPS_SURTITRE: f32 = 11.0;
/// Corps du libellé d'un onglet, en petites capitales.
const CORPS_ONGLET: f32 = 12.0;
/// Interligne serré du display : un titre n'a pas besoin des 1,6 du texte.
const INTERLIGNE_SERRE: f32 = 1.1;
/// Diamètre d'une pastille d'état.
const PASTILLE: f32 = 8.0;
/// Padding vertical d'un onglet.
const PADDING_ONGLET: f32 = 10.0;
/// Padding de la barre latérale : 28 en haut, 24 sur les côtés, 20 en bas.
const PADDING_BARRE: Padding = Padding {
    top: 28.0,
    right: 24.0,
    bottom: 20.0,
    left: 24.0,
};
/// Padding du contenu : 28 en haut, 32 sur les côtés, 24 en bas.
const PADDING_CONTENU: Padding = Padding {
    top: 28.0,
    right: 32.0,
    bottom: 24.0,
    left: 32.0,
};
/// Padding du bandeau de notice : 10 en hauteur, 14 en largeur.
const PADDING_BANDEAU: Padding = Padding {
    top: 10.0,
    right: 14.0,
    bottom: 10.0,
    left: 14.0,
};
/// Écart entre le filet d'or de la marque et ce qui l'entoure.
const ECART_FILET_MARQUE: f32 = 20.0;
/// Écart entre le titre de la vue et son sous-titre.
const ECART_SOUS_TITRE: f32 = 6.0;
/// Écart entre l'en-tête et son filet d'or.
const ECART_ENTETE: f32 = 18.0;
/// Écart entre le filet d'or de l'en-tête et le contenu de la vue.
const ECART_CONTENU: f32 = 20.0;
/// Décalage vertical de la pastille pour qu'elle s'aligne sur la première
/// ligne de son libellé, et non sur le milieu d'un libellé qui se replie.
const RETRAIT_PASTILLE: f32 = 4.0;

// --- Onglets ----------------------------------------------------------------

/// Onglet affiché dans la fenêtre (SPEC §5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Tab {
    /// Liste des câbles virtuels.
    #[default]
    Cables,
    /// Graphe de routage.
    Patchbay,
    /// Xruns, latence, pilote.
    Diagnostic,
}

impl Tab {
    /// Les onglets, dans l'ordre d'affichage.
    pub const ALL: [Tab; 3] = [Tab::Cables, Tab::Patchbay, Tab::Diagnostic];

    /// Libellé de l'onglet, qui est aussi le titre de sa vue.
    pub fn label(self) -> &'static str {
        i18n::t(match self {
            Tab::Cables => Text::TabCables,
            Tab::Patchbay => Text::TabPatchbay,
            Tab::Diagnostic => Text::TabDiagnostic,
        })
    }
}

// --- Décisions --------------------------------------------------------------

/// État du démon tel que le pied de la barre latérale le montre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtatDemon {
    /// Le démon répond.
    EnMarche,
    /// La connexion est en cours d'établissement.
    Demarrage,
    /// Le démon ne répond pas.
    Arrete,
}

impl EtatDemon {
    /// L'état correspondant à la connexion.
    pub fn depuis(connexion: &Connection) -> Self {
        match connexion {
            Connection::Ready { .. } => EtatDemon::EnMarche,
            Connection::Starting | Connection::Connecting => EtatDemon::Demarrage,
            Connection::Lost { .. } => EtatDemon::Arrete,
        }
    }

    /// Couleur de la pastille : céladon en marche, or au démarrage, garance à
    /// l'arrêt.
    ///
    /// Ces trois matières sont invariantes entre les deux modes : la pastille
    /// est un aplat, jamais du texte.
    pub fn couleur(self) -> Color {
        match self {
            EtatDemon::EnMarche => CELADON,
            EtatDemon::Demarrage => OR,
            EtatDemon::Arrete => GARANCE,
        }
    }

    /// Libellé affiché à côté de la pastille.
    pub fn libelle(self) -> &'static str {
        i18n::t(match self {
            EtatDemon::EnMarche => Text::DaemonRunning,
            EtatDemon::Demarrage => Text::DaemonStarting,
            EtatDemon::Arrete => Text::DaemonStopped,
        })
    }
}

/// Le pilote de la plateforme pour laquelle ce binaire est compilé.
///
/// Windows a un pilote noyau, macOS un plugin HAL, Linux ni l'un ni l'autre :
/// les câbles y sont des nœuds PipeWire.
pub fn pilote_plateforme() -> &'static str {
    i18n::t(if cfg!(target_os = "windows") {
        Text::DriverWindows
    } else if cfg!(target_os = "macos") {
        Text::DriverMacos
    } else {
        Text::DriverLinux
    })
}

/// Le libellé du bouton qui démarre le démon (F-51).
///
/// Tant que le démarrage est demandé, le bouton dit « Démarrage… » et ne
/// répond plus : le lancement est parti, il n'y a rien à redemander.
pub fn libelle_de_demarrage(demande: bool) -> Text {
    if demande {
        Text::DaemonStartPending
    } else {
        Text::DaemonStart
    }
}

/// Libellé du nœud pilote du graphe.
pub fn libelle_pilote(pilote: &DriverStatus) -> String {
    match pilote {
        DriverStatus::None => i18n::t(Text::DriverNone).to_string(),
        DriverStatus::Internal => i18n::t(Text::DriverInternal).to_string(),
        DriverStatus::Device { id } => id.to_string(),
    }
}

/// Durée de marche du moteur, en secondes : les trames jouées divisées par la
/// fréquence d'échantillonnage.
fn secondes_de_marche(status: &EngineStatus) -> u64 {
    let hz = u64::from(status.sample_rate.hz());
    if hz == 0 {
        return 0;
    }
    status.position / hz
}

/// Sous-titre de la vue, sous son titre.
///
/// Tant que le miroir n'est pas chargé, aucune des trois vues n'a de chiffre à
/// annoncer : le sous-titre dit l'attente.
pub fn sous_titre(onglet: Tab, mirror: &Mirror) -> String {
    let Some(status) = &mirror.status else {
        return i18n::t(Text::Waiting).to_string();
    };
    match onglet {
        Tab::Cables => i18n::cables_subtitle(mirror.cables.len()),
        Tab::Patchbay => i18n::patchbay_subtitle(
            &libelle_pilote(&status.driver),
            &format::kilohertz(status.sample_rate.hz()),
            &format::entier(status.quantum.get() as u64),
        ),
        Tab::Diagnostic => i18n::diagnostic_subtitle(
            &format::duree_de_marche(secondes_de_marche(status)),
            &format::xruns(mirror.xruns),
        ),
    }
}

/// Les deux nombres du pied de la barre latérale : le compte de xruns et la
/// latence estimée.
///
/// Tant que le miroir n'est pas chargé, les deux valeurs sont des tirets
/// cadratins — la place reste prise, la ligne ne saute pas au chargement.
pub fn chiffres(mirror: &Mirror) -> (String, String) {
    let inconnu = i18n::t(Text::Inconnu);
    match &mirror.status {
        Some(status) => (
            format::xruns(mirror.xruns),
            format::latence_estimee(status.quantum.get() as u32, status.sample_rate.hz()),
        ),
        None => (
            format!("{inconnu}{}{}", format::INSECABLE, i18n::t(Text::Xrun)),
            format!("{inconnu}{}{}", format::INSECABLE, i18n::t(Text::UnitMs)),
        ),
    }
}

// --- Composition ------------------------------------------------------------

/// La fenêtre entière : barre latérale, filet de séparation, contenu.
///
/// `graisse` est la graisse du texte courant du mode
/// ([`crate::theme::Jetons::graisse_texte`]) : `iced` fige la police au moment
/// de la composition, alors que les couleurs, elles, sont choisies au rendu.
///
/// `actions` sont les actions de l'en-tête, dans l'ordre de lecture ; une vue
/// peut n'en avoir aucune, ou en montrer une qui dépend de sa sélection.
pub(crate) fn fenetre<'a>(
    onglet: Tab,
    connexion: &Connection,
    mirror: &Mirror,
    notice: Option<&'a Notice>,
    graisse: Weight,
    actions: Vec<Element<'a, Message>>,
    contenu: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut page = column![
        entete(onglet, mirror, graisse, actions),
        space::vertical().height(ECART_ENTETE),
        rule::horizontal(FILET).style(style::filet_or),
        space::vertical().height(ECART_CONTENU),
    ]
    .width(Fill)
    .height(Fill);
    if let Some(notice) = notice {
        page = page.push(bandeau(notice, graisse));
        page = page.push(space::vertical().height(ESPACE_L));
    }
    page = page.push(contenu);

    // La rangée porte la hauteur : sans elle, la barre latérale et son filet
    // de séparation se réduiraient à la hauteur de leur contenu.
    container(
        row![
            barre_laterale(onglet, connexion, mirror),
            rule::vertical(FILET).style(style::filet),
            container(page)
                .padding(PADDING_CONTENU)
                .width(Fill)
                .height(Fill),
        ]
        .width(Fill)
        .height(Fill),
    )
    .style(style::surface)
    .width(Fill)
    .height(Fill)
    .into()
}

/// La barre latérale : marque, navigation, pied d'état.
fn barre_laterale<'a>(
    onglet: Tab,
    connexion: &Connection,
    mirror: &Mirror,
) -> Element<'a, Message> {
    let marque = column![
        text(i18n::t(Text::AppTitle))
            .size(CORPS_DISPLAY)
            .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
            .font(typo::display(CORPS_DISPLAY))
            .style(style::texte_en(|j| j.titre)),
        container(typo::petites_capitales(
            i18n::t(Text::BrandTagline),
            CORPS_SURTITRE
        ))
        .style(style::encre_de(|j| j.texte_2)),
    ]
    .spacing(ESPACE_S);

    container(
        column![
            marque,
            space::vertical().height(ECART_FILET_MARQUE),
            rule::horizontal(FILET).style(style::filet_or),
            space::vertical().height(ECART_FILET_MARQUE),
            navigation(onglet),
            space::vertical(),
            pied(connexion, mirror),
        ]
        // L'espace élastique qui repousse le pied ne s'étire que dans une
        // colonne de hauteur imposée.
        .width(Fill)
        .height(Fill),
    )
    .padding(PADDING_BARRE)
    .width(LARGEUR_BARRE)
    .height(Fill)
    .into()
}

/// La navigation verticale : un bouton par onglet, l'actif souligné d'or.
fn navigation<'a>(actif: Tab) -> Element<'a, Message> {
    let mut liste = column![].spacing(ESPACE_XS).width(Fill);
    for onglet in Tab::ALL {
        let courant = onglet == actif;
        // La colonne est en largeur intrinsèque : le filet, lui, remplit —
        // il prend donc exactement la largeur du libellé.
        let mut libelle = column![typo::petites_capitales(onglet.label(), CORPS_ONGLET)];
        if courant {
            libelle = libelle.push(rule::horizontal(FILET).style(style::filet_or));
        }
        liste = liste.push(
            button(libelle.spacing(ESPACE_XS))
                .on_press(Message::Tab(onglet))
                .padding(Padding::new(0.0).vertical(PADDING_ONGLET))
                .width(Fill)
                .style(style::onglet(courant)),
        );
    }
    liste.into()
}

/// Le pied de la barre latérale : démon, pilote de plateforme, chiffres.
fn pied<'a>(connexion: &Connection, mirror: &Mirror) -> Element<'a, Message> {
    let etat = EtatDemon::depuis(connexion);
    let (xruns, latence) = chiffres(mirror);
    column![
        ligne_pastille(etat.couleur(), etat.libelle().to_string()),
        ligne_pastille(CELADON, pilote_plateforme().to_string()),
        // Une ligne libre, pas une colonne de tableau : rien n'a à s'aligner
        // dessous, donc pas de largeur fixée (voir `docs/design-system.md`
        // § 6.2, qui ne vise que les colonnes).
        row![
            meta(xruns),
            meta(i18n::t(Text::Separateur).to_string()),
            meta(latence),
        ]
        .spacing(ESPACE_S)
        .align_y(Center),
    ]
    .spacing(ESPACE_S)
    .width(Fill)
    .into()
}

/// Une métadonnée du pied : Inter, corps 12, texte secondaire.
fn meta<'a>(contenu: String) -> iced::widget::Text<'a> {
    text(contenu)
        .size(CORPS_META)
        .font(typo::interface())
        .style(style::texte_en(|j| j.texte_2))
}

/// Une pastille de 8 px suivie de son libellé.
fn ligne_pastille<'a>(couleur: Color, libelle: String) -> Element<'a, Message> {
    row![
        container(
            container(space::horizontal())
                .width(PASTILLE)
                .height(PASTILLE)
                .style(style::pastille(couleur))
        )
        .padding(iced::Padding::ZERO.top(RETRAIT_PASTILLE)),
        meta(libelle),
    ]
    .spacing(ESPACE_S)
    .align_y(iced::Top)
    .into()
}

/// L'en-tête du contenu : titre et sous-titre à gauche, actions de la vue
/// alignées en bas à droite.
fn entete<'a>(
    onglet: Tab,
    mirror: &Mirror,
    graisse: Weight,
    actions: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let titres = column![
        text(onglet.label())
            .size(CORPS_DISPLAY)
            .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
            .font(typo::display(CORPS_DISPLAY))
            .style(style::texte_en(|j| j.titre)),
        space::vertical().height(ECART_SOUS_TITRE),
        text(sous_titre(onglet, mirror))
            .size(CORPS_INTERFACE)
            .font(typo::texte_a(graisse, CORPS_INTERFACE))
            .style(style::texte_en(|j| j.texte_2)),
    ]
    .width(Fill);
    let mut ligne = row![titres].spacing(ESPACE_XL).width(Fill).align_y(Bottom);
    for action in actions {
        ligne = ligne.push(action);
    }
    ligne.into()
}

/// Le bandeau de notice : surface posée, filet de contour pour une
/// information, filet garance pour une erreur.
///
/// Le bandeau n'a pas de bouton de fermeture : il s'efface tout seul au bout
/// de [`crate::app::DUREE_NOTICE`].
fn bandeau<'a>(notice: &'a Notice, graisse: Weight) -> Element<'a, Message> {
    let habillage = if notice.erreur {
        style::notice_erreur
    } else {
        style::carte
    };
    container(
        text(notice.texte.as_str())
            .size(CORPS_INTERFACE)
            .font(typo::texte_a(graisse, CORPS_INTERFACE))
            .style(style::texte_en(|j| j.texte))
            .width(Fill),
    )
    .padding(PADDING_BANDEAU)
    .width(Fill)
    .style(habillage)
    .into()
}

/// L'action principale d'une vue, en bouton primaire ; `message` absent la
/// grise.
pub(crate) fn action_primaire<'a>(libelle: Text, message: Option<Message>) -> Element<'a, Message> {
    button(typo::petites_capitales(
        i18n::t(libelle),
        style::CORPS_BOUTON,
    ))
    .on_press_maybe(message)
    .height(style::HAUTEUR_BOUTON)
    .padding(Padding::new(0.0).horizontal(style::PADDING_BOUTON))
    .style(style::bouton_primaire)
    .into()
}

/// Le bouton qui démarre le démon, en bouton primaire.
///
/// Il ne s'affiche que lorsque le démon ne répond pas : c'est alors la seule
/// action de la fenêtre qui puisse encore aboutir.
pub(crate) fn action_demarrer<'a>(demande: bool) -> Element<'a, Message> {
    action_primaire(
        libelle_de_demarrage(demande),
        (!demande).then_some(Message::StartDaemon),
    )
}

/// Une action d'appoint d'une vue, en bouton secondaire ; `message` absent la
/// grise.
pub(crate) fn action_secondaire<'a>(
    libelle: Text,
    message: Option<Message>,
) -> Element<'a, Message> {
    button(typo::petites_capitales(
        i18n::t(libelle),
        style::CORPS_BOUTON,
    ))
    .on_press_maybe(message)
    .height(style::HAUTEUR_BOUTON)
    .padding(Padding::new(0.0).horizontal(style::PADDING_BOUTON))
    .style(style::bouton_secondaire)
    .into()
}

/// Une action destructrice d'une vue, en lien souligné ; `message` absent la
/// grise.
///
/// `iced` ne sait pas souligner un texte : le filet d'accent se compose sous
/// le libellé (voir [`crate::style::lien`]). Le tout est centré dans la
/// hauteur d'un bouton pour s'aligner sur les autres actions de l'en-tête.
pub(crate) fn action_lien<'a>(libelle: Text, message: Option<Message>) -> Element<'a, Message> {
    // Le filet est posé dans une couche **au-dessus** du libellé : un `rule`
    // remplit son parent, et un `Stack` prend la taille de sa couche de base.
    // C'est donc le libellé qui décide de la longueur du soulignement, et non
    // la place restée libre dans l'en-tête.
    let souligne = stack![
        column![
            typo::petites_capitales(i18n::t(libelle), style::CORPS_BOUTON),
            space::vertical().height(ESPACE_XS + FILET),
        ],
        column![
            space::vertical(),
            rule::horizontal(FILET).style(style::filet_accent),
        ]
        .height(Fill),
    ];
    container(
        button(souligne)
            .on_press_maybe(message)
            .padding(0)
            .style(style::lien),
    )
    .height(style::HAUTEUR_BOUTON)
    .align_y(Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use crate::model::fixtures::{loaded, status};
    use crate::theme::{CLAIR, SOMBRE};

    /// Une connexion perdue, pour les parcours d'état.
    fn perdue() -> Connection {
        Connection::Lost {
            reason: "connexion perdue".into(),
            retry_in: Duration::from_secs(2),
        }
    }

    #[test]
    fn the_three_tabs_are_labelled_in_french() {
        assert_eq!(Tab::default(), Tab::Cables);
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["Câbles", "Patchbay", "Diagnostic"]
        );
    }

    /// Chaque état de la connexion a sa pastille et son libellé.
    #[test]
    fn la_pastille_dit_l_etat_du_demon() {
        let cas = [
            (Connection::Starting, EtatDemon::Demarrage, OR),
            (Connection::Connecting, EtatDemon::Demarrage, OR),
            (
                Connection::Ready {
                    server: "conduitd 0.1.0".into(),
                },
                EtatDemon::EnMarche,
                CELADON,
            ),
            (perdue(), EtatDemon::Arrete, GARANCE),
        ];
        for (connexion, attendu, couleur) in cas {
            let etat = EtatDemon::depuis(&connexion);
            assert_eq!(etat, attendu, "connexion {connexion:?}");
            assert_eq!(etat.couleur(), couleur);
        }
        assert_eq!(EtatDemon::EnMarche.libelle(), "conduitd en marche");
        assert_eq!(EtatDemon::Demarrage.libelle(), "conduitd démarre…");
        assert_eq!(EtatDemon::Arrete.libelle(), "conduitd arrêté");
        // Les trois matières de pastille sont invariantes entre les modes.
        for etat in [EtatDemon::EnMarche, EtatDemon::Demarrage, EtatDemon::Arrete] {
            assert!([CLAIR.celadon, CLAIR.or, CLAIR.garance].contains(&etat.couleur()));
            assert!([SOMBRE.celadon, SOMBRE.or, SOMBRE.garance].contains(&etat.couleur()));
        }
    }

    /// Le pilote annoncé est celui de la plateforme compilée.
    #[test]
    fn le_pilote_est_celui_de_la_plateforme() {
        let attendu = if cfg!(target_os = "windows") {
            "Pilote noyau conduit-kmd"
        } else if cfg!(target_os = "macos") {
            "Plugin HAL conduit-hal"
        } else {
            "Nœuds PipeWire (sans pilote)"
        };
        assert_eq!(pilote_plateforme(), attendu);
    }

    /// Miroir chargé : chaque vue annonce ses chiffres.
    #[test]
    fn les_sous_titres_disent_l_etat_charge() {
        let m = loaded();
        assert_eq!(
            sous_titre(Tab::Cables, &m),
            "2 câbles · visibles par toutes les applications"
        );
        assert_eq!(
            sous_titre(Tab::Patchbay, &m),
            "Pilote de graphe : horloge interne · 48\u{a0}kHz · quantum 256"
        );
        // `position` vaut 0 dans la fixture : le moteur vient de démarrer.
        assert_eq!(
            sous_titre(Tab::Diagnostic, &m),
            "Moteur en marche depuis 0\u{a0}s · 3\u{a0}xruns"
        );
    }

    /// La durée de marche se lit dans les trames jouées.
    #[test]
    fn la_duree_de_marche_vient_des_trames_jouees() {
        let mut s = status();
        s.position = 48_000 * 90;
        assert_eq!(secondes_de_marche(&s), 90);
        let mut m = loaded();
        m.status = Some(s);
        assert_eq!(
            sous_titre(Tab::Diagnostic, &m),
            "Moteur en marche depuis 1\u{a0}min · 3\u{a0}xruns"
        );
    }

    /// Miroir vide : les trois vues disent l'attente.
    #[test]
    fn les_sous_titres_disent_l_attente_sans_miroir() {
        let vide = Mirror::default();
        for onglet in Tab::ALL {
            assert_eq!(sous_titre(onglet, &vide), "En attente du démon…");
        }
    }

    /// Les chiffres du pied, avec et sans statut.
    #[test]
    fn les_chiffres_du_pied_tombent_sur_des_tirets_sans_statut() {
        let (xruns, latence) = chiffres(&loaded());
        assert_eq!(xruns, "3\u{a0}xruns");
        assert_eq!(latence, "10,7\u{a0}ms");
        let (xruns, latence) = chiffres(&Mirror::default());
        assert_eq!(xruns, "—\u{a0}xrun");
        assert_eq!(latence, "—\u{a0}ms");
    }

    /// Le libellé du nœud pilote couvre les trois cas du protocole.
    #[test]
    fn le_libelle_du_pilote_couvre_les_trois_cas() {
        assert_eq!(libelle_pilote(&DriverStatus::None), "aucun");
        assert_eq!(libelle_pilote(&DriverStatus::Internal), "horloge interne");
        assert_eq!(
            libelle_pilote(&DriverStatus::Device { id: "hp".into() }),
            "hp"
        );
    }
}
