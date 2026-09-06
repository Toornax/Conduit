//! Les deux écrans d'état : « démon absent » et premier lancement (M2-13).
//!
//! Ce sont les deux moments où la fenêtre n'a rien à montrer de son contenu
//! habituel : le démon n'a jamais répondu, ou l'utilisateur vient d'installer
//! Conduit. Ils **remplacent la vue et son en-tête** ; la barre latérale, elle,
//! reste — avec sa pastille, qui dit l'état du démon.
//!
//! Deux couches, comme partout dans ce crate :
//!
//! - les fonctions **qui décident** — [`ecran`], [`consequence_de_l_arret`] —
//!   sont pures et testées sans fenêtre ni moteur de rendu ;
//! - les fonctions **qui composent** rendent des `Element` et ne sont pas
//!   testables autrement qu'à l'œil.
//!
//! # « Démon absent » ne s'affiche pas au moindre hoquet
//!
//! L'écran attend deux conditions : la connexion est perdue **et** le miroir
//! n'a jamais été chargé. Une session déjà établie qui perd le démon garde donc
//! ce qu'elle affichait — des câbles et un graphe qui existent toujours —, avec
//! ses actions grisées et le bouton de démarrage dans l'en-tête. Remplacer
//! l'écran à chaque reconnexion serait une secousse pour rien.
//!
//! # Ce que l'arrêt du démon change n'est pas le même partout
//!
//! Sous Windows et sous macOS, les câbles sont tenus par un pilote (pilote
//! noyau, plugin HAL) : ils continuent de fonctionner en boucle locale sans le
//! démon (F-05). Sous Linux, ce sont des nœuds PipeWire que le démon crée
//! lui-même : ils **disparaissent** avec lui, et SPEC §5.3 le dit — « F-05
//! n'est pas garanti sur Linux ». La maquette est écrite pour Windows ; sa
//! promesse serait fausse ailleurs, donc le paragraphe suit la plateforme
//! compilée (voir [`consequence_de_l_arret`]).

use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{column, container, row, rule, space, text};
use iced::{Center, Element, Fill, Padding, Top};

use crate::app::{Connection, Message};
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::shell::{self, EtatDemon, Tab};
use crate::theme::{
    CELADON, CORPS_DISPLAY, CORPS_INTERFACE, CORPS_TEXTE, ESPACE_L, ESPACE_S, FILET,
};
use crate::{format, style, typo};

// --- Mesures de la maquette -------------------------------------------------

/// Corps du surtitre, en petites capitales.
const CORPS_SURTITRE: f32 = 11.0;
/// Interligne serré du titre : un display de 40 px n'a pas besoin des 1,6 du
/// texte courant.
const INTERLIGNE_SERRE: f32 = 1.1;
/// Largeur du filet d'or posé sous le titre.
const FILET_OR: f32 = 96.0;
/// Largeur maximale de l'écran « démon absent ».
const LARGEUR_ABSENT: f32 = 520.0;
/// Écart entre deux blocs de l'écran « démon absent ».
const ECART_ABSENT: f32 = ESPACE_L;
/// Largeur maximale de la carte d'accueil.
const LARGEUR_ACCUEIL: f32 = 560.0;
/// Écart entre deux blocs de la carte d'accueil. Il n'est pas sur l'échelle
/// d'espacement : la carte respire un cran de plus que l'écran nu.
const ECART_ACCUEIL: f32 = 18.0;
/// Padding de la carte d'accueil : 40 en hauteur, 44 en largeur.
const PADDING_ACCUEIL: Padding = Padding {
    top: 40.0,
    right: 44.0,
    bottom: 40.0,
    left: 44.0,
};
/// Diamètre d'une pastille de ligne de contrôle.
const PASTILLE: f32 = 8.0;
/// Décalage vertical de la pastille, pour qu'elle s'aligne sur la première
/// ligne de son libellé.
const RETRAIT_PASTILLE: f32 = 5.0;
/// Écart entre les deux actions d'un écran.
const ECART_ACTIONS: f32 = ESPACE_L;

// --- Décisions --------------------------------------------------------------

/// Ce que la fenêtre montre à la place de la vue de l'onglet, s'il y a lieu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecran {
    /// La vue de l'onglet courant, comme d'habitude.
    Vue,
    /// Le démon ne répond pas, et n'a jamais répondu.
    DemonAbsent,
    /// Premier lancement : l'accueil, une seule fois.
    Accueil,
}

