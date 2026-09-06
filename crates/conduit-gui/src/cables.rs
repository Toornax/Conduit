//! Vue « Câbles » : la liste des câbles virtuels, en grille à six colonnes
//! (M2-03, F-01 à F-03).
//!
//! Une ligne par câble et six colonnes : le **périphérique** (le nom système,
//! `Conduit N`), l'**alias** éditable en place, les **canaux** en − / +, le
//! **niveau**, l'**état**, et la croix de **suppression**. Il n'y a pas de
//! formulaire de création : le bouton « Ajouter un câble » de l'en-tête crée
//! un câble stéréo sans nom — le démon le nomme —, et l'alias se saisit
//! ensuite dans la ligne.
//!
//! Chaque action envoie la commande correspondante (`CableAdd`,
//! `CableRemove`, `CableRename`, `CableSetChannels`) : rien n'est appliqué
//! localement par anticipation, l'affichage suit les notifications
//! `CableChanged`.
//!
//! # La colonne « Niveau » est inerte
//!
//! Les barres sont dessinées **à zéro**, une par canal, et rien ne les
//! rafraîchit : ni minuteur, ni interrogation périodique. Le protocole ne
//! pousse aucun niveau, et `Command::ReadMeter` ne lit que les nœuds internes
//! `Meter` — un câble n'en est pas un. La colonne tient donc sa place jusqu'aux
//! VU-mètres (M2-07) ; une infobulle sur son en-tête le dit à l'utilisateur.
//!
//! # Le texte secondaire ne descend pas dans les lignes
//!
//! [`Jetons::texte_2`](crate::theme::Jetons::texte_2) est réservé au fond de
//! la fenêtre : sur la surface posée d'une ligne, il ne passe que 4,3:1 en
//! mode clair. Les en-têtes de colonnes et le pied, qui sont sur le fond de la
//! fenêtre, le portent ; dans une ligne, la hiérarchie se fait au corps et
//! entre `titre` et `texte`.

use conduit_backend::{CableId, CableInfo};
use conduit_core::types::ChannelCount;
use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{button, column, container, row, scrollable, space, text, text_input, tooltip};
use iced::{Center, Element, Fill, Length, Padding, Right};

use crate::app::Message;
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::theme::{CORPS_INTERFACE, CORPS_META, CORPS_TEXTE, ESPACE_S, ESPACE_XS};
use crate::{style, typo};

// --- Mesures de la maquette -------------------------------------------------

/// Largeur de la colonne du périphérique.
const COL_PERIPHERIQUE: f32 = 150.0;
/// Part de la colonne de l'alias dans l'espace laissé par les colonnes fixes.
const PART_ALIAS: u16 = 140;
/// Largeur de la colonne des canaux.
const COL_CANAUX: f32 = 104.0;
/// Part de la colonne du niveau dans ce même espace.
const PART_NIVEAU: u16 = 80;
/// Largeur de la colonne d'état.
const COL_ETAT: f32 = 90.0;
/// Largeur de la colonne de suppression.
const COL_SUPPRESSION: f32 = 32.0;
/// Écart entre deux colonnes.
const ECART_COLONNES: f32 = 16.0;
/// Largeur des actions de confirmation : exactement la place de la colonne
/// d'état, de son écart et de la colonne de suppression, pour que rien ne
/// bouge à gauche quand la question s'ouvre.
const COL_ACTIONS: f32 = COL_ETAT + ECART_COLONNES + COL_SUPPRESSION;
/// Corps d'un en-tête de colonne, en petites capitales.
const CORPS_ENTETE: f32 = 11.0;
/// Corps de la seconde ligne du périphérique.
const CORPS_SOUS_LIGNE: f32 = 11.0;
/// Écart entre le nom système et sa seconde ligne.
const ECART_SOUS_LIGNE: f32 = 2.0;
/// Interligne serré des deux lignes du périphérique : un nom n'a pas besoin
/// des 1,6 du texte courant.
const INTERLIGNE_SERRE: f32 = 1.15;
/// Côté d'un bouton carré : pas de canal, croix de suppression.
const COTE_BOUTON: f32 = 28.0;
/// Largeur du nombre de canaux, entre ses deux boutons.
const LARGEUR_CANAUX: f32 = 20.0;
/// Hauteur d'une barre de niveau.
const HAUTEUR_BARRE: f32 = 4.0;
/// Écart entre deux barres de niveau.
const ECART_BARRES: f32 = 3.0;
/// Diamètre de la pastille d'état.
const PASTILLE: f32 = 8.0;
/// Padding d'une ligne : 14 en hauteur, 16 en largeur.
const PADDING_LIGNE: Padding = Padding {
    top: 14.0,
    right: 16.0,
    bottom: 14.0,
    left: 16.0,
};
/// Hauteur minimale d'une ligne, padding compris.
const HAUTEUR_LIGNE: f32 = 44.0;
/// Hauteur minimale du contenu d'une ligne : ce qu'il en reste une fois le
/// padding retiré. Tenue par une cale dans la première colonne — une cale
/// posée dans la rangée des colonnes y prendrait un écart de colonne.
const HAUTEUR_CONTENU: f32 = HAUTEUR_LIGNE - PADDING_LIGNE.top - PADDING_LIGNE.bottom;
/// Padding de la ligne d'en-têtes : rien en haut, 16 sur les côtés, 10 en bas.
const PADDING_ENTETES: Padding = Padding {
    top: 0.0,
    right: 16.0,
    bottom: 10.0,
    left: 16.0,
};
/// Padding du pied de la vue, aligné sur les colonnes.
const PADDING_PIED: Padding = Padding {
    top: 14.0,
    right: 16.0,
    bottom: 0.0,
    left: 16.0,
};
/// Padding intérieur du champ d'alias.
const PADDING_CHAMP: f32 = 6.0;
/// Padding intérieur de l'infobulle.
const PADDING_INFOBULLE: f32 = 10.0;
/// Largeur maximale de l'infobulle.
const LARGEUR_INFOBULLE: f32 = 320.0;
/// Écart entre les lignes de la liste.
const ECART_LIGNES: f32 = 8.0;
/// Niveau affiché par chaque barre : voir « La colonne Niveau est inerte ».
const NIVEAU_INERTE: f32 = 0.0;
/// Nombre de parts d'une barre de niveau : `iced` ne sait partager une
/// largeur qu'en parts entières, mille suffisent au dixième de pour cent.
const PARTS_BARRE: u16 = 1_000;

