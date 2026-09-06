//! Vue « Patchbay » : le graphe du démon en cartes déplaçables et en liens
//! (M2-04, M2-05, M2-06).
//!
//! Un nœud du graphe est une **carte** : son libellé et une étiquette d'état
//! en en-tête, puis ses ports — les entrées à gauche, les sorties à droite,
//! chacun marqué d'une pastille d'or posée à cheval sur le bord de la carte —,
//! et enfin son **pied** : la glissière de gain, le libellé en décibels et le
//! bouton de coupure (F-13). Un lien est une **courbe d'or** tirée d'une
//! pastille de sortie à une pastille d'entrée.
//!
//! # Une scène hybride : un canevas dessous, de vraies cartes dessus
//!
//! La scène est un [`stack`] : un [`canvas()`] occupe toute la zone, et chaque
//! carte est un widget épinglé au-dessus de lui.
//!
//! Le canevas seul saurait tout dessiner — c'est ainsi que se font
//! habituellement les patchbays —, mais il ne sait rien faire d'autre : pas de
//! focus clavier, pas de bouton, pas d'infobulle, et un rendu de texte qui
//! n'est pas celui du reste de la fenêtre. Les cartes sont donc de vrais
//! widgets, avec les vraies polices, le vrai thème et la vraie glissière de
//! gain du pied.
//!
//! Le canevas, lui, garde ce que les widgets ne savent pas faire : dessiner
//! des courbes entre deux points quelconques et suivre le curseur pendant un
//! déplacement. Il le peut parce que [`Stack`] distribue chaque
//! événement de haut en bas et ne s'arrête qu'à la première couche qui le
//! **capture** : le [`mouse_area`] de l'en-tête capture la pression, mais
//! laisse passer les mouvements et le relâchement, que le canevas voit donc
//! même quand le curseur a quitté la carte.
//!
//! Le prix de ce partage est que la géométrie des points d'attache est
//! **calculée** ([`hauteur_carte`], [`ancre`]) et non mesurée : le canevas ne
//! connaît pas la disposition des widgets qui le couvrent. Un test vérifie que
//! [`hauteur_carte`] rend bien la hauteur que la composition produit.
//!
//! [`Stack`]: iced::widget::Stack
//!
//! # Tirer un lien : la cible est celle que le widget dit
//!
//! Une pastille est entourée d'un [`mouse_area`] : la pression sur une sortie
//! ouvre un [`Tirage`], l'entrée et la sortie du curseur sur une entrée
//! nomment la cible. La cible n'est donc **jamais** devinée par un test
//! d'intersection contre la géométrie recalculée : ce sont les bornes réelles
//! du widget qui tranchent, et elles ne peuvent pas mentir.
//!
//! Pendant le tirage, la position du curseur est retenue dans l'état du
//! canevas ([`Suivi`]) et non dans celui de l'application : le canevas se
//! redessine seul ([`canvas::Action::request_redraw`]) sans qu'un message
//! reconstruise la vue à chaque pixel.
//!
//! # Ce que la vue décide, et ce qu'elle ne décide pas
//!
//! Aucun lien n'est ajouté ni retiré localement : la scène ne montre que ce
//! que le miroir contient, c'est-à-dire ce que le démon a annoncé. La seule
//! décision prise ici est un **refus** — celui d'une boucle
//! ([`cree_un_cycle`], F-12), d'un nœud vers lui-même ou d'un doublon exact —
//! et refuser n'est pas anticiper. Le démon reste l'autorité : s'il refuse à
//! son tour, c'est sa phrase qui s'affiche.
//!
//! # Le gain : une commande au relâchement, une valeur montrée pendant le
//! geste
//!
//! Une glissière d'`iced` émet un message **par pixel parcouru**. Envoyer
//! `SetNodeGain` à chaque message inonderait le démon d'une centaine de
//! commandes pour un seul geste : la commande ne part donc qu'au relâchement
//! ([`Slider::on_release`]), et la valeur parcourue est retenue le temps du
//! geste dans [`State::glissement`].
//!
//! C'est le **seul** endroit de la GUI où un état local devance la
//! notification, et ce n'en est pas pour autant une anticipation du résultat :
//! ce qui s'affiche est la **position du doigt**, pas l'état du démon. La
//! distinction se voit dans le code — [`State::glissement`] ne touche jamais au
//! miroir — et dans le comportement : dès que le démon a parlé, la valeur
//! locale est jetée et l'affichage retombe sur le miroir.
//!
//! Le démon parle ici par une **relecture** et non par une notification : le
//! protocole n'en diffuse aucune pour un gain ou une coupure. L'interface
//! renvoie donc un `Nodes` ou un `Links` derrière chaque réglage et remplace la
//! partie correspondante du miroir ([`State::relu`], [`crate::ipc::Relecture`])
//! — plutôt que de supposer localement ce que le démon a fait.
//!
//! La **butée basse** de la glissière ([`GAIN_MIN`], −60 dB) n'est pas un gain
//! de −60 dB : c'est un silence, et c'est [`Db::NEG_INF`] qui part au démon
//! (voir [`gain_de_position`]).
//!
//! Le bouton de coupure, lui, n'a pas ce problème — un clic, une commande — et
//! **n'anticipe rien** : l'état coupé qu'il dessine est celui du miroir.
//!
//! Aucune notice n'accompagne un réglage de gain : le geste est continu, une
//! phrase par mouvement serait du bruit. La coupure, elle, en mérite une (voir
//! [`crate::app`]).
//!
//! [`Slider::on_release`]: iced::widget::Slider::on_release
//!
//! # Le déplacement est un réglage local
//!
//! Déplacer une carte ne change rien au graphe : aucune commande n'est
//! envoyée. Les positions sont mémorisées par **clé stable de nœud**
//! ([`NodeKey`]), jamais par `NodeId` — celui-ci porte une génération et change
//! si le nœud est recréé, alors que débrancher puis rebrancher un casque doit
//! le remettre à sa place. Elles sont écrites au **relâchement
//! seulement** (voir [`crate::preferences`]).
//!
//! [`NodeKey`]: conduit_protocol::api::NodeKey

use std::collections::{BTreeMap, BTreeSet};

use conduit_core::graph::{Direction, LinkId, NodeId, PortId};
use conduit_core::types::Db;
use conduit_protocol::api::NodeKey;
use conduit_protocol::{Command, LinkDescriptor, NodeDescriptor, NodeState, Notification};
use iced::font::Weight;
use iced::mouse;
use iced::widget::canvas::{LineCap, LineDash, LineJoin, Path, Stroke};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
    button, canvas, column, container, mouse_area, pin, row, rule, scrollable, slider, space,
    stack, text,
};
use iced::{
    Center, Color, Element, Fill, Length, Padding, Point, Rectangle, Renderer, Right, Size, Theme,
    Vector,
};
use serde::{Deserialize, Serialize};

use crate::app::Message;
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::theme::{jetons, Jetons, CORPS_INTERFACE, ESPACE_S, FILET};
use crate::{format, style, typo};

// --- Mesures de la maquette -------------------------------------------------

/// Largeur d'une carte de nœud.
pub const LARGEUR_CARTE: f32 = 210.0;
/// Hauteur de l'en-tête d'une carte.
pub const HAUTEUR_ENTETE: f32 = 36.0;
/// Hauteur d'une ligne de port.
pub const HAUTEUR_PORT: f32 = 22.0;
/// Marge verticale de la grille des ports, en haut comme en bas.
pub const MARGE_PORTS: f32 = 6.0;
/// Diamètre de la pastille d'un port.
pub const PASTILLE: f32 = 9.0;
/// De combien la pastille d'un port déborde du bord de la carte.
pub const DEBORDEMENT: f32 = 5.0;
/// Écart horizontal entre deux colonnes de rôle.
const ECART_COLONNES: f32 = 80.0;
/// Écart vertical entre deux cartes d'une même colonne.
const ECART_CARTES: f32 = 24.0;
/// Marge autour de la scène.
const MARGE_SCENE: f32 = 24.0;
/// Padding horizontal de l'en-tête d'une carte.
const PADDING_ENTETE: f32 = 10.0;
/// Padding horizontal de la grille des ports : de quoi dégager la part de
/// pastille qui rentre dans la carte.
const PADDING_PORTS: f32 = 12.0;
/// Corps de l'étiquette d'état, en petites capitales.
const CORPS_ETIQUETTE: f32 = 10.0;
/// Corps du nom d'un port.
const CORPS_PORT: f32 = 11.0;
/// Corps du libellé en décibels et du bouton de coupure.
const CORPS_GAIN: f32 = 11.0;
/// Padding du pied d'une carte : 4 en haut, 10 sur les côtés, 8 en bas.
const PADDING_PIED: Padding = Padding {
    top: 4.0,
    right: 10.0,
    bottom: 8.0,
    left: 10.0,
};
/// Hauteur des réglages d'un pied : celle du bouton de coupure, le plus haut
/// des trois.
const HAUTEUR_REGLAGE: f32 = 22.0;
/// Hauteur du pied d'une carte, padding compris.
pub const HAUTEUR_PIED: f32 = PADDING_PIED.top + HAUTEUR_REGLAGE + PADDING_PIED.bottom;
/// Hauteur de la glissière de gain.
const HAUTEUR_GLISSIERE: f32 = 18.0;
/// Largeur du libellé en décibels.
///
/// Elle est **fixée** : `tnum` étant inaccessible depuis `iced` (voir
/// [`crate::typo`]), c'est la largeur de la cellule qui tient la colonne quand
/// la valeur change.
const LARGEUR_GAIN: f32 = 44.0;
/// Largeur du bouton de coupure.
const LARGEUR_MUET: f32 = 24.0;
/// Largeur de la glissière du réglage de lien, dans l'en-tête : là, rien ne
/// dit la place à prendre, il faut donc la donner.
const LARGEUR_GLISSIERE_ENTETE: f32 = 96.0;
/// Interligne serré des textes d'une carte : un libellé n'a pas besoin des
/// 1,6 du texte courant.
const INTERLIGNE_SERRE: f32 = 1.1;
/// Padding du texte d'aide, sous la scène.
const PADDING_AIDE: Padding = Padding {
    top: 10.0,
    right: 14.0,
    bottom: 10.0,
    left: 14.0,
};
/// Padding de la zone vide, quand le graphe n'a aucun nœud.
const PADDING_VIDE: f32 = 24.0;
/// Identifiant du conteneur d'une carte : il ne sert qu'au test qui mesure la
/// hauteur réellement composée.
const ID_CARTE: &str = "patchbay.carte";

// --- Décisions --------------------------------------------------------------

/// Rôle d'un nœud dans la disposition automatique, déduit de ses ports.
///
/// Le signal va de gauche à droite : ce qui n'a pas d'entrée est une source,
/// ce qui n'a pas de sortie est un puits, le reste passe au milieu. Le nœud
/// qui pilote le graphe est un puits quoi qu'il arrive : c'est lui qui donne
/// l'heure, tout finit chez lui.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Sans entrée : à gauche.
    Source,
    /// Entrées et sorties : au centre.
    Intermediaire,
    /// Sans sortie, ou pilote du graphe : à droite.
    Puits,
}

impl Role {
    /// Les trois rôles, de gauche à droite.
    pub const ALL: [Role; 3] = [Role::Source, Role::Intermediaire, Role::Puits];

    /// Rang de la colonne du rôle, à partir de la gauche.
    pub fn colonne(self) -> usize {
        match self {
            Role::Source => 0,
            Role::Intermediaire => 1,
            Role::Puits => 2,
        }
    }
}

/// Rôle d'un nœud, déduit de ses ports et de son état.
pub fn role(noeud: &NodeDescriptor) -> Role {
    if noeud.state == NodeState::Driver || noeud.outputs.is_empty() {
        Role::Puits
    } else if noeud.inputs.is_empty() {
        Role::Source
    } else {
        Role::Intermediaire
    }
}

/// Étiquette portée par l'en-tête d'une carte : ce que le nœud est, ou ce qui
/// lui arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Etiquette {
    /// Le nœud pilote le graphe.
    Pilote,
    /// Son périphérique est absent ou fermé ; ses liens sont conservés.
    Suspendu,
    /// Un côté d'un câble virtuel de Conduit.
    Cable,
    /// Un périphérique du système.
    Materiel,
    /// Un nœud interne du moteur.
    Utilitaire,
}