/// L'écran à montrer.
///
/// `accueil_vu` est ce que les préférences disent du premier lancement, et
/// `None` tant qu'elles n'ont rien dit : leur lecture est une tâche, et
/// accueillir quelqu'un avant de savoir qu'il l'a déjà été serait un
/// clignotement.
///
/// L'accueil exige en outre un miroir chargé : souhaiter la bienvenue devant
/// un écran vide, sans le compte de câbles ni les réglages du moteur, n'aurait
/// aucun sens.
pub fn ecran(connexion: &Connection, mirror: &Mirror, accueil_vu: Option<bool>) -> Ecran {
    if !mirror.is_loaded() {
        // Tant que la connexion se cherche, rien ne dit encore que le démon
        // manque : la vue montre son attente.
        return match EtatDemon::depuis(connexion) {
            EtatDemon::Arrete => Ecran::DemonAbsent,
            EtatDemon::EnMarche | EtatDemon::Demarrage => Ecran::Vue,
        };
    }
    match accueil_vu {
        Some(false) => Ecran::Accueil,
        Some(true) | None => Ecran::Vue,
    }
}

/// Ce que l'arrêt du démon change pour les câbles, sur cette plateforme.
///
/// Windows et macOS ont un pilote qui tient les câbles : ils survivent au
/// démon (F-05). Linux n'en a pas : les câbles sont des nœuds PipeWire que le
/// démon crée, et ils partent avec lui (SPEC §5.3). Promettre la boucle locale
/// partout serait promettre ce que Linux ne tient pas.
pub fn consequence_de_l_arret() -> &'static str {
    i18n::t(
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            Text::DaemonGoneDriver
        } else {
            Text::DaemonGoneLinux
        },
    )
}

// --- Composition ------------------------------------------------------------

/// L'écran « démon absent » : ce qui marche encore, ce qui ne marche plus, et
/// les deux gestes qui restent.
///
/// « Ouvrir le journal » **affiche le chemin** du journal dans une notice ; il
/// n'ouvre pas l'explorateur de fichiers du système, qui demanderait une
/// dépendance de plus (`opener`, `open`) pour un geste que l'utilisateur fait
/// très bien lui-même. Ce n'est pas le moment d'en ajouter une.
pub(crate) fn demon_absent<'a>(graisse: Weight, demarrage: bool) -> Element<'a, Message> {
    let contenu = column![
        // La maquette veut le surtitre en garance ; elle est dessinée en mode
        // clair, où la garance est bien l'accent lisible. En mode sombre elle
        // ne passe que 2,0:1 sur l'encre : c'est l'or qui prend le relais.
        // `accent_texte` est exactement cette distribution-là.
        surtitre(Text::DaemonGone, |j| j.accent_texte),
        titre(Text::DaemonGoneTitle),
        filet_d_or(),
        paragraphe(consequence_de_l_arret().to_string(), graisse),
        row![
            shell::action_demarrer(demarrage),
            shell::action_lien(Text::OpenLog, Some(Message::Journal)),
        ]
        .spacing(ECART_ACTIONS)
        .align_y(Center),
    ]
    .spacing(ECART_ABSENT);
    container(contenu).max_width(LARGEUR_ABSENT).into()
}

/// L'écran de premier lancement : ce que l'installation a fait, et par où
/// commencer.
///
/// Le compte de câbles est celui du **miroir**, et la phrase s'accorde
/// dessus : le démon en annonce deux par défaut, mais rien n'oblige à le
/// croire sur parole.
pub(crate) fn accueil<'a>(mirror: &Mirror, graisse: Weight) -> Element<'a, Message> {
    let cables = mirror.cables.len();
    let mut contenu = column![
        surtitre(Text::WelcomeOver, |j| j.texte_2_carte),
        titre(Text::WelcomeTitle),
        filet_d_or(),
        paragraphe(i18n::accueil_cables(cables), graisse),
        ligne_de_controle(shell::pilote_plateforme().to_string(), graisse),
        ligne_de_controle(EtatDemon::EnMarche.libelle().to_string(), graisse),
    ]
    .spacing(ECART_ACCUEIL);
    if let Some(status) = &mirror.status {
        contenu = contenu.push(ligne_de_controle(
            i18n::accueil_reglages(
                cables,
                &format::kilohertz(status.sample_rate.hz()),
                &format::entier(status.quantum.get() as u64),
            ),
            graisse,
        ));
    }
    contenu = contenu.push(
        row![
            shell::action_primaire(Text::WelcomeStart, Some(Message::AccueilVu(Tab::Cables))),
            shell::action_lien(
                Text::WelcomePatchbay,
                Some(Message::AccueilVu(Tab::Patchbay))
            ),
        ]
        .spacing(ECART_ACTIONS)
        .align_y(Center),
    );
    container(contenu)
        .padding(PADDING_ACCUEIL)
        .max_width(LARGEUR_ACCUEIL)
        .style(style::carte)
        .into()
}

/// Le surtitre d'un écran : petites capitales, corps 11, dans la couleur
/// donnée.
fn surtitre<'a>(
    libelle: Text,
    couleur: fn(&crate::theme::Jetons) -> iced::Color,
) -> Element<'a, Message> {
    container(typo::petites_capitales(i18n::t(libelle), CORPS_SURTITRE))
        .style(style::encre_de(couleur))
        .into()
}