// --- État -------------------------------------------------------------------

/// État d'édition de la vue, distinct du miroir du démon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Câble dont l'alias est en cours de saisie, et le texte saisi.
    pub renaming: Option<(CableId, String)>,
    /// Câble dont la suppression attend confirmation.
    pub removing: Option<CableId>,
}

impl State {
    /// Ferme la confirmation et l'édition d'alias en cours.
    pub fn close_dialogs(&mut self) {
        self.renaming = None;
        self.removing = None;
    }
}

// --- Décisions --------------------------------------------------------------

/// Le nombre de canaux atteint depuis `courant` par un pas de `delta`, ou
/// `None` en butée : 1 en bas, [`ChannelCount::MAX`] en haut (F-03).
///
/// C'est cette absence de cible qui grise le bouton correspondant.
pub fn pas(courant: ChannelCount, delta: i8) -> Option<ChannelCount> {
    let vise = i16::from(courant.get()) + i16::from(delta);
    u8::try_from(vise).ok().and_then(ChannelCount::new)
}

/// Les niveaux dessinés pour un câble : un par canal, tous nuls.
///
/// Le démon ne diffuse aucun niveau (voir le module) : cette fonction ne lit
/// rien, elle ne fait que donner sa hauteur à la colonne.
pub fn niveaux(cable: &CableInfo) -> Vec<f32> {
    vec![NIVEAU_INERTE; cable.channels.as_usize()]
}

// --- Composition ------------------------------------------------------------

