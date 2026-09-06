//! Vue « Patchbay » : le graphe du démon en cartes déplaçables (M2-04).
//!
//! Un nœud du graphe est une **carte** : son libellé et une étiquette d'état
//! en en-tête, puis ses ports — les entrées à gauche, les sorties à droite,
//! chacun marqué d'une pastille d'or posée à cheval sur le bord de la carte.
//! Les **liens** arrivent au commit suivant (M2-05), les **gains** à celui
//! d'après (M2-06) : ce module pose la scène, sa disposition et sa mémoire.
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
//! widgets, avec les vraies polices, le vrai thème et, demain, la vraie
//! glissière de gain de M2-06.
//!
//! Le canevas, lui, garde ce que les widgets ne savent pas faire : dessiner
//! des courbes entre deux points quelconques (M2-05) et suivre le curseur
//! pendant un déplacement. Il le peut parce que [`Stack`] distribue chaque
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

use std::collections::BTreeMap;

use conduit_core::graph::Direction;
use conduit_protocol::{NodeDescriptor, NodeState};
use iced::font::Weight;
use iced::mouse;
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
    canvas, column, container, mouse_area, pin, row, rule, scrollable, space, stack, text,
};
use iced::{Center, Element, Fill, Padding, Point, Rectangle, Renderer, Size, Theme, Vector};
use serde::{Deserialize, Serialize};

use crate::app::Message;
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::theme::{CORPS_INTERFACE, ESPACE_S, FILET};
use crate::{style, typo};

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
/// L'en-tête, son filet, et une grille de ports à deux colonnes : c'est la
/// plus fournie des deux qui donne le nombre de lignes. Les constantes sont
/// celles de la composition — un test le vérifie en mesurant une carte
/// réellement composée.
pub fn hauteur_carte(entrees: usize, sorties: usize) -> f32 {
    let lignes = entrees.max(sorties) as f32;
    HAUTEUR_ENTETE + FILET + 2.0 * MARGE_PORTS + lignes * HAUTEUR_PORT
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
/// est le rang du port dans sa colonne. M2-05 y accrochera les liens.
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

// --- État -------------------------------------------------------------------

/// Geste de déplacement d'une carte.
#[derive(Debug, Clone, PartialEq)]
pub enum Geste {
    /// L'en-tête d'une carte vient d'être pressé.
    Saisi(String),
    /// Le curseur a bougé, en coordonnées de la scène.
    Deplace(Point),
    /// Le bouton est relâché.
    Relache,
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

/// État de la vue, distinct du miroir du démon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// Positions mémorisées, lues au démarrage et écrites au relâchement.
    pub positions: Positions,
    /// Carte en cours de déplacement, s'il y en a une.
    pub saisie: Option<Saisie>,
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

    /// Réduit un geste. Rend vrai s'il faut écrire les préférences, c'est-à-
    /// dire au relâchement d'une carte qui a bougé.
    ///
    /// Pure : aucune entrée/sortie, `placements` dit seulement où les cartes
    /// se trouvent au moment de la saisie.
    pub fn reduire(&mut self, geste: Geste, placements: &[Placement]) -> bool {
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
                return self.saisie.take().is_some_and(|s| s.deplacee);
            }
        }
        false
    }
}

// --- Canevas ----------------------------------------------------------------

/// Le canevas de la scène : sous les cartes, il ne dessine encore rien — les
/// liens sont M2-05 — mais porte déjà l'interaction de déplacement.
///
/// Il voit les mouvements et le relâchement que le [`mouse_area`] de l'en-tête
/// ne capture pas, ce qui permet de continuer à suivre le curseur quand celui-
/// ci sort de la carte.
#[derive(Debug, Clone, Copy)]
pub struct Graphe {
    /// Vrai pendant le déplacement d'une carte.
    deplacement: bool,
}

impl canvas::Program<Message> for Graphe {
    type State = ();