/// Le titre d'un écran : display 40, couleur de titre.
fn titre<'a>(libelle: Text) -> Element<'a, Message> {
    text(i18n::t(libelle))
        .size(CORPS_DISPLAY)
        .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
        .font(typo::display(CORPS_DISPLAY))
        .style(style::texte_en(|j| j.titre))
        .into()
}

/// Le filet d'or posé sous le titre, court et à gauche.
fn filet_d_or<'a>() -> Element<'a, Message> {
    container(rule::horizontal(FILET).style(style::filet_or))
        .width(FILET_OR)
        .into()
}

/// Le paragraphe d'un écran : texte courant, corps 17.
fn paragraphe<'a>(contenu: String, graisse: Weight) -> Element<'a, Message> {
    text(contenu)
        .size(CORPS_TEXTE)
        .line_height(LineHeight::Relative(crate::theme::INTERLIGNE))
        .font(typo::texte_a(graisse, CORPS_TEXTE))
        .style(style::texte_en(|j| j.texte))
        .width(Fill)
        .into()
}

/// Une ligne de contrôle de l'accueil : une pastille céladon et son libellé.
///
/// Le libellé prend le texte secondaire **des surfaces posées** : l'accueil est
/// une carte, et le texte secondaire du fond n'y passerait que 4,3:1 en mode
/// clair.
fn ligne_de_controle<'a>(libelle: String, graisse: Weight) -> Element<'a, Message> {
    row![
        container(
            container(space::horizontal())
                .width(PASTILLE)
                .height(PASTILLE)
                .style(style::pastille(CELADON))
        )
        .padding(Padding::ZERO.top(RETRAIT_PASTILLE)),
        text(libelle)
            .size(CORPS_INTERFACE)
            // Inter, comme les mêmes lignes au pied de la barre latérale : ce
            // sont des états, pas de la prose.
            .font(typo::interface_graisse(graisse))
            .style(style::texte_en(|j| j.texte_2_carte)),
    ]
    .spacing(ESPACE_S)
    .align_y(Top)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use crate::model::fixtures::loaded;

    /// Une connexion perdue.
    fn perdue() -> Connection {
        Connection::Lost {
            reason: "connexion perdue".into(),
            retry_in: Duration::from_secs(2),
        }
    }

    /// L'écran « démon absent » ne s'affiche que si le démon n'a jamais
    /// répondu : une session établie qui perd la connexion garde sa vue.
    #[test]
    fn l_ecran_du_demon_absent_attend_de_n_avoir_jamais_rien_eu() {
        let vide = Mirror::default();
        assert_eq!(ecran(&perdue(), &vide, Some(true)), Ecran::DemonAbsent);
        // Tant que la connexion se cherche, rien ne dit que le démon manque.
        for connexion in [Connection::Starting, Connection::Connecting] {
            assert_eq!(ecran(&connexion, &vide, Some(true)), Ecran::Vue);
        }
        // Miroir chargé : la vue reste, avec ce qu'elle montrait.
        assert_eq!(ecran(&perdue(), &loaded(), Some(true)), Ecran::Vue);
    }

    /// L'accueil demande un miroir chargé **et** des préférences qui ont parlé.
    #[test]
    fn l_accueil_attend_le_miroir_et_les_preferences() {
        let charge = loaded();
        assert_eq!(
            ecran(&Connection::Starting, &charge, Some(false)),
            Ecran::Accueil
        );
        assert_eq!(
            ecran(&Connection::Starting, &charge, Some(true)),
            Ecran::Vue
        );
        assert_eq!(
            ecran(&Connection::Starting, &charge, None),
            Ecran::Vue,
            "sans préférences lues, on n'accueille pas au hasard"
        );
        // Sans miroir, l'accueil n'aurait aucun chiffre à montrer.
        assert_eq!(
            ecran(&Connection::Connecting, &Mirror::default(), Some(false)),
            Ecran::Vue
        );
        assert_eq!(
            ecran(&perdue(), &Mirror::default(), Some(false)),
            Ecran::DemonAbsent
        );
    }

    /// Le paragraphe suit la plateforme : la boucle locale n'est promise que
    /// là où un pilote la tient.
    #[test]
    fn la_consequence_de_l_arret_suit_la_plateforme() {
        let texte = consequence_de_l_arret();
        assert!(!texte.is_empty());
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            assert_eq!(texte, i18n::t(Text::DaemonGoneDriver));
            assert!(texte.contains("continuent"), "{texte}");
        } else {
            assert_eq!(texte, i18n::t(Text::DaemonGoneLinux));
            assert!(texte.contains("disparaissent"), "{texte}");
            assert!(
                !texte.contains("boucle locale"),
                "ne promets pas sous Linux ce que F-05 n'y garantit pas : {texte}"
            );
        }
    }
}