impl Etiquette {
    /// Libellé affiché, en petites capitales.
    pub fn libelle(self) -> &'static str {
        i18n::t(match self {
            Etiquette::Pilote => Text::NodeDriver,
            Etiquette::Suspendu => Text::NodeSuspended,
            Etiquette::Cable => Text::NodeCable,
            Etiquette::Materiel => Text::NodeHardware,
            Etiquette::Utilitaire => Text::NodeUtility,
        })
    }

    /// Vrai si l'étiquette dit un état plutôt qu'une nature : elle porte alors
    /// la couleur d'accent du mode.
    pub fn accentuee(self) -> bool {
        matches!(self, Etiquette::Pilote | Etiquette::Suspendu)
    }
}

/// Étiquette d'un nœud : son état d'abord, sa nature ensuite.
pub fn etiquette(noeud: &NodeDescriptor) -> Etiquette {
    match noeud.state {
        NodeState::Driver => Etiquette::Pilote,
        NodeState::Suspended => Etiquette::Suspendu,
        _ if noeud.device.as_ref().and_then(|d| d.cable).is_some() => Etiquette::Cable,
        _ if noeud.device.is_some() => Etiquette::Materiel,
        _ => Etiquette::Utilitaire,
    }
}

/// Hauteur d'une carte à `entrees` entrées et `sorties` sorties.
///
/// L'en-tête, son filet, une grille de ports à deux colonnes — c'est la plus
/// fournie des deux qui donne le nombre de lignes —, un second filet et le
/// pied de gain. Les constantes sont celles de la composition : un test le
/// vérifie en mesurant une carte réellement composée.
pub fn hauteur_carte(entrees: usize, sorties: usize) -> f32 {
    let lignes = entrees.max(sorties) as f32;
    HAUTEUR_ENTETE + FILET + 2.0 * MARGE_PORTS + lignes * HAUTEUR_PORT + FILET + HAUTEUR_PIED
}

/// Où et sur quelle hauteur une carte est posée, dans le repère de la scène.
///
/// La position est le coin haut-gauche de la **carte**, pastilles non
/// comprises : celles-ci débordent de [`DEBORDEMENT`] de chaque côté.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// Clé stable du nœud, telle que `NodeKey::to_string` la rend.
    pub cle: String,
    /// Coin haut-gauche de la carte.
    pub position: Point,
    /// Hauteur de la carte, telle que [`hauteur_carte`] la calcule.
    pub hauteur: f32,
}

/// Point d'attache d'un port sur sa carte : le centre de sa pastille.
///
/// Une entrée s'attache à gauche de la carte, une sortie à droite ; `index`
/// est le rang du port dans sa colonne. C'est le point d'où partent et où
/// arrivent les courbes de liens.
pub fn ancre(placement: &Placement, direction: Direction, index: usize) -> Point {
    let x = match direction {
        Direction::Input => placement.position.x - DEBORDEMENT + PASTILLE / 2.0,
        Direction::Output => placement.position.x + LARGEUR_CARTE + DEBORDEMENT - PASTILLE / 2.0,
    };
    Point::new(x, placement.position.y + ordonnee_port(index))
}

/// Ordonnée du centre de la `index`-ième ligne de ports, depuis le haut de la
/// carte.
fn ordonnee_port(index: usize) -> f32 {
    HAUTEUR_ENTETE + FILET + MARGE_PORTS + (index as f32 + 0.5) * HAUTEUR_PORT
}

/// Positions retenues d'une session à l'autre, par clé stable de nœud.
///
/// Les clés dont aucun nœud ne répond aujourd'hui sont conservées : un
/// périphérique débranché revient à sa place.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Positions(BTreeMap<String, (f32, f32)>);

impl Positions {
    /// Position retenue pour une clé, s'il y en a une.
    pub fn get(&self, cle: &str) -> Option<Point> {
        self.0.get(cle).map(|(x, y)| Point::new(*x, *y))
    }

    /// Retient une position pour une clé.
    pub fn set(&mut self, cle: &str, position: Point) {
        self.0.insert(cle.to_string(), (position.x, position.y));
    }

    /// Nombre de positions retenues.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Vrai si aucune position n'est retenue.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Les positions de `nouvelles` posées sur celles-ci : ce qui est ici sans
    /// être là-bas est **conservé**.
    ///
    /// C'est ce qui permet d'écrire le fichier sans perdre les nœuds qu'une
    /// autre session, ou un autre jour, y a laissés.
    pub fn fusion(&self, nouvelles: &Positions) -> Positions {
        let mut fusion = self.0.clone();
        fusion.extend(nouvelles.0.iter().map(|(k, v)| (k.clone(), *v)));
        Positions(fusion)
    }
}

/// Dispose les cartes : une position mémorisée l'emporte, sinon le créneau
/// automatique de la colonne du rôle.
///
/// La fonction est **pure** : mêmes nœuds et mêmes positions, mêmes
/// placements. Le créneau est calculé pour tous les nœuds, y compris ceux
/// dont la position est mémorisée : déplacer une carte ne fait donc pas
/// remonter celles qui la suivaient.
///
/// Les placements sont rendus dans l'ordre des nœuds reçus.
pub fn disposer(noeuds: &[NodeDescriptor], positions: &Positions) -> Vec<Placement> {
    // Le créneau se lit dans un ordre stable, indépendant de celui du démon :
    // par état, puis par libellé, puis par clé.
    let mut rangs: Vec<usize> = (0..noeuds.len()).collect();
    rangs.sort_by(|a, b| tri(&noeuds[*a]).cmp(&tri(&noeuds[*b])));

    let mut creneaux = vec![Point::ORIGIN; noeuds.len()];
    let mut hauteurs = vec![0.0_f32; noeuds.len()];
    let mut bas = [MARGE_SCENE; Role::ALL.len()];
    for rang in rangs {
        let noeud = &noeuds[rang];
        let colonne = role(noeud).colonne();
        let hauteur = hauteur_carte(noeud.inputs.len(), noeud.outputs.len());
        let x = MARGE_SCENE + colonne as f32 * (LARGEUR_CARTE + ECART_COLONNES);
        creneaux[rang] = Point::new(x, bas[colonne]);
        hauteurs[rang] = hauteur;
        bas[colonne] += hauteur + ECART_CARTES;
    }

    noeuds
        .iter()
        .enumerate()
        .map(|(i, noeud)| {
            let cle = noeud.key.to_string();
            let position = positions.get(&cle).unwrap_or(creneaux[i]);
            Placement {
                cle,
                position,
                hauteur: hauteurs[i],
            }
        })
        .collect()
}

/// Clé de tri intra-colonne : l'état d'abord — pilote, actif, interne,
/// suspendu —, puis le libellé, puis la clé stable pour départager.
fn tri(noeud: &NodeDescriptor) -> (u8, &str, String) {
    let etat = match noeud.state {
        NodeState::Driver => 0,
        NodeState::Active => 1,
        NodeState::Internal => 2,
        NodeState::Suspended => 3,
    };
    (etat, noeud.label.as_str(), noeud.key.to_string())
}

/// Étendue de la scène : de quoi contenir toutes les cartes, pastilles et
/// marge comprises.
///
/// Elle donne sa taille au canevas, donc à la zone que le défilement promène.
pub fn etendue(placements: &[Placement]) -> Size {
    let mut largeur: f32 = 0.0;
    let mut hauteur: f32 = 0.0;
    for p in placements {
        largeur = largeur.max(p.position.x + LARGEUR_CARTE + DEBORDEMENT + MARGE_SCENE);
        hauteur = hauteur.max(p.position.y + p.hauteur + MARGE_SCENE);
    }
    Size::new(largeur, hauteur)
}

// --- Gains ------------------------------------------------------------------

/// Butée basse de la glissière de gain, en décibels.
///
/// Elle ne vaut **pas** −60 dB : elle vaut silence (voir
/// [`gain_de_position`]). Le protocole accepte jusqu'à
/// [`Db::SILENCE_THRESHOLD`] (−120 dB), mais une course de 120 dB rendrait le
/// dernier tiers inaudible et le premier intouchable.
pub const GAIN_MIN: f32 = -60.0;
/// Butée haute de la glissière de gain, en décibels.
///
/// Le protocole autorise jusqu'à [`Db::MAX`] (+24 dB) ; +12 suffit à rattraper
/// une source faible sans mettre la saturation à portée d'un geste distrait.
pub const GAIN_MAX: f32 = 12.0;
/// Pas de la glissière de gain, en décibels.
pub const GAIN_PAS: f32 = 1.0;

/// Position de glissière qui montre le gain donné.
///
/// Un gain hors course est **ramené à la butée** : le démon peut connaître
/// +24 dB, la glissière ne sait pas le montrer. Le silence se pose sur la
/// butée basse, qui est justement ce qu'il faut y lire.
pub fn position_de_gain(db: Db) -> f32 {
    if db.is_silent() {
        return GAIN_MIN;
    }
    db.get().clamp(GAIN_MIN, GAIN_MAX)
}

/// Gain que la glissière demande à cette position.
///
/// La **butée basse vaut silence** : `−60` donne [`Db::NEG_INF`] et non
/// `Db::new(-60.0)`. Descendre une glissière à fond, c'est demander à ne plus
/// rien entendre, pas à entendre un millionième.
pub fn gain_de_position(position: f32) -> Db {
    let position = position.clamp(GAIN_MIN, GAIN_MAX);
    if position <= GAIN_MIN {
        Db::NEG_INF
    } else {
        Db::new(position.round())
    }
}

/// Ce dont un réglage de gain change la valeur : un nœud, ou un lien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cible {
    /// Un nœud du graphe.
    Noeud(NodeId),
    /// Un lien du graphe.
    Lien(LinkId),
}

impl Cible {
    /// La commande qui porte ce réglage au démon.
    pub fn commande(self, gain_db: Option<Db>, muted: Option<bool>) -> Command {
        match self {
            Cible::Noeud(node) => Command::SetNodeGain {
                node,
                gain_db,
                muted,
            },
            Cible::Lien(link) => Command::SetLinkGain {
                link,
                gain_db,
                muted,
            },
        }
    }
}

/// L'état coupé d'une cible réglable, tel que le miroir le connaît.
///
/// `None` dit qu'il n'y a rien à régler : la cible a disparu du miroir, ou
/// c'est un **nœud suspendu** — son périphérique est absent, sa carte est
/// voilée et ses réglages sont désactivés.
pub fn coupure(cible: Cible, noeuds: &[NodeDescriptor], links: &[LinkDescriptor]) -> Option<bool> {
    match cible {
        Cible::Noeud(id) => noeuds
            .iter()
            .find(|n| n.id == id)
            .filter(|n| n.state != NodeState::Suspended)
            .map(|n| n.muted),
        Cible::Lien(id) => links.iter().find(|l| l.link.id == id).map(|l| l.muted),
    }
}

/// Vrai si le gain de cette cible se règle : voir [`coupure`].
pub fn reglable(cible: Cible, noeuds: &[NodeDescriptor], links: &[LinkDescriptor]) -> bool {
    coupure(cible, noeuds, links).is_some()
}

/// La cible dont une notification dit quelque chose, s'il y en a une.
///
/// C'est elle qui décide si la valeur montrée pendant un geste doit céder la
/// place à celle du miroir (voir [`State::notifie`]). Le protocole n'a pas
/// aujourd'hui de notification propre au gain : les variantes listées ici sont
/// celles qui remplacent ou retirent un descripteur, donc celles qui peuvent
/// contredire ce qui est affiché.
pub fn cible_notifiee(notification: &Notification) -> Option<Cible> {
    match notification {
        Notification::NodeAdded(node) => Some(Cible::Noeud(node.id)),
        Notification::NodeRemoved { id, .. } | Notification::NodeStateChanged { id, .. } => {
            Some(Cible::Noeud(*id))
        }
        Notification::LinkAdded(link) => Some(Cible::Lien(link.link.id)),
        Notification::LinkRemoved { id } => Some(Cible::Lien(*id)),
        _ => None,
    }
}

// --- Liens ------------------------------------------------------------------

/// Épaisseur d'un lien au repos.
const EPAISSEUR_LIEN: f32 = 1.5;
/// Épaisseur du lien sélectionné.
const EPAISSEUR_LIEN_CHOISI: f32 = 2.5;
/// Part de l'écart horizontal reportée sur les points de contrôle.
const TENSION: f32 = 0.5;
/// Écart horizontal minimal des points de contrôle : sans lui, deux ports
/// presque alignés donneraient un trait et non une courbe.
const TENSION_MINIMALE: f32 = 50.0;
/// Pointillés du lien en cours de tirage : 4 px de trait, 4 px de vide.
const POINTILLES: [f32; 2] = [4.0, 4.0];
/// Nombre d'intervalles échantillonnés sur une courbe pour le test de clic.
const ECHANTILLONS: usize = 16;
/// Distance au-delà de laquelle un clic ne touche plus une courbe.
const SEUIL_CLIC: f32 = 6.0;