    fn update(
        &self,
        _etat: &mut (),
        evenement: &iced::Event,
        bornes: Rectangle,
        _curseur: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if !self.deplacement {
            return None;
        }
        // La position de l'événement est celle de la fenêtre ; la scène a son
        // propre repère, qui commence au coin du canevas.
        match evenement {
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(canvas::Action::publish(Message::Patchbay(Geste::Deplace(
                    *position - Vector::new(bornes.x, bornes.y),
                ))))
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                Some(canvas::Action::publish(Message::Patchbay(Geste::Relache)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _etat: &(),
        _renderer: &Renderer,
        _theme: &Theme,
        _bornes: Rectangle,
        _curseur: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // Les liens arrivent en M2-05 : rien à dessiner pour l'instant.
        Vec::new()
    }

    fn mouse_interaction(
        &self,
        _etat: &(),
        _bornes: Rectangle,
        _curseur: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.deplacement {
            mouse::Interaction::Grabbing
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
    let mut couches = stack![canvas(Graphe {
        deplacement: state.deplacement(),
    })
    .width(etendue.width)
    .height(etendue.height)];
    for (noeud, placement) in mirror.nodes.iter().zip(&placements) {
        let saisie = state.saisie_de(&placement.cle);
        couches = couches.push(
            pin(carte(noeud, placement, saisie, graisse))
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
    let mut couches = stack![container(corps(noeud, saisie, opacite, graisse))
        .padding(Padding::ZERO.horizontal(DEBORDEMENT))];
    for (direction, ports) in [
        (Direction::Input, &noeud.inputs),
        (Direction::Output, &noeud.outputs),
    ] {
        for index in 0..ports.len() {
            let centre = ancre(&coin, direction, index);
            couches = couches.push(
                pin(container(space::horizontal())
                    .width(PASTILLE)
                    .height(PASTILLE)
                    .style(style::pastille_voilee(|j| j.or, opacite)))
                .x(centre.x - PASTILLE / 2.0)
                .y(centre.y - PASTILLE / 2.0),
            );
        }
    }
    couches.into()
}

/// Le corps de la carte : l'en-tête saisissable, son filet, la grille des
/// ports.
fn corps<'a>(
    noeud: &'a NodeDescriptor,
    saisie: bool,
    opacite: f32,
    graisse: Weight,
) -> Element<'a, Message> {
    let pilote = noeud.state == NodeState::Driver;
    container(column![
        entete(noeud, saisie, opacite, graisse),
        rule::horizontal(FILET).style(style::filet),
        ports(noeud, opacite, graisse),
    ])
    .id(ID_CARTE)
    .width(LARGEUR_CARTE)
    .style(style::carte_noeud(pilote, opacite))
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
/// Il annonce le glisser-déposer de M2-05 : la phrase reste, la vue sera
/// complète au commit suivant. Il est posé **hors** du défilement, pour
/// rester lisible quelle que soit la position de la scène.
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
    use conduit_core::graph::NodeId;
    use conduit_core::node::PortSpec;
    use conduit_core::types::{Db, SampleRate};
    use conduit_protocol::api::NodeKey;

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

    /// La hauteur suit la plus fournie des deux colonnes de ports.
    #[test]
    fn la_hauteur_suit_la_colonne_la_plus_fournie() {
        let nue = HAUTEUR_ENTETE + FILET + 2.0 * MARGE_PORTS;
        assert_eq!(hauteur_carte(0, 0), nue);
        assert_eq!(hauteur_carte(2, 0), nue + 2.0 * HAUTEUR_PORT);
        assert_eq!(hauteur_carte(0, 2), nue + 2.0 * HAUTEUR_PORT);
        assert_eq!(hauteur_carte(1, 3), hauteur_carte(3, 1));
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

        assert!(!state.reduire(Geste::Saisi("internal:a".into()), &placements));
        assert!(state.deplacement() && state.saisie_de("internal:a"));
        // Le premier mouvement ne fait que noter l'écart au curseur.
        let curseur = Point::new(depart.x + 30.0, depart.y + 12.0);
        assert!(!state.reduire(Geste::Deplace(curseur), &placements));
        assert!(state.positions.is_empty(), "la carte n'a pas encore bougé");
        // Le suivant emmène la carte, écart conservé.
        assert!(!state.reduire(
            Geste::Deplace(Point::new(curseur.x + 100.0, curseur.y + 40.0)),
            &placements
        ));
        assert_eq!(
            state.positions.get("internal:a"),
            Some(Point::new(depart.x + 100.0, depart.y + 40.0))
        );
        // Le relâchement ferme la saisie et demande l'écriture.
        assert!(state.reduire(Geste::Relache, &placements));
        assert!(!state.deplacement());
        assert!(
            !state.reduire(Geste::Relache, &placements),
            "plus rien à écrire"
        );
    }

    /// Une carte glissée hors de la scène est retenue au bord.
    #[test]
    fn une_carte_ne_sort_pas_par_le_haut_ni_par_la_gauche() {
        let noeuds = vec![noeud("a", 0, 1)];
        let placements = disposer(&noeuds, &Positions::default());
        let mut state = State::default();
        state.reduire(Geste::Saisi("internal:a".into()), &placements);
        state.reduire(Geste::Deplace(placements[0].position), &placements);
        state.reduire(Geste::Deplace(Point::new(-500.0, -500.0)), &placements);
        assert_eq!(state.positions.get("internal:a"), Some(Point::ORIGIN));
    }

    /// Une clé inconnue ne saisit rien, et un mouvement sans saisie n'a aucun
    /// effet.
    #[test]
    fn un_geste_sans_carte_est_sans_effet() {
        let mut state = State::default();
        assert!(!state.reduire(Geste::Saisi("internal:fantôme".into()), &[]));
        assert!(!state.deplacement());
        assert!(!state.reduire(Geste::Deplace(Point::new(10.0, 10.0)), &[]));
        assert!(state.positions.is_empty());
        assert!(!state.reduire(Geste::Relache, &[]));
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
            let mesuree = mesurer(corps(&n, false, 1.0, Weight::Normal));
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
