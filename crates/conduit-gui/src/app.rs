//! Application `iced` : architecture Elm ([`Message`], [`App::update`],
//! [`App::view`], [`App::subscription`]).
//!
//! Aucune logique audio ici : l'application traduit des clics en commandes du
//! protocole, et des notifications en changements d'état.

use std::path::PathBuf;
use std::time::Duration;

use conduit_backend::{CableId, CableSpec};
use iced::widget::{column, container};
use iced::{Element, Fill, Subscription, Task};

use conduit_protocol::Command;

use crate::cables::{self, Channels};
use crate::i18n::{self, Text};
use crate::ipc::{self, Requester};
use crate::model::Mirror;
use crate::view::{self, Tab};
use crate::{theme, typo};

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
    /// Change d'onglet.
    Tab(Tab),
    /// Ferme la bannière d'erreur.
    DismissError,
    /// Nom saisi pour le câble à créer.
    NewCableName(String),
    /// Canaux choisis pour le câble à créer.
    NewCableChannels(Channels),
    /// Crée le câble décrit par le formulaire (`CableAdd`).
    AddCable,
    /// Demande confirmation avant de supprimer un câble.
    AskRemove(CableId),
    /// Referme la confirmation ou l'édition de nom en cours.
    CancelDialog,
    /// Supprime un câble, confirmation faite (`CableRemove`).
    RemoveCable(CableId),
    /// Passe un câble en édition de nom.
    StartRename(CableId),
    /// Nom saisi pendant le renommage.
    RenameEdited(String),
    /// Applique le renommage en cours (`CableRename`).
    CommitRename,
    /// Change les canaux d'un câble (`CableSetChannels`).
    SetChannels(CableId, Channels),
    /// Mode clair ou sombre annoncé par le système, au démarrage puis à chaque
    /// changement.
    ThemeSysteme(iced::theme::Mode),
}

/// État complet de l'application.
#[derive(Debug)]
pub struct App {
    socket: PathBuf,
    connection: Connection,
    requester: Option<Requester>,
    mirror: Mirror,
    tab: Tab,
    cables: cables::State,
    error: Option<String>,
    mode: iced::theme::Mode,
}