/// Compose la vue. `enabled` est faux tant que le démon ne répond pas : les
/// actions sont alors grisées plutôt que perdues.
///
/// `graisse` est la graisse du texte courant du mode
/// ([`crate::theme::Jetons::graisse_texte`]).
pub fn view<'a>(
    mirror: &'a Mirror,
    state: &'a State,
    enabled: bool,
    graisse: Weight,
) -> Element<'a, Message> {
    let mut liste = column![].spacing(ECART_LIGNES).width(Fill);
    if mirror.cables.is_empty() {
        let vide = if mirror.is_loaded() {
            Text::CablesEmpty
        } else {
            Text::Waiting
        };
        liste = liste.push(
            container(
                text(i18n::t(vide))
                    .size(CORPS_INTERFACE)
                    .font(typo::texte_a(graisse, CORPS_INTERFACE))
                    .style(style::texte_en(|j| j.texte_2)),
            )
            .padding(PADDING_LIGNE),
        );
    }
    for cable in &mirror.cables {
        liste = liste.push(ligne(cable, state, enabled, graisse));
    }
    column![
        entetes(graisse),
        scrollable(liste)
            .height(Fill)
            .width(Fill)
            .style(style::defilement),
        pied(mirror, graisse),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// La ligne d'en-têtes de colonnes, en petites capitales.
fn entetes<'a>(graisse: Weight) -> Element<'a, Message> {
    container(
        row![
            entete(Text::ColumnDevice, Length::Fixed(COL_PERIPHERIQUE)),
            entete(Text::ColumnAlias, Length::FillPortion(PART_ALIAS)),
            entete(Text::ColumnChannels, Length::Fixed(COL_CANAUX)),
            entete_niveau(graisse),
            entete(Text::ColumnState, Length::Fixed(COL_ETAT)),
            space::horizontal().width(COL_SUPPRESSION),
        ]
        .spacing(ECART_COLONNES)
        .width(Fill),
    )
    .padding(PADDING_ENTETES)
    .into()
}

/// Un en-tête de colonne : capitales espacées, corps 11, texte secondaire.
fn entete<'a>(libelle: Text, largeur: Length) -> Element<'a, Message> {
    container(typo::petites_capitales(i18n::t(libelle), CORPS_ENTETE))
        .width(largeur)
        .style(style::encre_de(|j| j.texte_2))
        .into()
}

/// L'en-tête de la colonne « Niveau », qui porte l'infobulle disant pourquoi
/// ses barres restent à zéro.
fn entete_niveau<'a>(graisse: Weight) -> Element<'a, Message> {
    let note = container(
        text(i18n::t(Text::CableLevelInert))
            .size(CORPS_META)
            .font(typo::texte_a(graisse, CORPS_META)),
    )
    .max_width(LARGEUR_INFOBULLE);
    container(
        tooltip(
            typo::petites_capitales(i18n::t(Text::ColumnLevel), CORPS_ENTETE),
            note,
            tooltip::Position::Bottom,
        )
        .gap(ESPACE_XS)
        .padding(PADDING_INFOBULLE)
        .style(style::carte),
    )
    .width(Length::FillPortion(PART_NIVEAU))
    .style(style::encre_de(|j| j.texte_2))
    .into()
}

/// Une ligne de la liste : les six colonnes, sur une surface posée.
///
/// Tant qu'une suppression attend confirmation, les trois dernières colonnes
/// cèdent la place à la question : la croix seule, sur une action destructrice,
/// serait un piège (écart assumé par rapport à la maquette).
fn ligne<'a>(
    cable: &'a CableInfo,
    state: &'a State,
    enabled: bool,
    graisse: Weight,
) -> Element<'a, Message> {
    let id = cable.id;
    let mut cellules = row![].spacing(ECART_COLONNES).align_y(Center).width(Fill);
    cellules = cellules.push(peripherique(id, graisse));
    cellules = cellules.push(alias(cable, state, enabled, graisse));
    cellules = cellules.push(canaux(cable, enabled));
    if state.removing == Some(id) {
        let (question, actions) = confirmation(id, enabled, graisse);
        cellules = cellules.push(question).push(actions);
    } else {
        cellules = cellules.push(niveau(cable));
        cellules = cellules.push(etat(cable, graisse));
        cellules = cellules.push(suppression(id, enabled));
    }
    container(cellules)
        .padding(PADDING_LIGNE)
        .width(Fill)
        .style(style::ligne)
        .into()
}

/// Le nom système du câble et, dessous, ce qu'il expose au système.
fn peripherique<'a>(id: CableId, graisse: Weight) -> Element<'a, Message> {
    container(row![
        // La cale qui tient la hauteur minimale de la ligne.
        space::vertical().height(HAUTEUR_CONTENU),
        column![
            text(id.to_string())
                .size(CORPS_TEXTE)
                .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
                .font(typo::texte_a(graisse, CORPS_TEXTE))
                .style(style::texte_en(|j| j.titre)),
            text(i18n::t(Text::CableDuplex))
                .size(CORPS_SOUS_LIGNE)
                .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
                .font(typo::interface_graisse(graisse)),
        ]
        .spacing(ECART_SOUS_LIGNE),
    ])
    .width(COL_PERIPHERIQUE)
    .into()
}