/// Fréquence du générateur de test : le la du diapason.
pub const FREQUENCE_TEST: f32 = 440.0;
/// Niveau crête du générateur de test, en décibels pleine échelle.
///
/// Le générateur part vers de vraies enceintes : −12 dBFS laisse le sinus
/// franchement audible sans faire sursauter qui a monté le volume. La pleine
/// échelle serait un signal de test dangereux pour les oreilles comme pour les
/// haut-parleurs.
pub const NIVEAU_TEST_DB: f32 = -12.0;
/// Canaux du générateur de test : stéréo, comme la plupart des entrées.
pub const CANAUX_TEST: usize = 2;

/// Amplitude crête du générateur de test, en gain linéaire (≈ 0,251).
pub fn amplitude_test() -> f32 {
    10.0_f32.powf(NIVEAU_TEST_DB / 20.0)
}

/// Un nom libre pour un nouveau générateur de test.
///
/// Le démon refuse deux nœuds internes de même nom : le second générateur
/// s'appelle donc « Générateur de test 2 », et ainsi de suite.
pub fn nom_de_generateur(noeuds: &[NodeDescriptor]) -> String {
    let base = i18n::t(Text::PatchbayGenerator);
    let pris = |nom: &str| {
        let cle = NodeKey::internal(nom);
        noeuds.iter().any(|n| n.key == cle)
    };
    if !pris(base) {
        return base.to_string();
    }
    let mut rang = 2;
    loop {
        let nom = format!("{base} {rang}");
        if !pris(&nom) {
            return nom;
        }
        rang += 1;
    }
}

/// Les deux points de contrôle de la courbe qui va de `a` à `b`.
///
/// Ils sortent **horizontalement** de chaque extrémité, d'une demi-largeur de
/// l'écart horizontal et d'au moins `TENSION_MINIMALE` : le lien quitte la
/// sortie vers la droite et entre par la gauche, quel que soit le sens dans
/// lequel les deux cartes ont été rangées.
pub fn controles(a: Point, b: Point) -> (Point, Point) {
    let dx = ((b.x - a.x).abs() * TENSION).max(TENSION_MINIMALE);
    (Point::new(a.x + dx, a.y), Point::new(b.x - dx, b.y))
}

/// Le point de la courbe de `a` à `b` au paramètre `t` ∈ [0, 1].
pub fn point_de_courbe(a: Point, b: Point, t: f32) -> Point {
    let (c1, c2) = controles(a, b);
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        w0 * a.x + w1 * c1.x + w2 * c2.x + w3 * b.x,
        w0 * a.y + w1 * c1.y + w2 * c2.y + w3 * b.y,
    )
}

/// Le chemin de la courbe qui va de `a` à `b`.
fn chemin(a: Point, b: Point) -> Path {
    let (c1, c2) = controles(a, b);
    Path::new(|constructeur| {
        constructeur.move_to(a);
        constructeur.bezier_curve_to(c1, c2, b);
    })
}

/// Distance d'un point au **segment** qui va de `a` à `b`.
fn distance_au_segment(a: Point, b: Point, point: Point) -> f32 {
    let segment = b - a;
    let carre = segment.x * segment.x + segment.y * segment.y;
    if carre <= f32::EPSILON {
        return a.distance(point);
    }
    let t = (((point.x - a.x) * segment.x + (point.y - a.y) * segment.y) / carre).clamp(0.0, 1.0);
    Point::new(a.x + t * segment.x, a.y + t * segment.y).distance(point)
}

/// Distance approchée d'un point à la courbe qui va de `a` à `b`.
///
/// La courbe est découpée en `ECHANTILLONS` segments et c'est la distance
/// aux **segments** qui est mesurée, non aux seuls points d'échantillonnage :
/// sur une courbe longue de plusieurs centaines de pixels, un point tous les
/// vingt pixels laisserait un clic posé entre deux échantillons hors du seuil.
/// Cela évite au passage de résoudre une équation de degré cinq.
pub fn distance_a_la_courbe(a: Point, b: Point, point: Point) -> f32 {
    let mut precedent = a;
    let mut distance = f32::INFINITY;
    for i in 1..=ECHANTILLONS {
        let courant = point_de_courbe(a, b, i as f32 / ECHANTILLONS as f32);
        distance = distance.min(distance_au_segment(precedent, courant, point));
        precedent = courant;
    }
    distance
}

/// Un lien prêt à dessiner : ses deux extrémités dans le repère de la scène.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Courbe {
    /// Le lien représenté.
    pub lien: LinkId,
    /// Point d'attache de la sortie.
    pub depart: Point,
    /// Point d'attache de l'entrée.
    pub arrivee: Point,
}

/// Point d'attache d'un port, si son nœud est encore là et placé.
pub fn ancre_de_port(
    port: PortId,
    noeuds: &[NodeDescriptor],
    placements: &[Placement],
) -> Option<Point> {
    let rang = noeuds.iter().position(|n| n.id == port.node)?;
    Some(ancre(
        placements.get(rang)?,
        port.direction,
        port.index as usize,
    ))
}

/// Les liens dessinables : ceux dont les deux extrémités ont un placement.
///
/// Un lien dont un nœud a disparu du miroir n'est pas dessiné, et ce n'est pas
/// une erreur : la notification qui retire le lien peut arriver après celle qui
/// retire le nœud.
pub fn courbes(
    links: &[LinkDescriptor],
    noeuds: &[NodeDescriptor],
    placements: &[Placement],
) -> Vec<Courbe> {
    links
        .iter()
        .filter_map(|lien| {
            Some(Courbe {
                lien: lien.link.id,
                depart: ancre_de_port(lien.link.src, noeuds, placements)?,
                arrivee: ancre_de_port(lien.link.dst, noeuds, placements)?,
            })
        })
        .collect()
}

/// Le lien le plus proche du point cliqué, s'il passe à moins de
/// `SEUIL_CLIC`.
pub fn lien_le_plus_proche(courbes: &[Courbe], point: Point) -> Option<LinkId> {
    courbes
        .iter()
        .map(|c| (c.lien, distance_a_la_courbe(c.depart, c.arrivee, point)))
        .filter(|(_, distance)| *distance <= SEUIL_CLIC)
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(lien, _)| lien)
}

/// Vrai si lier `depuis` à `vers` fermerait une boucle (F-12).
///
/// Fonction **pure** : un parcours en profondeur du graphe des liens, parti de
/// `vers`. Si `depuis` est atteint, c'est que le signal reviendrait sur ses
/// pas ; un nœud vers lui-même est donc une boucle, lui aussi.
///
/// Le démon reste l'autorité : ce refus local évite un aller-retour et permet
/// de nommer les deux nœuds, il ne remplace pas la vérification du moteur.
pub fn cree_un_cycle(links: &[LinkDescriptor], depuis: NodeId, vers: NodeId) -> bool {
    let mut vus = BTreeSet::new();
    let mut pile = vec![vers];
    while let Some(noeud) = pile.pop() {
        if noeud == depuis {
            return true;
        }
        if !vus.insert(noeud) {
            continue;
        }
        pile.extend(
            links
                .iter()
                .filter(|l| l.link.src.node == noeud)
                .map(|l| l.link.dst.node),
        );
    }
    false
}

// --- État -------------------------------------------------------------------

/// Geste de l'utilisateur sur la scène.
#[derive(Debug, Clone, PartialEq)]
pub enum Geste {
    /// L'en-tête d'une carte vient d'être pressé.
    Saisi(String),
    /// Le curseur a bougé, en coordonnées de la scène.
    Deplace(Point),
    /// Le bouton est relâché après le déplacement d'une carte.
    Relache,
    /// Une pastille de sortie vient d'être pressée : un lien se tire.
    DebutLien(PortId),
    /// Le curseur entre sur une pastille d'entrée, ou la quitte.
    SurvolPort(Option<PortId>),
    /// Le bouton est relâché pendant le tirage d'un lien.
    FinLien,
    /// Un lien est choisi dans le canevas, ou le vide est cliqué.
    Selection(Option<LinkId>),
    /// Suppr ou Retour arrière : le lien sélectionné s'en va.
    Supprimer,
    /// La glissière de gain d'une cible est à cette position, en décibels.
    ///
    /// `iced` en émet un par pixel : rien ne part au démon (voir le module).
    Gain(Cible, f32),
    /// La glissière de gain d'une cible est relâchée : la valeur atteinte part
    /// au démon.
    FinGain(Cible),
    /// Le bouton de coupure d'une cible est pressé.
    Muet(Cible),
}

/// Le lien en cours de tirage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tirage {
    /// Port de sortie d'où part le lien.
    pub source: PortId,
    /// Port d'entrée survolé, s'il y en a un.
    ///
    /// C'est le **survol du widget** qui le remplit, pas un test
    /// d'intersection : les bornes réelles de la pastille sont plus justes que
    /// la géométrie que le canevas recalcule.
    pub cible: Option<PortId>,
}

/// Ce qu'un geste demande à l'application, en plus de son effet local.
#[derive(Debug, Clone, PartialEq)]
pub enum Effet {
    /// Rien : le geste s'est joué entièrement dans l'état de la vue.
    Rien,
    /// Écrire les préférences : une carte a bougé.
    Enregistrer,
    /// Envoyer cette commande au démon.
    Commande(Command),
    /// Refuser le lien et le dire : il fermerait une boucle (F-12).
    Boucle {
        /// Nœud d'où le lien demandé partait.
        depuis: NodeId,
        /// Nœud où il allait, et qui alimente déjà `depuis`.
        vers: NodeId,
    },
}

/// La carte en cours de déplacement.
#[derive(Debug, Clone, PartialEq)]
pub struct Saisie {
    /// Clé stable du nœud saisi.
    pub cle: String,
    /// Position de la carte au moment de la saisie.
    pub depart: Point,
    /// Écart entre le curseur et le coin de la carte.
    ///
    /// Il n'est connu qu'au **premier mouvement** : `iced` ne donne pas la
    /// position du curseur avec la pression d'un [`mouse_area`]. La carte ne
    /// bouge donc pas de ce premier mouvement — de quelques pixels au plus —,
    /// et suit exactement le curseur ensuite.
    pub ecart: Option<Vector>,
    /// Vrai dès que la carte a effectivement bougé : c'est ce qui décide
    /// d'écrire les préférences au relâchement.
    pub deplacee: bool,
}

/// La glissière de gain qu'un doigt est en train de promener.
///
/// C'est la seule valeur que la vue montre avant que le démon ne l'ait
/// annoncée — et ce n'est pas son état à lui, c'est la position du doigt (voir
/// le module).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glissement {
    /// Ce dont le gain est réglé.
    pub cible: Cible,
    /// Position courante de la glissière, en décibels.
    pub position: f32,
}

/// État de la vue, distinct du miroir du démon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// Positions mémorisées, lues au démarrage et écrites au relâchement.
    pub positions: Positions,
    /// Carte en cours de déplacement, s'il y en a une.
    pub saisie: Option<Saisie>,
    /// Lien en cours de tirage, s'il y en a un.
    pub tirage: Option<Tirage>,
    /// Lien sélectionné, s'il y en a un.
    ///
    /// Il peut désigner un lien que le démon vient de retirer :
    /// [`State::lien_selectionne`] ne le rend que s'il est encore là.
    pub selection: Option<LinkId>,
    /// Glissière de gain en cours de réglage, s'il y en a une.
    ///
    /// Elle survit au relâchement : la commande est partie, mais le démon n'a
    /// encore rien annoncé, et faire retomber l'affichage sur l'ancienne
    /// valeur du miroir le temps de l'aller-retour serait un clignotement.
    /// C'est [`State::notifie`] qui la jette.
    pub glissement: Option<Glissement>,
}

impl State {
    /// Vrai si une carte est en cours de déplacement.
    pub fn deplacement(&self) -> bool {
        self.saisie.is_some()
    }

    /// Vrai si cette carte est celle qu'on déplace.
    pub fn saisie_de(&self, cle: &str) -> bool {
        self.saisie.as_ref().is_some_and(|s| s.cle == cle)
    }

    /// Vrai si un lien est en cours de tirage.
    pub fn en_tirage(&self) -> bool {
        self.tirage.is_some()
    }

    /// Le lien sélectionné, s'il existe encore dans le graphe.
    pub fn lien_selectionne(&self, links: &[LinkDescriptor]) -> Option<LinkId> {
        let id = self.selection?;
        links.iter().any(|l| l.link.id == id).then_some(id)
    }