impl App {
    /// Construit l'état initial pour un socket donné.
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            connection: Connection::Starting,
            requester: None,
            mirror: Mirror::default(),
            tab: Tab::default(),
            cables: cables::State::default(),
            error: None,
            mode: iced::theme::Mode::default(),
        }
    }

    /// Construit l'état initial et la tâche de démarrage : demander au système
    /// son mode clair ou sombre.
    pub fn boot(socket: PathBuf) -> (Self, Task<Message>) {
        (
            Self::new(socket),
            iced::system::theme().map(Message::ThemeSysteme),
        )
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

    /// Onglet affiché.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// État d'édition de la vue Câbles.
    pub fn cables(&self) -> &cables::State {
        &self.cables
    }

    /// Dernière erreur renvoyée par le démon, affichée en bannière.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Mode clair ou sombre annoncé par le système.
    pub fn mode(&self) -> iced::theme::Mode {
        self.mode
    }

    /// Thème « Sericæ » correspondant au mode du système.
    ///
    /// Renvoyer un thème et non `None` est indispensable : `None` ferait
    /// retomber `iced` sur ses thèmes intégrés.
    pub fn theme(&self) -> iced::Theme {
        theme::selon(self.mode)
    }

    /// Met une commande en file et efface la bannière d'erreur : le résultat
    /// arrivera par notification, ou par une nouvelle erreur.
    fn request(&mut self, command: Command) {
        self.error = None;
        if let Some(requester) = &self.requester {
            requester.send(command);
        }
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
            Message::Tab(tab) => {
                self.tab = tab;
                self.cables.close_dialogs();
            }
            Message::DismissError => self.error = None,
            Message::NewCableName(name) => self.cables.new_name = name,
            Message::NewCableChannels(channels) => self.cables.new_channels = channels,
            Message::AddCable => {
                let name = self.cables.new_name.trim();
                let spec = CableSpec {
                    name: (!name.is_empty()).then(|| name.to_string()),
                    channels: self.cables.new_channels.into(),
                };
                self.cables.new_name.clear();
                self.request(Command::CableAdd { spec });
            }
            Message::AskRemove(id) => {
                self.cables.close_dialogs();
                self.cables.removing = Some(id);
            }
            Message::CancelDialog => self.cables.close_dialogs(),
            Message::RemoveCable(id) => {
                self.cables.close_dialogs();
                self.request(Command::CableRemove { id });
            }
            Message::StartRename(id) => {
                let current = self
                    .mirror
                    .cable(id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.cables.close_dialogs();
                self.cables.renaming = Some((id, current));
            }
            Message::RenameEdited(value) => {
                if let Some((_, name)) = &mut self.cables.renaming {
                    *name = value;
                }
            }
            Message::CommitRename => {
                if let Some((id, name)) = self.cables.renaming.take() {
                    let name = name.trim().to_string();
                    // Un nom vide n'est pas une demande : on referme sans rien
                    // envoyer.
                    if !name.is_empty() {
                        self.request(Command::CableRename { id, name });
                    }
                }
            }
            Message::SetChannels(id, channels) => self.request(Command::CableSetChannels {
                id,
                channels: channels.into(),
            }),
            Message::ThemeSysteme(mode) => self.mode = mode,
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
                self.cables.close_dialogs();
                self.error = None;
                self.connection = Connection::Ready { server };
            }
            ipc::Event::Lost { reason, retry_in } => {
                self.connection = Connection::Lost { reason, retry_in };
            }
            // L'état ne suit que les notifications : aucun optimisme local.
            ipc::Event::Notified(notification) => self.mirror.apply(&notification),
            // Le message du démon dit déjà quoi faire (ADR-006).
            ipc::Event::Failed(message) => self.error = Some(message),
        }
    }

    /// Flux d'événements du démon, avec reconnexion automatique.
    ///
    /// La subscription est identifiée par le chemin du socket : elle survit à
    /// tous les redessins et n'est relancée que si le socket change.
    pub fn subscription(&self) -> Subscription<Message> {
        let demon = Subscription::run_with(self.socket.clone(), |socket: &PathBuf| {
            let socket = socket.clone();
            iced::stream::channel(64, async move |output| {
                ipc::run(socket, output).await;
            })
        })
        .map(Message::Ipc);
        let systeme = iced::system::theme_changes().map(Message::ThemeSysteme);
        Subscription::batch([demon, systeme])
    }

    /// Fenêtre : barre de navigation, état de la connexion, bannière
    /// d'erreur éventuelle, puis la page de l'onglet courant.
    pub fn view(&self) -> Element<'_, Message> {
        let mut page = column![
            view::navigation(self.tab),
            view::status_line(&self.connection, &self.socket),
        ]
        .spacing(10)
        .width(Fill);
        if let Some(error) = &self.error {
            page = page.push(view::banner(error));
        }
        let enabled = self.connection.is_ready();
        page = page.push(match self.tab {
            Tab::Cables => cables::view(&self.mirror, &self.cables, enabled),
            Tab::Patchbay => view::placeholder(Text::PatchbaySoon),
            Tab::Diagnostic => view::placeholder(Text::DiagnosticSoon),
        });
        container(page).padding(16).width(Fill).height(Fill).into()
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
///
/// Les quatre fichiers de police du thème « Sericæ » sont embarqués dans le
/// binaire et chargés ici (ADR-014) ; Inter est la police par défaut.
pub fn run(socket: PathBuf) -> iced::Result {
    iced::application(move || App::boot(socket.clone()), App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(App::theme)
        .font(typo::POLICE_FRAUNCES)
        .font(typo::POLICE_INTER)
        .font(typo::POLICE_SPECTRAL)
        .font(typo::POLICE_SPECTRAL_LIGHT)
        .default_font(typo::defaut())
        .window_size(FENETRE)
        .centered()
        .run()
}

/// Taille de la fenêtre à la première ouverture, en points logiques.
pub const FENETRE: (f32, f32) = (1120.0, 720.0);

#[cfg(test)]
mod tests {
    use super::*;

    use conduit_core::types::ChannelCount;
    use tokio::sync::mpsc;

    use crate::model::fixtures::{cable, snapshot};

    fn app() -> App {
        App::new(PathBuf::from("/tmp/conduitd.sock"))
    }

    /// Application connectée, avec le récepteur des commandes émises.
    fn connected() -> (App, mpsc::Receiver<Command>) {
        let (requester, rx) = Requester::channel(8);
        let mut a = app();
        a.apply_ipc(ipc::Event::Started(requester));
        a.apply_ipc(ipc::Event::Ready {
            server: "conduitd 0.1.0".into(),
            snapshot: Box::new(snapshot()),
        });
        (a, rx)
    }

    /// La commande émise, en échouant s'il n'y en a pas exactement une.
    fn sent(rx: &mut mpsc::Receiver<Command>) -> Command {
        let command = rx.try_recv().expect("aucune commande émise");
        assert!(rx.try_recv().is_err(), "une seule commande attendue");
        command
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

    #[test]
    fn tabs_switch_and_close_the_open_dialogs() {
        let (mut a, _rx) = connected();
        assert_eq!(a.tab(), Tab::Cables);
        let _ = a.update(Message::AskRemove(CableId(1)));
        assert_eq!(a.cables().removing, Some(CableId(1)));
        let _ = a.update(Message::Tab(Tab::Diagnostic));
        assert_eq!(a.tab(), Tab::Diagnostic);
        assert!(a.cables().removing.is_none());
    }

    #[test]
    fn adding_a_cable_sends_cable_add_and_clears_the_form() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::NewCableName("  Musique  ".into()));
        let _ = a.update(Message::NewCableChannels(Channels(4)));
        let _ = a.update(Message::AddCable);
        assert_eq!(
            sent(&mut rx),
            Command::CableAdd {
                spec: CableSpec {
                    name: Some("Musique".into()),
                    channels: ChannelCount::new(4).unwrap(),
                }
            }
        );
        assert!(a.cables().new_name.is_empty());
        // Aucun optimisme local : le miroir attend la notification.
        assert_eq!(a.mirror().cables.len(), 2);
    }

    #[test]
    fn an_unnamed_cable_lets_the_daemon_choose_the_name() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AddCable);
        assert_eq!(
            sent(&mut rx),
            Command::CableAdd {
                spec: CableSpec {
                    name: None,
                    channels: ChannelCount::STEREO,
                }
            }
        );
    }

    #[test]
    fn removing_a_cable_requires_a_confirmation() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AskRemove(CableId(2)));
        assert!(
            rx.try_recv().is_err(),
            "rien n'est envoyé avant la confirmation"
        );
        let _ = a.update(Message::CancelDialog);
        assert!(a.cables().removing.is_none());
        assert!(rx.try_recv().is_err());
        let _ = a.update(Message::AskRemove(CableId(2)));
        let _ = a.update(Message::RemoveCable(CableId(2)));
        assert_eq!(sent(&mut rx), Command::CableRemove { id: CableId(2) });
        assert!(a.cables().removing.is_none());
    }

    #[test]
    fn renaming_starts_from_the_current_name_and_ignores_an_empty_one() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::StartRename(CableId(1)));
        assert_eq!(
            a.cables().renaming,
            Some((CableId(1), "Conduit 1".to_string()))
        );
        let _ = a.update(Message::RenameEdited("  Jeu ".into()));
        let _ = a.update(Message::CommitRename);
        assert_eq!(
            sent(&mut rx),
            Command::CableRename {
                id: CableId(1),
                name: "Jeu".into(),
            }
        );
        assert!(a.cables().renaming.is_none());

        let _ = a.update(Message::StartRename(CableId(1)));
        let _ = a.update(Message::RenameEdited("   ".into()));
        let _ = a.update(Message::CommitRename);
        assert!(rx.try_recv().is_err(), "un nom vide n'envoie rien");
    }

    #[test]
    fn changing_the_channels_sends_cable_set_channels() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::SetChannels(CableId(2), Channels(6)));
        assert_eq!(
            sent(&mut rx),
            Command::CableSetChannels {
                id: CableId(2),
                channels: ChannelCount::new(6).unwrap(),
            }
        );
    }

    #[test]
    fn a_refused_command_shows_a_banner_until_the_next_action() {
        let (mut a, mut rx) = connected();
        assert!(a.error().is_none());
        a.apply_ipc(ipc::Event::Failed(
            "limite de 8 câbles atteinte : supprimez un câble".into(),
        ));
        assert!(a.error().unwrap().contains("supprimez un câble"));
        let _ = a.update(Message::DismissError);
        assert!(a.error().is_none());
        a.apply_ipc(ipc::Event::Failed("câble inconnu".into()));
        let _ = a.update(Message::AddCable);
        assert!(
            a.error().is_none(),
            "une nouvelle action efface la bannière"
        );
        let _ = sent(&mut rx);
    }

    /// Le thème suit le mode annoncé par le système, sans jamais rendre
    /// `None` — qui ferait retomber `iced` sur ses thèmes intégrés.
    #[test]
    fn the_theme_follows_the_system_mode() {
        let (mut a, _rx) = connected();
        assert_eq!(a.mode(), iced::theme::Mode::None);
        assert!(!a.theme().extended_palette().is_dark);
        let _ = a.update(Message::ThemeSysteme(iced::theme::Mode::Dark));
        assert_eq!(a.mode(), iced::theme::Mode::Dark);
        assert!(a.theme().extended_palette().is_dark);
        assert_eq!(*crate::theme::jetons(&a.theme()), crate::theme::SOMBRE);
        let _ = a.update(Message::ThemeSysteme(iced::theme::Mode::Light));
        assert!(!a.theme().extended_palette().is_dark);
    }

    /// `boot` construit le même état que `new`, plus la tâche d'interrogation
    /// du système.
    #[test]
    fn boot_starts_from_the_same_state_as_new() {
        let socket = PathBuf::from("/tmp/conduitd.sock");
        let (a, _task) = App::boot(socket.clone());
        assert_eq!(a.socket(), &socket);
        assert_eq!(*a.connection(), Connection::Starting);
        assert_eq!(a.mode(), iced::theme::Mode::None);
    }

    #[test]
    fn the_mirror_only_follows_the_notifications() {
        let (mut a, _rx) = connected();
        assert_eq!(a.mirror().cables.len(), 2);
        a.apply_ipc(ipc::Event::Notified(Box::new(
            conduit_protocol::Notification::CableChanged {
                id: CableId(3),
                info: Some(cable(3, "Jeu")),
            },
        )));
        assert_eq!(a.mirror().cables.len(), 3);
        assert_eq!(a.mirror().cable(CableId(3)).unwrap().name, "Jeu");
    }
}
