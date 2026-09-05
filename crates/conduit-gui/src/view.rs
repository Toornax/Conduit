//! Décor commun de la fenêtre : barre de navigation, état de la connexion,
//! bannière d'erreur et pages encore à venir.

use std::path::Path;

use iced::widget::{button, column, container, row, space, text};
use iced::{Center, Element, Fill};

use crate::app::{Connection, Message};
use crate::i18n::{self, Text};

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

    /// Libellé de l'onglet.
    pub fn label(self) -> &'static str {
        i18n::t(match self {
            Tab::Cables => Text::TabCables,
            Tab::Patchbay => Text::TabPatchbay,
            Tab::Diagnostic => Text::TabDiagnostic,
        })
    }
}

/// Barre de navigation : un bouton par onglet, l'onglet courant mis en avant.
pub fn navigation(active: Tab) -> Element<'static, Message> {
    let mut bar = row![].spacing(6).align_y(Center);
    for tab in Tab::ALL {
        let style = if tab == active {
            button::primary
        } else {
            button::secondary
        };
        bar = bar.push(
            button(text(tab.label()))
                .on_press(Message::Tab(tab))
                .style(style),
        );
    }
    bar.width(Fill).into()
}

/// Ligne d'état : résumé de la connexion, marche à suivre et socket surveillé.
pub fn status_line<'a>(connection: &Connection, socket: &'a Path) -> Element<'a, Message> {
    let mut lines = column![text(connection.summary()).size(14)].spacing(2);
    if let Some(hint) = connection.hint() {
        lines = lines.push(text(hint).size(12));
    }
    lines
        .push(text(format!("{} : {}", i18n::t(Text::Socket), socket.display())).size(11))
        .into()
}

/// Bannière d'erreur : le message du démon dit déjà quoi faire, il est affiché
/// tel quel.
pub fn banner(message: &str) -> Element<'_, Message> {
    container(
        row![
            text(message).size(14).width(Fill),
            button(text(i18n::t(Text::Dismiss)))
                .on_press(Message::DismissError)
                .style(button::secondary),
        ]
        .spacing(10)
        .align_y(Center),
    )
    .padding(10)
    .width(Fill)
    .style(container::danger)
    .into()
}

/// Page encore vide : un seul texte annonçant ce qui arrive.
pub fn placeholder(what: Text) -> Element<'static, Message> {
    column![text(i18n::t(what)).size(14), space::vertical().height(Fill),]
        .spacing(8)
        .width(Fill)
        .height(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_tabs_are_labelled_in_french() {
        assert_eq!(Tab::default(), Tab::Cables);
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["Câbles", "Patchbay", "Diagnostic"]
        );
    }
}