    /// Position que la glissière d'une cible doit montrer : celle du doigt
    /// pendant le geste, celle du miroir sinon.
    pub fn position_de(&self, cible: Cible, db: Db) -> f32 {
        match self.glissement {
            Some(g) if g.cible == cible => g.position,
            _ => position_de_gain(db),
        }
    }

    /// Gain que le libellé d'une cible doit écrire : celui du doigt pendant le
    /// geste, celui du miroir sinon.
    ///
    /// Hors geste, c'est bien la valeur **du miroir** qui est écrite, et non
    /// sa traduction en position : le démon a le droit de connaître un gain
    /// que la glissière ne sait pas montrer, et le libellé ne doit pas
    /// l'arrondir pour autant.
    pub fn gain_affiche(&self, cible: Cible, db: Db) -> Db {
        match self.glissement {
            Some(g) if g.cible == cible => gain_de_position(g.position),
            _ => db,
        }
    }

    /// Une notification est arrivée : la valeur montrée localement pour sa
    /// cible n'a plus lieu d'être.
    ///
    /// C'est ce qui referme la seule parenthèse d'état local de la GUI :
    /// passé ce point, l'affichage ne dit plus que le miroir.
    pub fn notifie(&mut self, notification: &Notification) {
        let Some(cible) = cible_notifiee(notification) else {
            return;
        };
        if self.glissement.is_some_and(|g| g.cible == cible) {
            self.glissement = None;
        }
    }

    /// Une relecture est arrivée : le démon vient de dire l'état réel des
    /// gains, la valeur montrée localement n'a plus lieu d'être.
    ///
    /// C'est l'autre façon de refermer la parenthèse d'état local, et la seule
    /// qui vaille pour un gain : le protocole ne diffuse aucune notification
    /// quand un gain ou une coupure change (voir [`crate::ipc::Relecture`]).
    pub fn relu(&mut self) {
        self.glissement = None;
    }

    /// Oublie tout réglage local : le miroir vient d'être rechargé.
    pub fn recharge(&mut self) {
        self.glissement = None;
    }

    /// Réduit un geste et dit ce qu'il demande à l'application.
    ///
    /// Pure : aucune entrée/sortie. `placements` dit où les cartes se
    /// trouvaient au moment de la saisie, `noeuds` et `links` ce que le démon
    /// a annoncé — le seul refus décidé ici est celui de la boucle, et la
    /// coupure y lit l'état qu'elle doit inverser.
    pub fn reduire(
        &mut self,
        geste: Geste,
        placements: &[Placement],
        noeuds: &[NodeDescriptor],
        links: &[LinkDescriptor],
    ) -> Effet {
        match geste {
            Geste::Saisi(cle) => {
                if let Some(placement) = placements.iter().find(|p| p.cle == cle) {
                    self.saisie = Some(Saisie {
                        cle,
                        depart: placement.position,
                        ecart: None,
                        deplacee: false,
                    });
                }
            }
            Geste::Deplace(curseur) => {
                if let Some(saisie) = &mut self.saisie {
                    match saisie.ecart {
                        None => saisie.ecart = Some(curseur - saisie.depart),
                        Some(ecart) => {
                            let position = Point::new(
                                (curseur.x - ecart.x).max(0.0),
                                (curseur.y - ecart.y).max(0.0),
                            );
                            self.positions.set(&saisie.cle, position);
                            saisie.deplacee = true;
                        }
                    }
                }
            }
            Geste::Relache => {
                if self.saisie.take().is_some_and(|s| s.deplacee) {
                    return Effet::Enregistrer;
                }
            }
            Geste::DebutLien(source) => {
                // Un lien part d'une sortie : une pression sur une entrée ne
                // commence rien.
                if source.direction == Direction::Output {
                    self.tirage = Some(Tirage {
                        source,
                        cible: None,
                    });
                }
            }
            Geste::SurvolPort(cible) => {
                if let Some(tirage) = &mut self.tirage {
                    tirage.cible = cible.filter(|p| p.direction == Direction::Input);
                }
            }
            Geste::FinLien => return self.finir_le_lien(links),
            Geste::Selection(lien) => {
                // Un second clic sur le même lien le désélectionne ; un clic à
                // l'écart aussi.
                self.selection = match lien {
                    Some(id) if self.selection == Some(id) => None,
                    autre => autre,
                };
            }
            Geste::Supprimer => {
                if let Some(link) = self.lien_selectionne(links) {
                    return Effet::Commande(Command::Unlink { link });
                }
            }
            Geste::Gain(cible, position) => {
                // Un message par pixel : rien ne part au démon, la valeur est
                // seulement montrée (voir le module).
                if reglable(cible, noeuds, links) {
                    self.glissement = Some(Glissement { cible, position });
                }
            }
            Geste::FinGain(cible) => {
                if let Some(glissement) = self.glissement.filter(|g| g.cible == cible) {
                    return Effet::Commande(
                        cible.commande(Some(gain_de_position(glissement.position)), None),
                    );
                }
            }
            Geste::Muet(cible) => {
                // L'état coupé vient du miroir : le bouton envoie son inverse
                // et n'anticipe pas ce que le démon en fera.
                if let Some(muted) = coupure(cible, noeuds, links) {
                    return Effet::Commande(cible.commande(None, Some(!muted)));
                }
            }
        }
        Effet::Rien
    }

    /// Ce que le relâchement d'un tirage produit.
    ///
    /// Trois refus muets — le tirage lâché dans le vide, le nœud vers
    /// lui-même, le doublon exact —, un refus qui se dit — la boucle —, et le
    /// seul cas qui parle au démon.
    fn finir_le_lien(&mut self, links: &[LinkDescriptor]) -> Effet {
        let Some(tirage) = self.tirage.take() else {
            return Effet::Rien;
        };
        let (source, Some(cible)) = (tirage.source, tirage.cible) else {
            return Effet::Rien;
        };
        if source.node == cible.node {
            return Effet::Rien;
        }
        if links
            .iter()
            .any(|l| l.link.src == source && l.link.dst == cible)
        {
            return Effet::Rien;
        }
        if cree_un_cycle(links, source.node, cible.node) {
            return Effet::Boucle {
                depuis: source.node,
                vers: cible.node,
            };
        }
        Effet::Commande(Command::Link {
            src: source,
            dst: cible,
        })
    }
}

// --- Canevas ----------------------------------------------------------------

/// Ce que le canevas retient d'un tirage : la position du curseur, dans le
/// repère de la scène.
///
/// Elle vit **ici** et non dans [`State`] : la suivre par message
/// reconstruirait toute la vue à chaque pixel, alors qu'un
/// [`canvas::Action::request_redraw`] suffit à redessiner la seule courbe qui
/// bouge.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Suivi {
    /// Dernière position connue du curseur, si le canevas en a vu une.
    curseur: Option<Point>,
}

/// Le canevas de la scène : sous les cartes, il dessine les liens et porte
/// l'interaction qui n'appartient à aucun widget.
///
/// Il voit les mouvements et le relâchement que le [`mouse_area`] de l'en-tête
/// ou d'une pastille ne capture pas, ce qui permet de continuer à suivre le
/// curseur quand celui-ci sort de la carte.
#[derive(Debug, Clone, PartialEq)]
pub struct Graphe {
    /// Vrai pendant le déplacement d'une carte.
    deplacement: bool,
    /// Vrai pendant le tirage d'un lien.
    tirage: bool,
    /// Point d'attache du port d'où le lien se tire, si son nœud est placé.
    depart: Option<Point>,
    /// Les liens dessinables, déjà résolus en points d'attache.
    courbes: Vec<Courbe>,
    /// Le lien sélectionné, s'il y en a un.
    selection: Option<LinkId>,
}

impl canvas::Program<Message> for Graphe {
    type State = Suivi;

    fn update(
        &self,
        etat: &mut Suivi,
        evenement: &iced::Event,
        bornes: Rectangle,
        curseur: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        // La position de l'événement est celle de la fenêtre ; la scène a son
        // propre repère, qui commence au coin du canevas.
        let scene = |position: &Point| *position - Vector::new(bornes.x, bornes.y);
        match evenement {
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if self.deplacement {
                    Some(canvas::Action::publish(Message::Patchbay(Geste::Deplace(
                        scene(position),
                    ))))
                } else if self.tirage {
                    // Aucun message : le canevas se redessine seul.
                    etat.curseur = Some(scene(position));
                    Some(canvas::Action::request_redraw())
                } else {
                    None
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let geste = if self.deplacement {
                    Geste::Relache
                } else if self.tirage {
                    Geste::FinLien
                } else {
                    return None;
                };
                Some(canvas::Action::publish(Message::Patchbay(geste)).and_capture())
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if !self.deplacement && !self.tirage =>
            {
                // Le canevas est la couche du bas : la pression qui lui
                // parvient n'a été prise ni par une carte ni par une pastille.
                let point = curseur.position_in(bornes)?;
                let choisi = lien_le_plus_proche(&self.courbes, point);
                Some(
                    canvas::Action::publish(Message::Patchbay(Geste::Selection(choisi)))
                        .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        etat: &Suivi,
        renderer: &Renderer,
        theme: &Theme,
        bornes: Rectangle,
        _curseur: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if self.courbes.is_empty() && self.depart.is_none() {
            return Vec::new();
        }
        let j = jetons(theme);
        let mut frame = canvas::Frame::new(renderer, bornes.size());
        // Dès qu'un lien est choisi, les autres passent au voile d'or. L'accent
        // seul ne suffirait pas : en mode sombre il **est** l'or, et le lien
        // choisi ne se distinguerait que par son épaisseur. Voiler le reste
        // fait ressortir la sélection dans les deux modes, sans introduire une
        // seconde couleur d'accent sur la surface.
        let une_selection = self.selection.is_some();
        for courbe in &self.courbes {
            let choisi = self.selection == Some(courbe.lien);
            frame.stroke(
                &chemin(courbe.depart, courbe.arrivee),
                Stroke {
                    style: canvas::Style::Solid(match (choisi, une_selection) {
                        (true, _) => j.accent_texte,
                        (false, true) => j.voile_d_or,
                        (false, false) => j.or,
                    }),
                    width: if choisi {
                        EPAISSEUR_LIEN_CHOISI
                    } else {
                        EPAISSEUR_LIEN
                    },
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    line_dash: LineDash::default(),
                },
            );
        }
        // Le lien en cours de tirage : garance et pointillés, le temps de
        // trouver une entrée.
        if let (Some(depart), Some(curseur)) = (self.depart, etat.curseur) {
            frame.stroke(
                &chemin(depart, curseur),
                Stroke {
                    style: canvas::Style::Solid(j.garance),
                    width: EPAISSEUR_LIEN,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    line_dash: LineDash {
                        segments: &POINTILLES,
                        offset: 0,
                    },
                },
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _etat: &Suivi,
        _bornes: Rectangle,
        _curseur: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.deplacement {
            mouse::Interaction::Grabbing
        } else if self.tirage {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::None
        }
    }
}

// --- Composition ------------------------------------------------------------

/// Compose la vue : la zone bordée, la scène qui s'y défile, le texte d'aide.
///
/// `graisse` est la graisse du texte courant du mode
/// ([`crate::theme::Jetons::graisse_texte`]).
pub fn view<'a>(mirror: &'a Mirror, state: &'a State, graisse: Weight) -> Element<'a, Message> {
    let contenu: Element<'a, Message> = if mirror.nodes.is_empty() {
        vide(mirror, graisse)
    } else {
        scene(mirror, state, graisse)
    };
    container(column![contenu, aide(graisse)].width(Fill).height(Fill))
        .style(style::zone)
        .width(Fill)
        .height(Fill)
        .into()
}

/// La scène : le canevas dessous, une carte épinglée par nœud dessus.
fn scene<'a>(mirror: &'a Mirror, state: &'a State, graisse: Weight) -> Element<'a, Message> {
    let placements = disposer(&mirror.nodes, &state.positions);
    let etendue = etendue(&placements);
    let graphe = Graphe {
        deplacement: state.deplacement(),
        tirage: state.en_tirage(),
        depart: state
            .tirage
            .and_then(|t| ancre_de_port(t.source, &mirror.nodes, &placements)),
        courbes: courbes(&mirror.links, &mirror.nodes, &placements),
        selection: state.lien_selectionne(&mirror.links),
    };
    let mut couches = stack![canvas(graphe).width(etendue.width).height(etendue.height)];
    for (noeud, placement) in mirror.nodes.iter().zip(&placements) {
        let saisie = state.saisie_de(&placement.cle);
        couches = couches.push(
            pin(carte(noeud, placement, saisie, state, graisse))
                // La carte est épinglée par son coin, pastilles comprises :
                // celles-ci débordent à gauche.
                .x(placement.position.x - DEBORDEMENT)
                .y(placement.position.y),
        );
    }
    scrollable(couches)
        .direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::default(),
            horizontal: scrollable::Scrollbar::default(),
        })
        .width(Fill)
        .height(Fill)
        .style(style::defilement)
        .into()
}

/// Une carte et ses pastilles de ports, dans une couche large de
/// `LARGEUR_CARTE + 2 × DEBORDEMENT`.
///
/// Les pastilles sont épinglées aux coordonnées que rend [`ancre`] : la
/// pastille dessinée et le point d'attache que M2-05 calculera sont donc le
/// même point, par construction.
fn carte<'a>(
    noeud: &'a NodeDescriptor,
    placement: &Placement,
    saisie: bool,
    state: &State,
    graisse: Weight,
) -> Element<'a, Message> {
    let opacite = opacite(noeud);
    // Les pastilles se placent dans le repère de la couche, dont le coin est
    // à `DEBORDEMENT` à gauche de la carte.
    let coin = Placement {
        cle: String::new(),
        position: Point::new(DEBORDEMENT, 0.0),
        hauteur: placement.hauteur,
    };
    let mut couches = stack![container(corps(noeud, saisie, state, opacite, graisse))
        .padding(Padding::ZERO.horizontal(DEBORDEMENT))];
    for (direction, ports) in [
        (Direction::Input, &noeud.inputs),
        (Direction::Output, &noeud.outputs),
    ] {
        for index in 0..ports.len() {
            let centre = ancre(&coin, direction, index);
            couches = couches.push(
                pin(pastille(noeud.id, direction, index, opacite))
                    .x(centre.x - PASTILLE / 2.0)
                    .y(centre.y - PASTILLE / 2.0),
            );
        }
    }
    couches.into()
}

/// La pastille d'un port : un point d'or, et la zone qui écoute la souris.
///
/// Une **sortie** commence un tirage à la pression ; une **entrée** se
/// contente de dire qu'elle est survolée. C'est ce survol — les bornes réelles
/// du widget — qui désigne la cible au relâchement, et non un test
/// d'intersection contre la géométrie recalculée.
fn pastille<'a>(
    noeud: NodeId,
    direction: Direction,
    index: usize,
    opacite: f32,
) -> Element<'a, Message> {
    let port = PortId::new(noeud, direction, index as u16);
    let point = container(space::horizontal())
        .width(PASTILLE)
        .height(PASTILLE)
        .style(style::pastille_voilee(|j| j.or, opacite));
    let zone = match direction {
        Direction::Output => mouse_area(point).on_press(Message::Patchbay(Geste::DebutLien(port))),
        Direction::Input => mouse_area(point)
            .on_enter(Message::Patchbay(Geste::SurvolPort(Some(port))))
            .on_exit(Message::Patchbay(Geste::SurvolPort(None))),
    };
    zone.interaction(mouse::Interaction::Crosshair).into()
}

