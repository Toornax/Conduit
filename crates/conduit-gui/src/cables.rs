//! Vue « Câbles » : liste, création, suppression, renommage et canaux
//! (M2-03, F-01 à F-03).
//!
//! Chaque action envoie la commande correspondante (`CableAdd`,
//! `CableRemove`, `CableRename`, `CableSetChannels`) : rien n'est appliqué
//! localement par anticipation, l'affichage suit les notifications
//! `CableChanged`.

use conduit_backend::{CableId, CableInfo};
use conduit_core::types::ChannelCount;
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Center, Element, Fill};

use crate::app::Message;
use crate::i18n::{self, Text};
use crate::model::Mirror;

/// Largeur de la colonne des noms.
const NAME_WIDTH: f32 = 240.0;

/// Un nombre de canaux proposé dans les listes déroulantes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channels(pub u8);

impl core::fmt::Display for Channels {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&i18n::channels_label(self.0))
    }
}

impl From<Channels> for ChannelCount {
    fn from(c: Channels) -> Self {
        ChannelCount::new(c.0).unwrap_or_default()
    }
}

impl From<ChannelCount> for Channels {
    fn from(c: ChannelCount) -> Self {
        Channels(c.get())
    }
}

/// Les choix de canaux offerts : 1 à [`ChannelCount::MAX`] (F-03).
pub const CHOICES: [Channels; ChannelCount::MAX as usize] = [
    Channels(1),
    Channels(2),
    Channels(3),
    Channels(4),
    Channels(5),
    Channels(6),
    Channels(7),
    Channels(8),
];

/// État d'édition de la vue, distinct du miroir du démon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// Nom saisi pour le câble à créer ; vide = nom automatique.
    pub new_name: String,
    /// Canaux du câble à créer.
    pub new_channels: Channels,
    /// Câble en cours de renommage et texte saisi.
    pub renaming: Option<(CableId, String)>,
    /// Câble dont la suppression attend confirmation.
    pub removing: Option<CableId>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            new_name: String::new(),
            new_channels: Channels::from(ChannelCount::STEREO),
            renaming: None,
            removing: None,
        }
    }
}

impl State {
    /// Ferme la boîte de confirmation et l'édition de nom en cours.
    pub fn close_dialogs(&mut self) {
        self.renaming = None;
        self.removing = None;
    }
}

/// Compose la vue. `enabled` est faux tant que le démon ne répond pas : les
/// actions sont alors grisées plutôt que perdues.
pub fn view<'a>(mirror: &'a Mirror, state: &'a State, enabled: bool) -> Element<'a, Message> {
    let mut list = column![].spacing(8).width(Fill);
    if mirror.cables.is_empty() {
        let empty = if mirror.is_loaded() {
            Text::CablesEmpty
        } else {
            Text::Waiting
        };
        list = list.push(text(i18n::t(empty)));
    }
    for cable in &mirror.cables {
        list = list.push(entry(cable, state, enabled));
    }
    column![
        text(i18n::t(Text::CablesTitle)).size(20),
        scrollable(list).height(Fill).width(Fill),
        creation(state, enabled),
    ]
    .spacing(12)
    .width(Fill)
    .height(Fill)
    .into()
}

/// Une ligne de la liste : nom (ou champ de renommage), canaux, état et
/// suppression.
fn entry<'a>(cable: &'a CableInfo, state: &'a State, enabled: bool) -> Element<'a, Message> {
    let id = cable.id;
    let name: Element<'a, Message> = match &state.renaming {
        Some((editing, value)) if *editing == id => row![
            text_input(i18n::t(Text::CableName), value)
                .on_input(Message::RenameEdited)
                .on_submit(Message::CommitRename)
                .width(NAME_WIDTH),
            button(text(i18n::t(Text::Validate)))
                .on_press_maybe(enabled.then_some(Message::CommitRename)),
            button(text(i18n::t(Text::Cancel)))
                .on_press(Message::CancelDialog)
                .style(button::secondary),
        ]
        .spacing(6)
        .align_y(Center)
        .into(),
        _ => row![
            text(cable.name.as_str()).width(NAME_WIDTH),
            button(text(i18n::t(Text::Rename)))
                .on_press_maybe(enabled.then_some(Message::StartRename(id)))
                .style(button::secondary),
        ]
        .spacing(6)
        .align_y(Center)
        .into(),
    };

    let channels: Element<'a, Message> = if enabled {
        pick_list(CHOICES, Some(Channels::from(cable.channels)), move |c| {
            Message::SetChannels(id, c)
        })
        .into()
    } else {
        text(i18n::channels_label(cable.channels.get())).into()
    };

    let state_label = if cable.active {
        Text::CableActive
    } else {
        Text::CableInactive
    };

    let removal: Element<'a, Message> = if state.removing == Some(id) {
        row![
            text(i18n::t(Text::CableRemoveConfirm)),
            button(text(i18n::t(Text::Remove)))
                .on_press_maybe(enabled.then_some(Message::RemoveCable(id)))
                .style(button::danger),
            button(text(i18n::t(Text::Cancel)))
                .on_press(Message::CancelDialog)
                .style(button::secondary),
        ]
        .spacing(6)
        .align_y(Center)
        .into()
    } else {
        button(text(i18n::t(Text::Remove)))
            .on_press_maybe(enabled.then_some(Message::AskRemove(id)))
            .style(button::danger)
            .into()
    };

    container(
        row![
            text(id.to_string()).size(14).width(110.0),
            name,
            channels,
            text(i18n::t(state_label)).size(12),
            removal,
        ]
        .spacing(10)
        .align_y(Center),
    )
    .padding(8)
    .style(container::bordered_box)
    .into()
}

/// Le formulaire de création, en bas de la vue.
fn creation<'a>(state: &'a State, enabled: bool) -> Element<'a, Message> {
    container(
        row![
            text_input(i18n::t(Text::CableNamePlaceholder), &state.new_name)
                .on_input(Message::NewCableName)
                .on_submit(Message::AddCable)
                .width(NAME_WIDTH),
            pick_list(CHOICES, Some(state.new_channels), Message::NewCableChannels),
            button(text(i18n::t(Text::Add))).on_press_maybe(enabled.then_some(Message::AddCable)),
        ]
        .spacing(10)
        .align_y(Center),
    )
    .padding(8)
    .style(container::bordered_box)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_channel_count_is_offered() {
        assert_eq!(CHOICES.len(), ChannelCount::MAX as usize);
        for c in CHOICES {
            assert_eq!(ChannelCount::from(c).get(), c.0, "canaux {c} non convertis");
        }
        assert_eq!(CHOICES[0].to_string(), "1 canal");
        assert_eq!(CHOICES[7].to_string(), "8 canaux");
    }

    #[test]
    fn a_new_cable_defaults_to_stereo_without_name() {
        let s = State::default();
        assert!(s.new_name.is_empty());
        assert_eq!(ChannelCount::from(s.new_channels), ChannelCount::STEREO);
        assert!(s.renaming.is_none() && s.removing.is_none());
    }
}