/// Le champ d'alias, édité en place.
///
/// La ligne en cours d'édition montre la saisie ; les autres montrent le nom
/// que le démon leur connaît.
fn alias<'a>(
    cable: &'a CableInfo,
    state: &'a State,
    enabled: bool,
    graisse: Weight,
) -> Element<'a, Message> {
    let id = cable.id;
    let valeur = match &state.renaming {
        Some((edite, saisie)) if *edite == id => saisie.as_str(),
        _ => cable.name.as_str(),
    };
    container(
        text_input(i18n::t(Text::CableAlias), valeur)
            .on_input_maybe(enabled.then_some(move |saisie| Message::AliasEdited(id, saisie)))
            .on_submit(Message::CommitAlias)
            .size(CORPS_INTERFACE)
            .font(typo::texte_a(graisse, CORPS_INTERFACE))
            .padding(PADDING_CHAMP)
            .style(style::champ_souligne)
            .width(Fill),
    )
    .width(Length::FillPortion(PART_ALIAS))
    .into()
}

/// Les canaux : `−`, le nombre, `+`. Un bouton en butée est grisé.
fn canaux<'a>(cable: &'a CableInfo, enabled: bool) -> Element<'a, Message> {
    let id = cable.id;
    let bouton = |libelle: Text, delta: i8| {
        let cible = enabled.then(|| pas(cable.channels, delta)).flatten();
        button(
            text(i18n::t(libelle))
                .size(CORPS_INTERFACE)
                .font(typo::interface())
                .width(Fill)
                .height(Fill)
                .align_x(Center)
                .align_y(Center),
        )
        .width(COTE_BOUTON)
        .height(COTE_BOUTON)
        .padding(0)
        .on_press_maybe(cible.map(|canaux| Message::SetChannels(id, canaux)))
        .style(style::bouton_pas)
    };
    container(
        row![
            bouton(Text::Minus, -1),
            text(cable.channels.get().to_string())
                .size(CORPS_INTERFACE)
                .font(typo::interface())
                .width(LARGEUR_CANAUX)
                .align_x(Center),
            bouton(Text::Plus, 1),
        ]
        .spacing(ESPACE_S)
        .align_y(Center),
    )
    .width(COL_CANAUX)
    .into()
}

/// Les barres de niveau : une par canal, toutes à zéro (voir le module).
fn niveau<'a>(cable: &'a CableInfo) -> Element<'a, Message> {
    let mut barres = column![].spacing(ECART_BARRES).width(Fill);
    for valeur in niveaux(cable) {
        barres = barres.push(barre(valeur));
    }
    container(barres)
        .width(Length::FillPortion(PART_NIVEAU))
        .into()
}

/// Une barre : un rail creusé dans la ligne, rempli d'or à hauteur du niveau.
fn barre<'a>(niveau: f32) -> Element<'a, Message> {
    let rempli = (niveau.clamp(0.0, 1.0) * f32::from(PARTS_BARRE)) as u16;
    let mut rail = row![].width(Fill).height(Fill);
    if rempli > 0 {
        rail = rail.push(
            container(space::horizontal())
                .width(Length::FillPortion(rempli))
                .height(Fill)
                .style(style::barre_remplie),
        );
    }
    if rempli < PARTS_BARRE {
        rail = rail.push(space::horizontal().width(Length::FillPortion(PARTS_BARRE - rempli)));
    }
    container(rail)
        .width(Fill)
        .height(HAUTEUR_BARRE)
        .style(style::barre_rail)
        .into()
}

/// La pastille d'état et son libellé.
fn etat<'a>(cable: &'a CableInfo, graisse: Weight) -> Element<'a, Message> {
    let libelle = if cable.active {
        Text::CableActive
    } else {
        Text::CableInactive
    };
    let couleur: fn(&crate::theme::Jetons) -> iced::Color = if cable.active {
        |j| j.celadon
    } else {
        |j| j.texte_2
    };
    container(
        row![
            container(space::horizontal())
                .width(PASTILLE)
                .height(PASTILLE)
                .style(style::pastille_de(couleur)),
            text(i18n::t(libelle))
                .size(CORPS_META)
                .font(typo::texte_a(graisse, CORPS_META)),
        ]
        .spacing(ESPACE_S)
        .align_y(Center),
    )
    .width(COL_ETAT)
    .into()
}

/// La croix de suppression, qui ne fait qu'ouvrir la confirmation.
fn suppression<'a>(id: CableId, enabled: bool) -> Element<'a, Message> {
    container(
        button(
            text(i18n::t(Text::RemoveIcon))
                .size(CORPS_TEXTE)
                .font(typo::interface())
                .width(Fill)
                .height(Fill)
                .align_x(Center)
                .align_y(Center),
        )
        .width(COTE_BOUTON)
        .height(COTE_BOUTON)
        .padding(0)
        .on_press_maybe(enabled.then_some(Message::AskRemove(id)))
        .style(style::bouton_discret),
    )
    .width(COL_SUPPRESSION)
    .align_x(Right)
    .into()
}