/// Le corps de la carte : l'en-tête saisissable, son filet, la grille des
/// ports, un second filet et le pied de gain.
fn corps<'a>(
    noeud: &'a NodeDescriptor,
    saisie: bool,
    state: &State,
    opacite: f32,
    graisse: Weight,
) -> Element<'a, Message> {
    let pilote = noeud.state == NodeState::Driver;
    container(column![
        entete(noeud, saisie, opacite, graisse),
        rule::horizontal(FILET).style(style::filet),
        ports(noeud, opacite, graisse),
        rule::horizontal(FILET).style(style::filet),
        pied(noeud, state, opacite, graisse),
    ])
    .id(ID_CARTE)
    .width(LARGEUR_CARTE)
    .style(style::carte_noeud(pilote, opacite))
    .into()
}

/// Le pied de la carte : la glissière de gain, le libellé en décibels et le
/// bouton de coupure.
///
/// Un nœud **suspendu** garde le dessin et perd la main : son bouton est
/// désactivé, et sa glissière — `iced` n'a pas d'état désactivé pour une
/// glissière — est refusée par [`State::reduire`], ce qui revient au même
/// puisque sa valeur vient du miroir et ne bouge donc pas.
fn pied<'a>(
    noeud: &NodeDescriptor,
    state: &State,
    opacite: f32,
    graisse: Weight,
) -> Element<'a, Message> {
    let cible = Cible::Noeud(noeud.id);
    let reglages = reglage(
        Reglage {
            cible,
            position: state.position_de(cible, noeud.gain_db),
            affiche: state.gain_affiche(cible, noeud.gain_db),
            muted: noeud.muted,
            actif: noeud.state != NodeState::Suspended,
            opacite,
            largeur: Length::Fill,
            // Le pied vit sur une surface posée, comme le reste de la carte.
            encre: |j| j.texte_2_carte,
        },
        graisse,
    );
    container(reglages)
        .padding(PADDING_PIED)
        .width(Fill)
        .height(HAUTEUR_PIED)
        .into()
}

/// Le réglage de gain du lien sélectionné, tel que l'en-tête le porte.
///
/// La maquette ne lui donne pas de place sur la scène — un lien est une courbe,
/// pas une carte —, mais F-13 l'exige : il est donc posé là où le lien est déjà
/// l'objet courant, à côté de « Supprimer le lien ». Il disparaît avec la
/// sélection.
pub fn reglage_de_lien<'a>(
    lien: &LinkDescriptor,
    state: &State,
    actif: bool,
    graisse: Weight,
) -> Element<'a, Message> {
    let cible = Cible::Lien(lien.link.id);
    container(reglage(
        Reglage {
            cible,
            position: state.position_de(cible, lien.gain_db),
            affiche: state.gain_affiche(cible, lien.gain_db),
            muted: lien.muted,
            actif,
            opacite: 1.0,
            largeur: Length::Fixed(LARGEUR_GLISSIERE_ENTETE),
            // L'en-tête, lui, est sur le fond de la fenêtre.
            encre: |j| j.texte_2,
        },
        graisse,
    ))
    // La hauteur d'un bouton, pour s'aligner sur les autres actions.
    .height(style::HAUTEUR_BOUTON)
    .align_y(Center)
    .into()
}

/// Ce qu'un réglage de gain montre, et de quoi il dispose.
struct Reglage {
    /// Nœud ou lien réglé.
    cible: Cible,
    /// Position de la glissière, en décibels.
    position: f32,
    /// Gain écrit par le libellé.
    affiche: Db,
    /// État coupé dessiné par le bouton.
    muted: bool,
    /// Faux quand le réglage n'a pas la main : nœud suspendu, démon absent.
    actif: bool,
    /// Voile de la carte qui le porte (voir [`style::carte_noeud`]).
    opacite: f32,
    /// Largeur donnée à la glissière ; le reste est de taille fixe.
    largeur: Length,
    /// Encre du libellé, selon la surface qui le porte.
    encre: fn(&Jetons) -> Color,
}

/// Le dessin d'un réglage de gain : la glissière, le libellé, le bouton.
///
/// La glissière n'envoie **rien** pendant le geste : elle publie une position,
/// et c'est son relâchement qui parle au démon (voir le module).
fn reglage<'a>(r: Reglage, graisse: Weight) -> Element<'a, Message> {
    let cible = r.cible;
    let glissiere = slider(GAIN_MIN..=GAIN_MAX, r.position, move |position| {
        Message::Patchbay(Geste::Gain(cible, position))
    })
    .on_release(Message::Patchbay(Geste::FinGain(cible)))
    .step(GAIN_PAS)
    .width(r.largeur)
    .height(HAUTEUR_GLISSIERE)
    .style(style::curseur_voile(r.opacite));

    let libelle = text(format::decibels(r.affiche))
        .size(CORPS_GAIN)
        .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
        .font(typo::texte_a(graisse, CORPS_GAIN))
        .wrapping(Wrapping::None)
        .width(LARGEUR_GAIN)
        .align_x(Right)
        .style(style::texte_voile(r.encre, r.opacite));

    let coupure = button(
        text(i18n::t(Text::Mute))
            .size(CORPS_GAIN)
            .font(typo::interface())
            .width(Fill)
            .height(Fill)
            .align_x(Center)
            .align_y(Center),
    )
    .width(LARGEUR_MUET)
    .height(HAUTEUR_REGLAGE)
    .padding(0)
    .on_press_maybe(r.actif.then_some(Message::Patchbay(Geste::Muet(cible))))
    .style(style::bouton_muet(r.muted));

    row![glissiere, libelle, coupure]
        .spacing(ESPACE_S)
        .align_y(Center)
        .into()
}

/// L'en-tête : le libellé à gauche, l'étiquette d'état à droite.
///
/// C'est la poignée de la carte : la pression y est capturée, les mouvements
/// et le relâchement filent au canevas (voir le module).
fn entete<'a>(
    noeud: &'a NodeDescriptor,
    saisie: bool,
    opacite: f32,
    graisse: Weight,
) -> Element<'a, Message> {
    let etiquette = etiquette(noeud);
    let encre: fn(&crate::theme::Jetons) -> iced::Color = if etiquette.accentuee() {
        |j| j.accent_texte
    } else {
        |j| j.texte_2_carte
    };
    let cle = noeud.key.to_string();
    let poignee = container(
        row![
            container(
                text(noeud.label.as_str())
                    .size(CORPS_INTERFACE)
                    .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
                    .font(typo::texte_a(graisse, CORPS_INTERFACE))
                    .wrapping(Wrapping::None)
                    .style(style::texte_voile(|j| j.titre, opacite)),
            )
            .width(Fill)
            .clip(true),
            container(typo::petites_capitales(
                etiquette.libelle(),
                CORPS_ETIQUETTE
            ))
            .style(style::encre_voilee(encre, opacite)),
        ]
        .spacing(ESPACE_S)
        .align_y(Center),
    )
    .height(HAUTEUR_ENTETE)
    .padding(Padding::ZERO.horizontal(PADDING_ENTETE))
    .width(Fill);
    mouse_area(poignee)
        .on_press(Message::Patchbay(Geste::Saisi(cle)))
        .interaction(if saisie {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        })
        .into()
}

/// La grille des ports : les entrées à gauche, les sorties à droite.
fn ports<'a>(noeud: &'a NodeDescriptor, opacite: f32, graisse: Weight) -> Element<'a, Message> {
    let colonne = |ports: &'a [conduit_core::node::PortSpec], droite: bool| {
        let mut colonne = column![].width(Fill);
        for port in ports {
            let nom = text(port.name.as_str())
                .size(CORPS_PORT)
                .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
                .font(typo::texte_a(graisse, CORPS_PORT))
                .wrapping(Wrapping::None)
                .style(style::texte_voile(|j| j.texte_2_carte, opacite));
            colonne = colonne.push(
                container(nom)
                    .height(HAUTEUR_PORT)
                    .width(Fill)
                    .align_x(if droite { iced::Right } else { iced::Left })
                    .align_y(Center)
                    .clip(true),
            );
        }
        colonne
    };
    container(row![
        colonne(&noeud.inputs, false),
        colonne(&noeud.outputs, true),
    ])
    .padding(
        Padding::ZERO
            .vertical(MARGE_PORTS)
            .horizontal(PADDING_PORTS),
    )
    .width(Fill)
    .into()
}

/// Opacité de la carte : un nœud suspendu s'efface sans disparaître.
fn opacite(noeud: &NodeDescriptor) -> f32 {
    if noeud.state == NodeState::Suspended {
        style::OPACITE_SUSPENDU
    } else {
        1.0
    }
}

/// Le texte d'aide, en bas à gauche de la zone.
///
/// Il dit les trois gestes de la scène. Il est posé **hors** du défilement,
/// pour rester lisible quelle que soit la position de la scène.
fn aide<'a>(graisse: Weight) -> Element<'a, Message> {
    container(
        text(i18n::t(Text::PatchbayHint))
            .size(CORPS_PORT)
            .font(typo::texte_a(graisse, CORPS_PORT))
            .style(style::texte_en(|j| j.texte_2)),
    )
    .padding(PADDING_AIDE)
    .into()
}

