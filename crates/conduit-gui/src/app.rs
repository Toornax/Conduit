//! Application `iced` : architecture Elm ([`Message`], [`App::update`],
//! [`App::view`], [`App::subscription`]).
//!
//! Aucune logique audio ici : l'application traduit des clics en commandes du
//! protocole, et des notifications en changements d'état.

use std::path::PathBuf;
use std::time::Duration;

use iced::widget::{column, container, text};
use iced::{Element, Fill, Subscription, Task};

use crate::i18n::{self, Text};
use crate::ipc::{self, Requester};
use crate::model::Mirror;

/// État de la connexion au démon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Connection {
    /// La couche IPC n'a pas encore démarré.
    #[default]
    Starting,
    /// Tentative de connexion en cours.
    Connecting,
    /// Connectée et abonnée aux notifications.
    Ready {
        /// Identification du démon.
        server: String,
    },
    /// Connexion perdue ou impossible ; nouvelle tentative programmée.
    Lost {
        /// Cause.
        reason: String,
        /// Délai avant la tentative suivante.
        retry_in: Duration,
    },
}

impl Connection {
    /// Vrai si le démon répond.
    pub fn is_ready(&self) -> bool {
        matches!(self, Connection::Ready { .. })
    }

    /// Résumé d'une ligne, affiché en tête de fenêtre.
    pub fn summary(&self) -> String {
        match self {
            Connection::Starting => i18n::t(Text::Starting).to_string(),
            Connection::Connecting => i18n::t(Text::Connecting).to_string(),
            Connection::Ready { server } => {
                format!(
                    "{} — {} {server}",
                    i18n::t(Text::Connected),
                    i18n::t(Text::Server)
                )
            }
            Connection::Lost { retry_in, .. } => format!(
                "{} — {}",
                i18n::t(Text::DaemonMissing),
                i18n::reconnecting_in(*retry_in)
            ),
        }
    }

    /// Marche à suivre proposée à l'utilisateur, s'il y en a une.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Connection::Lost { .. } => Some(i18n::t(Text::DaemonMissingHint)),
            _ => None,
        }
    }
}

/// Message de l'application.
#[derive(Debug, Clone)]
pub enum Message {
    /// Événement de la couche IPC.
    Ipc(ipc::Event),
}

/// État complet de l'application.
#[derive(Debug)]
pub struct App {
    socket: PathBuf,
    connection: Connection,
    requester: Option<Requester>,
    mirror: Mirror,
}

impl App {
    /// Construit l'état initial pour un socket donné.
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            connection: Connection::Starting,
            requester: None,
            mirror: Mirror::default(),
        }
    }

    /// Socket surveillé.
    pub fn socket(&self) -> &PathBuf {
        &self.socket
    }

    /// État de la connexion.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Miroir de l'état du démon.
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// Titre de la fenêtre.
    pub fn title(&self) -> String {
        i18n::t(Text::AppTitle).to_string()
    }

    /// Réduit un message en modification d'état, éventuellement suivie d'une
    /// tâche asynchrone.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ipc(event) => self.apply_ipc(event),
        }
        Task::none()
    }

    /// Réduction d'un événement IPC (sans entrée/sortie : testable seul).
    pub fn apply_ipc(&mut self, event: ipc::Event) {
        match event {
            ipc::Event::Started(requester) => self.requester = Some(requester),
            ipc::Event::Connecting => self.connection = Connection::Connecting,
            ipc::Event::Ready { server, snapshot } => {
                self.mirror.reset(*snapshot);
                self.connection = Connection::Ready { server };
            }
            ipc::Event::Lost { reason, retry_in } => {
                self.connection = Connection::Lost { reason, retry_in };
            }
            // L'état ne suit que les notifications : aucun optimisme local.
            ipc::Event::Notified(notification) => self.mirror.apply(&notification),
            ipc::Event::Failed(_) => {}
        }
    }

    /// Flux d'événements du démon, avec reconnexion automatique.
    ///
    /// La subscription est identifiée par le chemin du socket : elle survit à
    /// tous les redessins et n'est relancée que si le socket change.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::run_with(self.socket.clone(), |socket: &PathBuf| {
            let socket = socket.clone();
            iced::stream::channel(64, async move |output| {
                ipc::run(socket, output).await;
            })
        })
        .map(Message::Ipc)
    }

    /// Fenêtre.
    pub fn view(&self) -> Element<'_, Message> {
        let mut chrome = column![
            text(i18n::t(Text::AppTitle)).size(28),
            text(self.connection.summary()).size(16),
            text(format!(
                "{} : {}",
                i18n::t(Text::Socket),
                self.socket.display()
            ))
            .size(12),
        ]
        .spacing(8);
        if let Some(hint) = self.connection.hint() {
            chrome = chrome.push(text(hint).size(14));
        }
        container(chrome)
            .padding(16)
            .width(Fill)
            .height(Fill)
            .into()
    }
}

/// Puits d'événements de la `Subscription` d'`iced`.
impl ipc::EventSink for iced::futures::channel::mpsc::Sender<ipc::Event> {
    async fn send(&mut self, event: ipc::Event) -> Result<(), ipc::Closed> {
        use iced::futures::SinkExt;
        SinkExt::send(self, event).await.map_err(|_| ipc::Closed)
    }
}

/// Ouvre la fenêtre principale et tourne jusqu'à sa fermeture.
pub fn run(socket: PathBuf) -> iced::Result {
    iced::application(move || App::new(socket.clone()), App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::fixtures::snapshot;

    fn app() -> App {
        App::new(PathBuf::from("/tmp/conduitd.sock"))
    }

    #[test]
    fn connection_states_follow_the_ipc_events() {
        let mut a = app();
        assert_eq!(*a.connection(), Connection::Starting);
        a.apply_ipc(ipc::Event::Connecting);
        assert_eq!(*a.connection(), Connection::Connecting);
        a.apply_ipc(ipc::Event::Ready {
            server: "conduitd 0.1.0".into(),
            snapshot: Box::new(snapshot()),
        });
        assert!(a.connection().is_ready());
        assert!(a.connection().summary().contains("conduitd 0.1.0"));
        assert!(a.mirror().is_loaded());
        assert_eq!(a.mirror().cables.len(), 2);
        assert!(a.connection().hint().is_none());
        a.apply_ipc(ipc::Event::Lost {
            reason: "connexion perdue".into(),
            retry_in: Duration::from_secs(2),
        });
        assert!(!a.connection().is_ready());
        assert!(a.connection().summary().contains("2 s"));
        assert!(a.connection().hint().is_some());
    }
}