/// La confirmation de suppression, en deux cellules : la question à la place
/// de la colonne « Niveau », les deux actions à la place de l'état et de la
/// croix.
///
/// Les deux cellules occupent exactement la largeur des trois colonnes
/// qu'elles remplacent : les colonnes de gauche ne bougent pas d'un pixel
/// quand la question s'ouvre.
fn confirmation<'a>(
    id: CableId,
    enabled: bool,
    graisse: Weight,
) -> (Element<'a, Message>, Element<'a, Message>) {
    let action = move |libelle: Text, message: Option<Message>| {
        button(
            text(i18n::t(libelle))
                .size(CORPS_META)
                .font(typo::texte_a(graisse, CORPS_META)),
        )
        .padding(ESPACE_XS)
        .on_press_maybe(message)
    };
    let question = container(
        text(i18n::t(Text::CableRemoveConfirm))
            .size(CORPS_META)
            .font(typo::texte_a(graisse, CORPS_META)),
    )
    .width(Length::FillPortion(PART_NIVEAU))
    .align_x(Right)
    .into();
    let actions = container(
        row![
            action(Text::Remove, enabled.then_some(Message::RemoveCable(id))).style(style::lien),
            action(Text::Cancel, Some(Message::CancelDialog)).style(style::bouton_discret),
        ]
        .spacing(ESPACE_S)
        .align_y(Center),
    )
    .width(COL_ACTIONS)
    .align_x(Right)
    .into();
    (question, actions)
}

/// Le pied de la vue : le compte de câbles à gauche, la note sur les alias à
/// droite.
fn pied<'a>(mirror: &'a Mirror, graisse: Weight) -> Element<'a, Message> {
    let meta = move |contenu: String| {
        text(contenu)
            .size(CORPS_META)
            .font(typo::texte_a(graisse, CORPS_META))
            .style(style::texte_en(|j| j.texte_2))
    };
    container(
        row![
            meta(i18n::cables_count(mirror.cables.len())),
            space::horizontal(),
            meta(i18n::t(Text::CableAliasNote).to_string()),
        ]
        .align_y(Center)
        .width(Fill),
    )
    .padding(PADDING_PIED)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::fixtures::cable;

    /// Les pas couvrent 1 à 8 et s'arrêtent aux deux butées.
    #[test]
    fn le_pas_couvre_tous_les_canaux_et_s_arrete_aux_butees() {
        let mut courant = ChannelCount::MONO;
        let mut atteints = vec![courant.get()];
        while let Some(suivant) = pas(courant, 1) {
            courant = suivant;
            atteints.push(courant.get());
        }
        assert_eq!(atteints, (1..=ChannelCount::MAX).collect::<Vec<_>>());
        // En haut comme en bas, le pas suivant n'existe pas : le bouton est
        // grisé plutôt que de proposer une valeur refusée.
        assert_eq!(pas(courant, 1), None);
        assert_eq!(courant.get(), ChannelCount::MAX);
        assert_eq!(pas(ChannelCount::MONO, -1), None);
        assert_eq!(pas(ChannelCount::STEREO, -1), Some(ChannelCount::MONO));
        assert_eq!(pas(ChannelCount::MONO, 1), Some(ChannelCount::STEREO));
    }

    /// Une barre par canal, toutes à zéro : rien ne les alimente.
    #[test]
    fn il_y_a_une_barre_de_niveau_par_canal_et_toutes_sont_nulles() {
        for canaux in 1..=ChannelCount::MAX {
            let mut c = cable(1, "Musique");
            c.channels = ChannelCount::new(canaux).unwrap();
            let niveaux = niveaux(&c);
            assert_eq!(niveaux.len(), usize::from(canaux));
            assert!(niveaux.iter().all(|n| *n == 0.0), "un niveau non nul");
        }
    }

    /// L'état d'édition part vide et se referme d'un coup.
    #[test]
    fn l_etat_d_edition_part_vide_et_se_referme() {
        let mut s = State::default();
        assert!(s.renaming.is_none() && s.removing.is_none());
        s.renaming = Some((CableId(1), "Jeu".into()));
        s.removing = Some(CableId(2));
        s.close_dialogs();
        assert_eq!(s, State::default());
    }
}