/// La zone quand le graphe n'a aucun nœud, ou que le démon n'a pas répondu.
fn vide<'a>(mirror: &'a Mirror, graisse: Weight) -> Element<'a, Message> {
    let quoi = if mirror.is_loaded() {
        Text::PatchbayEmpty
    } else {
        Text::Waiting
    };
    container(
        text(i18n::t(quoi))
            .size(CORPS_INTERFACE)
            .font(typo::texte_a(graisse, CORPS_INTERFACE))
            .style(style::texte_en(|j| j.texte_2)),
    )
    .padding(PADDING_VIDE)
    .width(Fill)
    .height(Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use conduit_backend::{DeviceDirection, DeviceId, DeviceInfo};
    use conduit_core::graph::{LinkInfo, NodeId};
    use conduit_core::node::PortSpec;
    use conduit_core::types::{Db, SampleRate};

    /// Un identifiant de nœud, du rang donné.
    fn id(rang: u32) -> NodeId {
        NodeId::new(rang, 0)
    }

    /// Un lien du premier port de sortie de `src` au premier port d'entrée de
    /// `dst`.
    fn lien(rang: u32, src: NodeId, dst: NodeId) -> LinkDescriptor {
        LinkDescriptor {
            link: LinkInfo {
                id: LinkId::new(rang, 0),
                src: PortId::new(src, Direction::Output, 0),
                dst: PortId::new(dst, Direction::Input, 0),
            },
            gain_db: Db::UNITY,
            muted: false,
        }
    }

    /// Un nœud interne du rang donné : [`noeud`] les crée tous au rang 0, ce
    /// qui suffit tant qu'un test ne parle pas d'un nœud en particulier.
    fn noeud_n(rang: u32, nom: &str, entrees: usize, sorties: usize) -> NodeDescriptor {
        NodeDescriptor {
            id: id(rang),
            ..noeud(nom, entrees, sorties)
        }
    }

    /// Un nœud interne à `entrees` entrées et `sorties` sorties.
    fn noeud(nom: &str, entrees: usize, sorties: usize) -> NodeDescriptor {
        NodeDescriptor {
            id: NodeId::new(0, 0),
            key: NodeKey::internal(nom),
            label: nom.into(),
            type_name: "test".into(),
            inputs: PortSpec::layout(entrees),
            outputs: PortSpec::layout(sorties),
            state: NodeState::Internal,
            gain_db: Db::UNITY,
            muted: false,
            device: None,
        }
    }

    /// Un périphérique, câble de Conduit ou non.
    fn peripherique(nom: &str, cable: Option<u32>) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(nom),
            name: nom.into(),
            direction: DeviceDirection::Render,
            channels: 2,
            sample_rate: SampleRate::HZ_48000,
            sample_rates: vec![],
            default_block: 480,
            is_default: false,
            cable: cable.map(conduit_backend::CableId),
        }
    }

    /// Le rôle se lit dans les ports ; le pilote va à droite quoi qu'il ait.
    #[test]
    fn le_role_se_deduit_des_ports() {
        assert_eq!(role(&noeud("lecteur", 0, 2)), Role::Source);
        assert_eq!(role(&noeud("câble", 2, 2)), Role::Intermediaire);
        assert_eq!(role(&noeud("hp", 2, 0)), Role::Puits);
        // Sans aucun port : ni entrée ni sortie, c'est un puits.
        assert_eq!(role(&noeud("muet", 0, 0)), Role::Puits);
        let mut pilote = noeud("carte son", 2, 2);
        pilote.state = NodeState::Driver;
        assert_eq!(role(&pilote), Role::Puits, "le pilote va à droite");
        assert_eq!(Role::ALL.map(Role::colonne), [0, 1, 2]);
    }

    /// L'étiquette se lit dans l'état puis dans le périphérique.
    #[test]
    fn l_etiquette_se_deduit_du_descripteur() {
        let mut n = noeud("sinus", 0, 2);
        assert_eq!(etiquette(&n), Etiquette::Utilitaire);
        n.device = Some(peripherique("hp", None));
        assert_eq!(etiquette(&n), Etiquette::Materiel);
        n.device = Some(peripherique("conduit1", Some(1)));
        assert_eq!(etiquette(&n), Etiquette::Cable);
        // L'état l'emporte sur la nature.
        n.state = NodeState::Suspended;
        assert_eq!(etiquette(&n), Etiquette::Suspendu);
        n.state = NodeState::Driver;
        assert_eq!(etiquette(&n), Etiquette::Pilote);
        assert_eq!(
            [
                Etiquette::Pilote,
                Etiquette::Suspendu,
                Etiquette::Cable,
                Etiquette::Materiel,
                Etiquette::Utilitaire,
            ]
            .map(Etiquette::libelle),
            ["Pilote", "Suspendu", "Câble", "Matériel", "Utilitaire"]
        );
        assert!(Etiquette::Pilote.accentuee() && Etiquette::Suspendu.accentuee());
        assert!(!Etiquette::Cable.accentuee());
    }

    /// La hauteur suit la plus fournie des deux colonnes de ports, et porte
    /// toujours l'en-tête, ses deux filets et le pied de gain.
    #[test]
    fn la_hauteur_suit_la_colonne_la_plus_fournie() {
        let nue = HAUTEUR_ENTETE + FILET + 2.0 * MARGE_PORTS + FILET + HAUTEUR_PIED;
        assert_eq!(hauteur_carte(0, 0), nue);
        assert_eq!(hauteur_carte(2, 0), nue + 2.0 * HAUTEUR_PORT);
        assert_eq!(hauteur_carte(0, 2), nue + 2.0 * HAUTEUR_PORT);
        assert_eq!(hauteur_carte(1, 3), hauteur_carte(3, 1));
        // Le pied compte pour de bon : une carte sans port n'est pas réduite
        // à son en-tête.
        assert!(hauteur_carte(0, 0) > HAUTEUR_ENTETE + FILET + 2.0 * MARGE_PORTS);
    }

    /// Une entrée s'attache à gauche, une sortie à droite, et chaque rang
    /// descend d'une ligne de port.
    #[test]
    fn les_ancres_sont_a_gauche_et_a_droite_et_descendent_par_rang() {
        let p = Placement {
            cle: "internal:a".into(),
            position: Point::new(100.0, 50.0),
            hauteur: hauteur_carte(2, 2),
        };
        let e0 = ancre(&p, Direction::Input, 0);
        let e1 = ancre(&p, Direction::Input, 1);
        let s0 = ancre(&p, Direction::Output, 0);
        assert!(e0.x < p.position.x, "l'entrée déborde à gauche");
        assert!(
            s0.x > p.position.x + LARGEUR_CARTE,
            "la sortie déborde à droite"
        );
        // Les deux pastilles débordent d'autant, chacune de son côté.
        assert_eq!(p.position.x - e0.x, DEBORDEMENT - PASTILLE / 2.0);
        assert_eq!(
            s0.x - (p.position.x + LARGEUR_CARTE),
            DEBORDEMENT - PASTILLE / 2.0
        );
        assert_eq!(e0.y, s0.y, "les deux colonnes sont à la même hauteur");
        assert_eq!(e1.y - e0.y, HAUTEUR_PORT);
        // La première ancre tombe au milieu de sa ligne, sous l'en-tête.
        assert_eq!(
            e0.y,
            p.position.y + HAUTEUR_ENTETE + FILET + MARGE_PORTS + HAUTEUR_PORT / 2.0
        );
    }

    /// Trois colonnes, une par rôle, et un empilement vertical régulier.
    #[test]
    fn la_disposition_range_les_noeuds_en_colonnes_par_role() {
        let noeuds = vec![
            noeud("hp", 2, 0),
            noeud("lecteur", 0, 2),
            noeud("câble", 2, 2),
            noeud("micro", 0, 1),
        ];
        let p = disposer(&noeuds, &Positions::default());
        let x = |i: usize| p[i].position.x;
        assert_eq!(x(1), x(3), "les deux sources partagent leur colonne");
        assert!(x(1) < x(2) && x(2) < x(0), "source, milieu, puits");
        assert_eq!(x(2) - x(1), LARGEUR_CARTE + ECART_COLONNES);
        // Dans la colonne des sources, « lecteur » précède « micro » : à état
        // égal, le tri est alphabétique.
        assert_eq!(p[1].position.y, MARGE_SCENE);
        assert_eq!(
            p[3].position.y,
            MARGE_SCENE + hauteur_carte(0, 2) + ECART_CARTES
        );
        // Les cartes seules de leur colonne sont en haut.
        assert_eq!(p[0].position.y, MARGE_SCENE);
        assert_eq!(p[2].position.y, MARGE_SCENE);
        assert_eq!(p[0].hauteur, hauteur_carte(2, 0));
        assert_eq!(p[0].cle, "internal:hp");
    }

    /// L'ordre du démon ne change pas le créneau : le tri est le sien.
    #[test]
    fn la_disposition_est_deterministe_et_ne_suit_pas_l_ordre_recu() {
        let a = noeud("alpha", 0, 2);
        let b = noeud("bravo", 0, 2);
        let dans_l_ordre = disposer(&[a.clone(), b.clone()], &Positions::default());
        let a_l_envers = disposer(&[b, a], &Positions::default());
        assert_eq!(dans_l_ordre[0].position, a_l_envers[1].position);
        assert_eq!(dans_l_ordre[1].position, a_l_envers[0].position);
        // Deux appels de suite rendent exactement la même chose.
        let noeuds = vec![noeud("x", 0, 1), noeud("y", 1, 1), noeud("z", 1, 0)];
        assert_eq!(
            disposer(&noeuds, &Positions::default()),
            disposer(&noeuds, &Positions::default())
        );
    }

    /// Une position mémorisée l'emporte sur le créneau, et les clés dont
    /// aucun nœud ne répond sont conservées.
    #[test]
    fn une_position_memorisee_l_emporte_et_les_cles_inconnues_restent() {
        let noeuds = vec![noeud("a", 0, 1), noeud("b", 0, 1)];
        let mut positions = Positions::default();
        positions.set("internal:a", Point::new(640.0, 320.0));
        positions.set("internal:disparu", Point::new(10.0, 10.0));
        let p = disposer(&noeuds, &positions);
        assert_eq!(p[0].position, Point::new(640.0, 320.0));
        // Le créneau de « b » ne bouge pas parce que « a » a été déplacé.
        assert_eq!(
            p[1].position,
            disposer(&noeuds, &Positions::default())[1].position
        );
        assert_eq!(positions.len(), 2, "la clé sans nœud est conservée");
        assert!(positions.get("internal:disparu").is_some());
    }

    /// L'étendue contient toutes les cartes, pastilles et marge comprises.
    #[test]
    fn l_etendue_contient_toutes_les_cartes() {
        let noeuds = vec![noeud("a", 0, 1), noeud("b", 1, 0)];
        let placements = disposer(&noeuds, &Positions::default());
        let taille = etendue(&placements);
        for p in &placements {
            assert!(p.position.x + LARGEUR_CARTE + DEBORDEMENT <= taille.width);
            assert!(p.position.y + p.hauteur <= taille.height);
        }
        assert_eq!(etendue(&[]), Size::new(0.0, 0.0));
    }

    /// La fusion garde les clés absentes des nouvelles positions.
    #[test]
    fn la_fusion_conserve_les_cles_absentes() {
        let mut ancien = Positions::default();
        ancien.set("internal:a", Point::new(1.0, 2.0));
        ancien.set("internal:vieux", Point::new(3.0, 4.0));
        let mut neuf = Positions::default();
        neuf.set("internal:a", Point::new(9.0, 9.0));
        let fusion = ancien.fusion(&neuf);
        assert_eq!(fusion.get("internal:a"), Some(Point::new(9.0, 9.0)));
        assert_eq!(fusion.get("internal:vieux"), Some(Point::new(3.0, 4.0)));
        assert_eq!(fusion.len(), 2);
    }

    /// Aller-retour JSON : les positions se relisent telles quelles.
    #[test]
    fn les_positions_font_l_aller_retour_json() {
        let mut positions = Positions::default();
        positions.set("wasapi:{0.0.0}", Point::new(24.0, 48.5));
        positions.set("internal:mix", Point::new(0.0, 0.0));
        let json = serde_json::to_string(&positions).expect("sérialisation");
        assert!(json.starts_with('{'), "un objet, pas un tableau : {json}");
        let relues: Positions = serde_json::from_str(&json).expect("désérialisation");
        assert_eq!(relues, positions);
        assert!(Positions::default().is_empty());
    }

    /// Saisie, déplacement, relâchement : la carte ne bouge qu'à partir du
    /// second mouvement, et seul un vrai déplacement demande l'écriture.
    #[test]
    fn le_geste_de_deplacement_se_reduit_en_trois_temps() {
        let noeuds = vec![noeud("a", 0, 1)];
        let placements = disposer(&noeuds, &Positions::default());
        let depart = placements[0].position;
        let mut state = State::default();

        assert_eq!(
            state.reduire(Geste::Saisi("internal:a".into()), &placements, &[], &[]),
            Effet::Rien
        );
        assert!(state.deplacement() && state.saisie_de("internal:a"));
        // Le premier mouvement ne fait que noter l'écart au curseur.
        let curseur = Point::new(depart.x + 30.0, depart.y + 12.0);
        assert_eq!(
            state.reduire(Geste::Deplace(curseur), &placements, &[], &[]),
            Effet::Rien
        );
        assert!(state.positions.is_empty(), "la carte n'a pas encore bougé");
        // Le suivant emmène la carte, écart conservé.
        assert_eq!(
            state.reduire(
                Geste::Deplace(Point::new(curseur.x + 100.0, curseur.y + 40.0)),
                &placements,
                &[],
                &[]
            ),
            Effet::Rien
        );
        assert_eq!(
            state.positions.get("internal:a"),
            Some(Point::new(depart.x + 100.0, depart.y + 40.0))
        );
        // Le relâchement ferme la saisie et demande l'écriture.
        assert_eq!(
            state.reduire(Geste::Relache, &placements, &[], &[]),
            Effet::Enregistrer
        );
        assert!(!state.deplacement());
        assert_eq!(
            state.reduire(Geste::Relache, &placements, &[], &[]),
            Effet::Rien,
            "plus rien à écrire"
        );
    }

    /// Une carte glissée hors de la scène est retenue au bord.
    #[test]
    fn une_carte_ne_sort_pas_par_le_haut_ni_par_la_gauche() {
        let noeuds = vec![noeud("a", 0, 1)];
        let placements = disposer(&noeuds, &Positions::default());
        let mut state = State::default();
        state.reduire(Geste::Saisi("internal:a".into()), &placements, &[], &[]);
        state.reduire(
            Geste::Deplace(placements[0].position),
            &placements,
            &[],
            &[],
        );
        state.reduire(
            Geste::Deplace(Point::new(-500.0, -500.0)),
            &placements,
            &[],
            &[],
        );
        assert_eq!(state.positions.get("internal:a"), Some(Point::ORIGIN));
    }

    /// Une clé inconnue ne saisit rien, et un mouvement sans saisie n'a aucun
    /// effet.
    #[test]
    fn un_geste_sans_carte_est_sans_effet() {
        let mut state = State::default();
        assert_eq!(
            state.reduire(Geste::Saisi("internal:fantôme".into()), &[], &[], &[]),
            Effet::Rien
        );
        assert!(!state.deplacement());
        assert_eq!(
            state.reduire(Geste::Deplace(Point::new(10.0, 10.0)), &[], &[], &[]),
            Effet::Rien
        );
        assert!(state.positions.is_empty());
        assert_eq!(state.reduire(Geste::Relache, &[], &[], &[]), Effet::Rien);
    }

    /// Les points de contrôle sortent horizontalement, d'au moins la tension
    /// minimale.
    #[test]
    fn les_points_de_controle_sortent_a_l_horizontale() {
        let a = Point::new(100.0, 50.0);
        let b = Point::new(500.0, 200.0);
        let (c1, c2) = controles(a, b);
        assert_eq!(
            c1.y, a.y,
            "le contrôle de départ reste à la hauteur du port"
        );
        assert_eq!(c2.y, b.y);
        let dx = (b.x - a.x).abs() * TENSION;
        assert_eq!(c1.x, a.x + dx);
        assert_eq!(c2.x, b.x - dx);
        // Deux ports presque alignés gardent une vraie courbe.
        let (c1, c2) = controles(a, Point::new(a.x + 4.0, a.y + 30.0));
        assert_eq!(c1.x, a.x + TENSION_MINIMALE);
        assert_eq!(c2.x, a.x + 4.0 - TENSION_MINIMALE);
        // Un lien qui remonte le graphe garde le même sens de sortie.
        let (c1, _) = controles(a, Point::new(a.x - 300.0, a.y));
        assert!(c1.x > a.x);
    }

    /// La courbe part du port et y arrive ; son milieu passe entre les deux.
    #[test]
    fn la_courbe_joint_ses_deux_extremites() {
        let a = Point::new(100.0, 50.0);
        let b = Point::new(400.0, 250.0);
        assert_eq!(point_de_courbe(a, b, 0.0), a);
        assert_eq!(point_de_courbe(a, b, 1.0), b);
        let milieu = point_de_courbe(a, b, 0.5);
        assert!((milieu.x - (a.x + b.x) / 2.0).abs() < 0.01);
        assert!((milieu.y - (a.y + b.y) / 2.0).abs() < 0.01);
    }

    /// Un clic sur la courbe la touche ; à 20 px, non.
    #[test]
    fn le_clic_ne_touche_la_courbe_que_de_pres() {
        let a = Point::new(100.0, 50.0);
        let b = Point::new(400.0, 250.0);
        let sur = point_de_courbe(a, b, 0.3);
        assert!(distance_a_la_courbe(a, b, sur) < 1.0);
        let a_cote = Point::new(sur.x, sur.y + 20.0);
        assert!(distance_a_la_courbe(a, b, a_cote) > SEUIL_CLIC);

        let courbes = vec![Courbe {
            lien: LinkId::new(7, 0),
            depart: a,
            arrivee: b,
        }];
        assert_eq!(lien_le_plus_proche(&courbes, sur), Some(LinkId::new(7, 0)));
        assert_eq!(lien_le_plus_proche(&courbes, a_cote), None);
        assert_eq!(lien_le_plus_proche(&[], sur), None);
    }

    /// Les courbes se calent sur les ancres, et un lien dont un nœud manque
    /// n'est ni dessiné ni fatal.
    #[test]
    fn un_lien_sans_noeud_n_est_pas_dessine_et_ne_panique_pas() {
        let noeuds = vec![noeud("a", 0, 1), noeud("b", 1, 0)];
        let noeuds: Vec<NodeDescriptor> = noeuds
            .into_iter()
            .enumerate()
            .map(|(i, mut n)| {
                n.id = id(i as u32);
                n
            })
            .collect();
        let placements = disposer(&noeuds, &Positions::default());
        let liens = vec![lien(0, id(0), id(1)), lien(1, id(0), id(9))];
        let dessinees = courbes(&liens, &noeuds, &placements);
        assert_eq!(dessinees.len(), 1, "le lien vers un nœud absent est ignoré");
        assert_eq!(dessinees[0].lien, LinkId::new(0, 0));
        assert_eq!(
            dessinees[0].depart,
            ancre(&placements[0], Direction::Output, 0)
        );
        assert_eq!(
            dessinees[0].arrivee,
            ancre(&placements[1], Direction::Input, 0)
        );
        // Sans aucun placement, rien n'est dessiné et rien ne panique.
        assert!(courbes(&liens, &noeuds, &[]).is_empty());
    }

    /// La boucle se voit directement, par un détour, ou pas du tout.
    #[test]
    fn le_cycle_se_lit_dans_le_graphe_des_liens() {
        // Graphe vide : aucun lien ne boucle.
        assert!(!cree_un_cycle(&[], id(0), id(1)));
        // Direct : a → b existe, b → a boucle.
        let direct = vec![lien(0, id(0), id(1))];
        assert!(cree_un_cycle(&direct, id(1), id(0)));
        assert!(!cree_un_cycle(&direct, id(0), id(1)));
        // Indirect : a → b → c, c → a boucle.
        let indirect = vec![lien(0, id(0), id(1)), lien(1, id(1), id(2))];
        assert!(cree_un_cycle(&indirect, id(2), id(0)));
        assert!(!cree_un_cycle(&indirect, id(0), id(2)));
        // Un nœud vers lui-même est une boucle.
        assert!(cree_un_cycle(&[], id(0), id(0)));
        // Une branche parallèle n'est pas une boucle.
        let fourche = vec![lien(0, id(0), id(1)), lien(1, id(0), id(2))];
        assert!(!cree_un_cycle(&fourche, id(1), id(2)));
    }

    /// Le tirage complet : pression sur une sortie, survol d'une entrée,
    /// relâchement — et la commande qui va avec.
    #[test]
    fn le_tirage_d_un_lien_se_reduit_en_trois_temps() {
        let mut state = State::default();
        let source = PortId::new(id(0), Direction::Output, 1);
        let cible = PortId::new(id(1), Direction::Input, 0);

        assert_eq!(
            state.reduire(Geste::DebutLien(source), &[], &[], &[]),
            Effet::Rien
        );
        assert!(state.en_tirage());
        assert_eq!(
            state.reduire(Geste::SurvolPort(Some(cible)), &[], &[], &[]),
            Effet::Rien
        );
        assert_eq!(
            state.reduire(Geste::FinLien, &[], &[], &[]),
            Effet::Commande(Command::Link {
                src: source,
                dst: cible
            })
        );
        assert!(!state.en_tirage(), "le tirage se referme au relâchement");
    }

    /// Relâché dans le vide, ou tiré depuis une entrée, le lien ne dit rien.
    #[test]
    fn un_tirage_sans_cible_n_emet_rien() {
        let mut state = State::default();
        let source = PortId::new(id(0), Direction::Output, 0);
        state.reduire(Geste::DebutLien(source), &[], &[], &[]);
        state.reduire(
            Geste::SurvolPort(Some(PortId::new(id(1), Direction::Input, 0))),
            &[],
            &[],
            &[],
        );
        // Le curseur quitte la pastille avant le relâchement.
        state.reduire(Geste::SurvolPort(None), &[], &[], &[]);
        assert_eq!(state.reduire(Geste::FinLien, &[], &[], &[]), Effet::Rien);
        assert!(!state.en_tirage());
        // Un relâchement sans tirage ne fait rien non plus.
        assert_eq!(state.reduire(Geste::FinLien, &[], &[], &[]), Effet::Rien);
        // Une pastille d'entrée ne commence pas de tirage.
        state.reduire(
            Geste::DebutLien(PortId::new(id(0), Direction::Input, 0)),
            &[],
            &[],
            &[],
        );
        assert!(!state.en_tirage());
    }

    /// Trois refus : le nœud vers lui-même, le doublon exact, et la boucle —
    /// seule celle-ci se dit.
    #[test]
    fn les_liens_impossibles_sont_refuses_avant_le_demon() {
        let liens = vec![lien(0, id(0), id(1))];
        let mut state = State::default();

        // Un nœud vers lui-même : muet.
        let sortie = PortId::new(id(0), Direction::Output, 0);
        state.reduire(Geste::DebutLien(sortie), &[], &[], &liens);
        state.reduire(
            Geste::SurvolPort(Some(PortId::new(id(0), Direction::Input, 0))),
            &[],
            &[],
            &liens,
        );
        assert_eq!(state.reduire(Geste::FinLien, &[], &[], &liens), Effet::Rien);

        // Le doublon exact : muet lui aussi.
        state.reduire(Geste::DebutLien(sortie), &[], &[], &liens);
        state.reduire(
            Geste::SurvolPort(Some(PortId::new(id(1), Direction::Input, 0))),
            &[],
            &[],
            &liens,
        );
        assert_eq!(state.reduire(Geste::FinLien, &[], &[], &liens), Effet::Rien);

        // La boucle : refusée, et dite.
        state.reduire(
            Geste::DebutLien(PortId::new(id(1), Direction::Output, 0)),
            &[],
            &[],
            &liens,
        );
        state.reduire(
            Geste::SurvolPort(Some(PortId::new(id(0), Direction::Input, 0))),
            &[],
            &[],
            &liens,
        );
        assert_eq!(
            state.reduire(Geste::FinLien, &[], &[], &liens),
            Effet::Boucle {
                depuis: id(1),
                vers: id(0)
            }
        );
    }

    /// La sélection bascule, et Suppr retire le lien sélectionné.
    #[test]
    fn la_selection_bascule_et_suppr_retire_le_lien() {
        let liens = vec![lien(0, id(0), id(1)), lien(1, id(1), id(2))];
        let mut state = State::default();
        // Sans sélection, Suppr n'émet rien.
        assert_eq!(
            state.reduire(Geste::Supprimer, &[], &[], &liens),
            Effet::Rien
        );

        state.reduire(Geste::Selection(Some(LinkId::new(0, 0))), &[], &[], &liens);
        assert_eq!(state.lien_selectionne(&liens), Some(LinkId::new(0, 0)));
        assert_eq!(
            state.reduire(Geste::Supprimer, &[], &[], &liens),
            Effet::Commande(Command::Unlink {
                link: LinkId::new(0, 0)
            })
        );
        // Un second clic sur le même lien le désélectionne.
        state.reduire(Geste::Selection(Some(LinkId::new(0, 0))), &[], &[], &liens);
        assert_eq!(state.lien_selectionne(&liens), None);
        // Un clic à l'écart désélectionne aussi.
        state.reduire(Geste::Selection(Some(LinkId::new(1, 0))), &[], &[], &liens);
        state.reduire(Geste::Selection(None), &[], &[], &liens);
        assert_eq!(state.lien_selectionne(&liens), None);
        // Un lien que le démon a retiré n'est plus sélectionné.
        state.reduire(Geste::Selection(Some(LinkId::new(1, 0))), &[], &[], &liens);
        assert_eq!(state.lien_selectionne(&[]), None);
        assert_eq!(state.reduire(Geste::Supprimer, &[], &[], &[]), Effet::Rien);
    }

    /// Le générateur de test prend un nom libre et un niveau prudent.
    #[test]
    fn le_generateur_de_test_prend_un_nom_libre() {
        assert_eq!(nom_de_generateur(&[]), "Générateur de test");
        let mut pris = noeud("Générateur de test", 0, 2);
        pris.key = NodeKey::internal("Générateur de test");
        assert_eq!(nom_de_generateur(&[pris.clone()]), "Générateur de test 2");
        let mut second = pris.clone();
        second.key = NodeKey::internal("Générateur de test 2");
        assert_eq!(nom_de_generateur(&[pris, second]), "Générateur de test 3");
        // −12 dBFS : audible sans danger pour de vraies enceintes.
        assert!((amplitude_test() - 0.2512).abs() < 0.001);
        assert!(amplitude_test() < 1.0);
    }

    /// La glissière et le gain se traduisent l'un dans l'autre, bornes
    /// comprises — et la butée basse vaut **silence**, pas −60 dB.
    #[test]
    fn la_glissiere_et_le_gain_se_traduisent_dans_les_deux_sens() {
        // Aller : du gain à la position.
        assert_eq!(position_de_gain(Db::UNITY), 0.0);
        assert_eq!(position_de_gain(Db::new(-12.0)), -12.0);
        assert_eq!(position_de_gain(Db::NEG_INF), GAIN_MIN);
        // Hors course : ramené à la butée, dans les deux sens.
        assert_eq!(position_de_gain(Db::new(Db::MAX)), GAIN_MAX);
        assert_eq!(position_de_gain(Db::new(-90.0)), GAIN_MIN);

        // Retour : de la position au gain.
        assert_eq!(gain_de_position(0.0), Db::UNITY);
        assert_eq!(gain_de_position(GAIN_MAX), Db::new(12.0));
        assert_eq!(gain_de_position(-12.0), Db::new(-12.0));
        // La butée basse est un silence, et non un gain de −60 dB.
        assert_eq!(gain_de_position(GAIN_MIN), Db::NEG_INF);
        assert!(gain_de_position(GAIN_MIN).is_silent());
        assert_ne!(gain_de_position(GAIN_MIN), Db::new(GAIN_MIN));
        // Un pixel au-dessus de la butée, ce n'est déjà plus le silence.
        assert_eq!(gain_de_position(-59.0), Db::new(-59.0));
        // Arrondi au pas de la glissière, et bornes tenues hors course.
        assert_eq!(gain_de_position(-3.4), Db::new(-3.0));
        assert_eq!(gain_de_position(-3.6), Db::new(-4.0));
        assert_eq!(gain_de_position(48.0), Db::new(GAIN_MAX));
        assert_eq!(gain_de_position(-300.0), Db::NEG_INF);

        // Aller-retour sur une valeur du pas : rien ne se perd.
        for db in [Db::UNITY, Db::new(-24.0), Db::new(GAIN_MAX), Db::NEG_INF] {
            assert_eq!(gain_de_position(position_de_gain(db)), db, "{db}");
        }
    }

    /// La glissière ne parle au démon qu'au relâchement : les cent messages du
    /// geste ne font que déplacer la valeur montrée.
    #[test]
    fn le_gain_n_est_envoye_qu_au_relachement() {
        let noeuds = vec![noeud_n(0, "a", 0, 1)];
        let cible = Cible::Noeud(id(0));
        let mut state = State::default();

        for position in [-2.0, -8.0, -12.0] {
            assert_eq!(
                state.reduire(Geste::Gain(cible, position), &[], &noeuds, &[]),
                Effet::Rien,
                "rien ne part pendant le geste"
            );
        }
        assert_eq!(state.glissement.map(|g| g.position), Some(-12.0));
        assert_eq!(
            state.reduire(Geste::FinGain(cible), &[], &noeuds, &[]),
            Effet::Commande(Command::SetNodeGain {
                node: id(0),
                gain_db: Some(Db::new(-12.0)),
                muted: None,
            })
        );

        // La butée basse envoie le silence, et un lien passe par la même
        // porte.
        let liens = vec![lien(0, id(0), id(1))];
        let cible = Cible::Lien(LinkId::new(0, 0));
        state.reduire(Geste::Gain(cible, GAIN_MIN), &[], &noeuds, &liens);
        assert_eq!(
            state.reduire(Geste::FinGain(cible), &[], &noeuds, &liens),
            Effet::Commande(Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: Some(Db::NEG_INF),
                muted: None,
            })
        );

        // Un relâchement qu'aucun geste n'a précédé n'envoie rien.
        state.glissement = None;
        assert_eq!(
            state.reduire(Geste::FinGain(cible), &[], &noeuds, &liens),
            Effet::Rien
        );
    }

    /// Le bouton de coupure envoie l'inverse de ce que le miroir dit, et rien
    /// d'autre — le gain reste inchangé.
    #[test]
    fn le_bouton_muet_envoie_l_inverse_de_l_etat_du_miroir() {
        let mut coupe = noeud_n(0, "a", 0, 1);
        coupe.muted = true;
        let noeuds = vec![noeud_n(1, "b", 0, 1), coupe];
        let mut liens = vec![lien(0, id(0), id(1))];
        let mut state = State::default();

        assert_eq!(
            state.reduire(Geste::Muet(Cible::Noeud(id(1))), &[], &noeuds, &liens),
            Effet::Commande(Command::SetNodeGain {
                node: id(1),
                gain_db: None,
                muted: Some(true),
            })
        );
        assert_eq!(
            state.reduire(Geste::Muet(Cible::Noeud(id(0))), &[], &noeuds, &liens),
            Effet::Commande(Command::SetNodeGain {
                node: id(0),
                gain_db: None,
                muted: Some(false),
            }),
            "un nœud déjà coupé se rétablit"
        );

        let cible = Cible::Lien(LinkId::new(0, 0));
        assert_eq!(
            state.reduire(Geste::Muet(cible), &[], &noeuds, &liens),
            Effet::Commande(Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: None,
                muted: Some(true),
            })
        );
        liens[0].muted = true;
        assert_eq!(
            state.reduire(Geste::Muet(cible), &[], &noeuds, &liens),
            Effet::Commande(Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: None,
                muted: Some(false),
            })
        );

        // Une cible que le miroir ne connaît pas n'envoie rien.
        assert_eq!(
            state.reduire(Geste::Muet(Cible::Noeud(id(9))), &[], &noeuds, &liens),
            Effet::Rien
        );
        assert_eq!(
            state.reduire(
                Geste::Muet(Cible::Lien(LinkId::new(9, 0))),
                &[],
                &noeuds,
                &liens
            ),
            Effet::Rien
        );
    }

    /// La valeur montrée est celle du doigt pendant le geste, puis celle du
    /// miroir dès qu'une notification arrive.
    #[test]
    fn la_valeur_montree_retombe_sur_le_miroir_a_la_notification() {
        let noeuds = vec![noeud_n(0, "a", 0, 1)];
        let cible = Cible::Noeud(id(0));
        let du_demon = Db::new(-3.0);
        let mut state = State::default();

        // Hors geste : la valeur du miroir, sans passer par la glissière —
        // une valeur que la course ne sait pas montrer n'est pas arrondie.
        assert_eq!(state.gain_affiche(cible, du_demon), du_demon);
        assert_eq!(state.position_de(cible, du_demon), -3.0);
        assert_eq!(state.gain_affiche(cible, Db::new(-3.5)), Db::new(-3.5));

        // Pendant le geste : la position du doigt, pour la glissière comme
        // pour le libellé.
        state.reduire(Geste::Gain(cible, -20.0), &[], &noeuds, &[]);
        assert_eq!(state.position_de(cible, du_demon), -20.0);
        assert_eq!(state.gain_affiche(cible, du_demon), Db::new(-20.0));
        // Une autre cible n'est pas concernée.
        let autre = Cible::Lien(LinkId::new(0, 0));
        assert_eq!(state.gain_affiche(autre, du_demon), du_demon);

        // Le relâchement envoie, mais ne rend pas encore la main : le démon
        // n'a rien annoncé.
        state.reduire(Geste::FinGain(cible), &[], &noeuds, &[]);
        assert_eq!(state.gain_affiche(cible, du_demon), Db::new(-20.0));

        // Une notification qui parle d'autre chose ne change rien…
        state.notifie(&Notification::LinkRemoved {
            id: LinkId::new(7, 0),
        });
        assert_eq!(state.gain_affiche(cible, du_demon), Db::new(-20.0));
        // … celle qui parle de la cible, si.
        state.notifie(&Notification::NodeAdded(noeuds[0].clone()));
        assert_eq!(state.glissement, None);
        assert_eq!(state.gain_affiche(cible, du_demon), du_demon);
        assert_eq!(state.position_de(cible, du_demon), -3.0);

        // Un rechargement complet oublie tout, lui aussi.
        state.reduire(Geste::Gain(cible, -30.0), &[], &noeuds, &[]);
        state.recharge();
        assert_eq!(state.glissement, None);
    }

    /// Un nœud suspendu garde son dessin et perd la main : ni glissière, ni
    /// coupure.
    #[test]
    fn un_noeud_suspendu_ne_regle_rien() {
        let mut suspendu = noeud_n(0, "hp", 2, 0);
        suspendu.state = NodeState::Suspended;
        let noeuds = vec![suspendu];
        let cible = Cible::Noeud(id(0));
        let mut state = State::default();

        assert!(!reglable(cible, &noeuds, &[]));
        assert_eq!(coupure(cible, &noeuds, &[]), None);
        assert_eq!(
            state.reduire(Geste::Gain(cible, -20.0), &[], &noeuds, &[]),
            Effet::Rien
        );
        assert_eq!(state.glissement, None, "la valeur montrée ne bouge pas");
        assert_eq!(
            state.reduire(Geste::FinGain(cible), &[], &noeuds, &[]),
            Effet::Rien
        );
        assert_eq!(
            state.reduire(Geste::Muet(cible), &[], &noeuds, &[]),
            Effet::Rien
        );
        // La carte, elle, reste là : c'est le voile qui le dit.
        assert_eq!(opacite(&noeuds[0]), style::OPACITE_SUSPENDU);
    }

    /// La hauteur calculée est celle de la carte réellement composée.
    ///
    /// C'est le contrat de l'architecture hybride : le canevas ne mesure rien,
    /// il calcule — encore faut-il qu'il calcule juste. La carte est ici
    /// composée puis mesurée hors écran, sans fenêtre.
    #[test]
    fn la_hauteur_calculee_est_celle_de_la_carte_composee() {
        for (entrees, sorties) in [(0, 0), (0, 2), (2, 2), (1, 4)] {
            let n = noeud("Carte mesurée", entrees, sorties);
            let placement = Placement {
                cle: n.key.to_string(),
                position: Point::ORIGIN,
                hauteur: hauteur_carte(entrees, sorties),
            };
            let mesuree = mesurer(corps(&n, false, &State::default(), 1.0, Weight::Normal));
            assert_eq!(
                mesuree,
                hauteur_carte(entrees, sorties),
                "{entrees} entrées, {sorties} sorties"
            );
            assert_eq!(placement.hauteur, mesuree);
        }

        /// Hauteur du corps d'une carte, composé puis disposé hors écran.
        fn mesurer(corps: Element<'_, Message>) -> f32 {
            let reglages = iced_test::core::Settings {
                fonts: vec![typo::POLICE_INTER.into()],
                default_font: typo::interface(),
                default_text_size: iced::Pixels(CORPS_INTERFACE),
                ..Default::default()
            };
            // La carte est posée en haut d'une colonne assez vaste pour ne
            // rien contraindre : sa hauteur est donc la sienne.
            let vue: Element<'_, Message> =
                iced::widget::column![container(corps).width(Fill), space::vertical()]
                    .width(Fill)
                    .height(Fill)
                    .into();
            let mut ui = iced_test::Simulator::with_size(reglages, Size::new(800.0, 600.0), vue);
            ui.find(iced_test::selector::id(ID_CARTE))
                .expect("la carte doit être trouvée dans l'arbre")
                .bounds()
                .height
        }
    }
}
