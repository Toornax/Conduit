//! Application `iced` : architecture Elm ([`Message`], [`App::update`],
//! [`App::view`], [`App::subscription`]).
//!
//! Aucune logique audio ici : l'application traduit des clics en commandes du
//! protocole, et des notifications en changements d'état.

use std::path::PathBuf;
use std::time::Duration;

use conduit_backend::{CableId, CableSpec};
use conduit_core::graph::{LinkId, NodeId};
use conduit_core::types::{ChannelCount, Db};
use iced::keyboard::{self, key::Named};
use iced::{Element, Subscription, Task};

use conduit_protocol::api::InternalKind;
use conduit_protocol::{Command, Notification};

use crate::i18n::{self, Text};
use crate::ipc::{self, Requester};
use crate::model::Mirror;
use crate::preferences::Preferences;
use crate::shell::{self, Tab};
use crate::{cables, format, patchbay, theme, typo};

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

/// Notice éphémère affichée sous l'en-tête.
///
/// Le message du démon dit déjà quoi faire (ADR-006) : il est affiché tel
/// quel, sans reformulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Texte affiché.
    pub texte: String,
    /// Vrai pour une erreur : le bandeau prend alors le filet garance.
    pub erreur: bool,
}

impl Notice {
    /// Une notice d'erreur.
    pub fn erreur(texte: impl Into<String>) -> Self {
        Self {
            texte: texte.into(),
            erreur: true,
        }
    }

    /// Une notice d'information.
    pub fn info(texte: impl Into<String>) -> Self {
        Self {
            texte: texte.into(),
            erreur: false,
        }
    }
}

/// Ce que l'interface attend d'une commande déjà envoyée, pour nommer le
/// câble dans la notice que la notification posera.
///
/// La GUI n'anticipe rien : la notice n'est écrite qu'au vu du `CableChanged`
/// correspondant, jamais au moment du clic.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attente {
    /// Un `CableAdd` a été envoyé : le prochain câble annoncé est le sien.
    Creation,
    /// Un `CableRemove` a été envoyé pour ce câble.
    Suppression(CableId),
    /// Un `Unlink` a été envoyé pour ce lien.
    Delien(LinkId),
    /// Un `AddInternal` a été envoyé sous ce nom.
    Generateur(String),
}

/// Temps d'affichage d'une notice avant son effacement.
pub const DUREE_NOTICE: Duration = Duration::from_millis(4_500);

/// Message de l'application.
#[derive(Debug, Clone)]
pub enum Message {
    /// Événement de la couche IPC.
    Ipc(ipc::Event),
    /// Change d'onglet.
    Tab(Tab),
    /// Affiche une notice, qui s'effacera d'elle-même.
    Notice(Notice),
    /// Fin du minuteur de la notice de génération donnée ; une notice plus
    /// récente n'est pas effacée.
    NoticeExpiree(u32),
    /// Crée un câble stéréo sans nom (`CableAdd`) : le démon le nomme, et
    /// l'utilisateur lui donne ensuite un alias dans sa ligne.
    AddCable,
    /// Demande confirmation avant de supprimer un câble.
    AskRemove(CableId),
    /// Referme la confirmation ou l'édition de nom en cours.
    CancelDialog,
    /// Supprime un câble, confirmation faite (`CableRemove`).
    RemoveCable(CableId),
    /// Alias saisi dans la ligne d'un câble.
    AliasEdited(CableId, String),
    /// Applique l'alias en cours de saisie (`CableRename`).
    CommitAlias,
    /// Change les canaux d'un câble (`CableSetChannels`).
    SetChannels(CableId, ChannelCount),
    /// Geste de l'utilisateur sur la scène du patchbay.
    ///
    /// Déplacer une carte ou sélectionner un lien n'envoie rien : ce sont des
    /// réglages locaux d'affichage. Tirer ou supprimer un lien, en revanche,
    /// se termine en commande du protocole (voir [`patchbay::Effet`]).
    Patchbay(patchbay::Geste),
    /// Crée un générateur de sinus de test (`AddInternal`).
    AddGenerator,
    /// Préférences lues au démarrage.
    Preferences(Box<Preferences>),
    /// Écriture des préférences terminée ; son résultat n'a rien à annoncer.
    PreferencesEcrites,
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
    patchbay: patchbay::State,
    notice: Option<Notice>,
    attente: Option<Attente>,
    generation: u32,
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
            patchbay: patchbay::State::default(),
            notice: None,
            attente: None,
            generation: 0,
            mode: iced::theme::Mode::default(),
        }
    }

    /// Construit l'état initial et les tâches de démarrage : demander au
    /// système son mode clair ou sombre, et relire les préférences.
    ///
    /// La lecture du fichier est faite dans une tâche : `new` reste pur, et
    /// les tests n'ouvrent jamais le fichier de l'utilisateur.
    pub fn boot(socket: PathBuf) -> (Self, Task<Message>) {
        (
            Self::new(socket),
            Task::batch([
                iced::system::theme().map(Message::ThemeSysteme),
                Task::perform(lire_les_preferences(), |preferences| {
                    Message::Preferences(Box::new(preferences))
                }),
            ]),
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

    /// État d'affichage de la vue Patchbay.
    pub fn patchbay(&self) -> &patchbay::State {
        &self.patchbay
    }

    /// Notice affichée sous l'en-tête, s'il y en a une.
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
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

    /// Met une commande en file et efface la notice courante : le résultat
    /// arrivera par notification, ou par une nouvelle notice.
    fn request(&mut self, command: Command) {
        self.notice = None;
        // Un réglage de gain ou de coupure ne produit aucune notification : le
        // protocole y répond `Reply::Ok` et rien d'autre. Plutôt que de
        // supposer localement le résultat, on redemande derrière l'état réel,
        // dont la réponse revient en `ipc::Event::Relu`.
        let relecture = relecture_apres(&command);
        if let Some(requester) = &self.requester {
            requester.send(command);
            if let Some(relecture) = relecture {
                requester.send(relecture);
            }
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
            Message::Ipc(event) => {
                let avant = self.generation;
                self.apply_ipc(event);
                // Une notice posée par l'événement a besoin de son minuteur.
                if self.generation != avant {
                    return self.minuteur();
                }
            }
            Message::Tab(tab) => {
                self.tab = tab;
                self.cables.close_dialogs();
            }
            Message::Notice(notice) => {
                self.poser_notice(notice);
                return self.minuteur();
            }
            Message::NoticeExpiree(generation) => {
                // Le minuteur d'une notice remplacée ne doit pas effacer la
                // suivante : seule la génération courante s'efface.
                if self.generation == generation {
                    self.notice = None;
                }
            }
            Message::AddCable => {
                // Le câble naît stéréo et sans nom : le démon le nomme, et
                // l'alias se saisit ensuite dans la ligne.
                let spec = CableSpec {
                    name: None,
                    channels: ChannelCount::STEREO,
                };
                self.attente = Some(Attente::Creation);
                self.request(Command::CableAdd { spec });
            }
            Message::AskRemove(id) => {
                self.cables.close_dialogs();
                self.cables.removing = Some(id);
            }
            Message::CancelDialog => self.cables.close_dialogs(),
            Message::RemoveCable(id) => {
                self.cables.close_dialogs();
                self.attente = Some(Attente::Suppression(id));
                self.request(Command::CableRemove { id });
            }
            Message::AliasEdited(id, value) => {
                // Passer à une autre ligne valide l'alias en cours plutôt que
                // de le perdre.
                if !matches!(&self.cables.renaming, Some((edite, _)) if *edite == id) {
                    self.commit_alias();
                }
                self.cables.renaming = Some((id, value));
            }
            Message::CommitAlias => self.commit_alias(),
            Message::SetChannels(id, channels) => {
                self.request(Command::CableSetChannels { id, channels });
                // L'avertissement de la maquette : le câble est recréé côté
                // système, le son se tait un instant.
                self.poser_notice(Notice::info(i18n::channels_changed(
                    &id.to_string(),
                    channels.get(),
                )));
                return self.minuteur();
            }
            Message::Patchbay(geste) => {
                // Le geste se réduit contre la disposition courante : c'est
                // elle qui dit où la carte saisie se trouvait, et contre les
                // liens du miroir, qui disent ce qui est déjà branché.
                let placements = patchbay::disposer(&self.mirror.nodes, &self.patchbay.positions);
                let effet = self.patchbay.reduire(
                    geste,
                    &placements,
                    &self.mirror.nodes,
                    &self.mirror.links,
                );
                return self.appliquer(effet);
            }
            Message::AddGenerator => {
                // Le nom doit être libre : le démon refuse deux nœuds internes
                // homonymes.
                let name = patchbay::nom_de_generateur(&self.mirror.nodes);
                self.attente = Some(Attente::Generateur(name.clone()));
                self.request(Command::AddInternal {
                    name,
                    kind: InternalKind::Sine {
                        frequency: patchbay::FREQUENCE_TEST,
                        amplitude: patchbay::amplitude_test(),
                        channels: patchbay::CANAUX_TEST,
                    },
                });
            }
            Message::Preferences(preferences) => {
                // Les positions lues ne remplacent pas celles d'un
                // déplacement déjà commencé : la lecture arrive au démarrage,
                // avant tout geste.
                if self.patchbay.positions.is_empty() {
                    self.patchbay.positions = preferences.patchbay;
                }
            }
            Message::PreferencesEcrites => {}
            Message::ThemeSysteme(mode) => self.mode = mode,
        }
        Task::none()
    }

    /// Donne suite à ce qu'un geste du patchbay demande.
    ///
    /// Le refus de boucle est la seule décision prise sans le démon (F-12), et
    /// c'est un refus : rien n'est ajouté ni retiré du miroir ici.
    fn appliquer(&mut self, effet: patchbay::Effet) -> Task<Message> {
        match effet {
            patchbay::Effet::Rien => {}
            patchbay::Effet::Enregistrer => return self.enregistrer_preferences(),
            patchbay::Effet::Boucle { depuis, vers } => {
                self.poser_notice(Notice::erreur(i18n::lien_refuse_boucle(
                    &self.libelle_noeud(depuis),
                    &self.libelle_noeud(vers),
                )));
                return self.minuteur();
            }
            patchbay::Effet::Commande(commande) => {
                // La notice de suppression attend, comme les autres, la
                // notification qui la confirme.
                if let Command::Unlink { link } = &commande {
                    self.attente = Some(Attente::Delien(*link));
                }
                // La notice de coupure, elle, se compose avant l'envoi : elle
                // nomme le lien, que la commande partie ne dit plus.
                let coupure = self.notice_de_coupure(&commande);
                self.request(commande);
                if let Some(notice) = coupure {
                    self.poser_notice(notice);
                    return self.minuteur();
                }
            }
        }
        Task::none()
    }

    /// La notice qu'une coupure mérite, si cette commande en est une.
    ///
    /// Elle est posée **au clic**, et non au vu d'une notification comme celles
    /// des câbles et des liens : le protocole n'annonce aucun changement de
    /// gain — ni `SetNodeGain` ni `SetLinkGain` ne produit de notification. La
    /// phrase dit donc ce qui a été demandé ; si le démon refuse, la sienne la
    /// remplace ([`ipc::Event::Failed`]).
    ///
    /// Un réglage de gain, lui, n'a droit à aucune notice : c'est un geste
    /// continu, et une phrase par mouvement serait du bruit.
    fn notice_de_coupure(&self, commande: &Command) -> Option<Notice> {
        match commande {
            Command::SetNodeGain {
                node,
                muted: Some(coupe),
                ..
            } => Some(Notice::info(i18n::noeud_coupe(
                &self.libelle_noeud(*node),
                *coupe,
            ))),
            Command::SetLinkGain {
                link,
                muted: Some(coupe),
                ..
            } => {
                let (source, destination) = self.extremites_du_lien(*link)?;
                Some(Notice::info(i18n::lien_coupe(
                    &source,
                    &destination,
                    *coupe,
                )))
            }
            _ => None,
        }
    }

    /// Le libellé d'un nœud du miroir, ou son identifiant s'il a disparu.
    fn libelle_noeud(&self, id: NodeId) -> String {
        self.mirror
            .node(id)
            .map_or_else(|| id.to_string(), |n| n.label.clone())
    }

    /// Les libellés des deux extrémités d'un lien, s'il est encore au miroir.
    fn extremites_du_lien(&self, id: LinkId) -> Option<(String, String)> {
        let lien = self.mirror.link(id)?;
        Some((
            self.libelle_noeud(lien.link.src.node),
            self.libelle_noeud(lien.link.dst.node),
        ))
    }

    /// Écrit les positions du patchbay hors du fil de l'interface.
    ///
    /// `update` ne touche jamais au disque : l'écriture part dans une tâche,
    /// et son résultat n'est rien à annoncer — une écriture manquée fait
    /// seulement repartir la disposition automatiquement.
    fn enregistrer_preferences(&self) -> Task<Message> {
        let positions = self.patchbay.positions.clone();
        Task::perform(ecrire_les_preferences(positions), |()| {
            Message::PreferencesEcrites
        })
    }

    /// Envoie l'alias en cours de saisie, s'il en vaut la peine.
    ///
    /// Un alias vide n'est pas une demande, et un alias identique à celui que
    /// le démon connaît déjà n'a rien à dire : dans les deux cas l'édition se
    /// referme sans commande.
    fn commit_alias(&mut self) {
        let Some((id, name)) = self.cables.renaming.take() else {
            return;
        };
        let name = name.trim().to_string();
        let inchange = self.mirror.cable(id).is_some_and(|c| c.name == name);
        if !name.is_empty() && !inchange {
            self.request(Command::CableRename { id, name });
        }
    }

    /// Pose une notice et ouvre une génération : le minuteur de la notice
    /// précédente devient sans effet.
    fn poser_notice(&mut self, notice: Notice) {
        self.notice = Some(notice);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Le minuteur qui effacera la notice courante au bout de
    /// [`DUREE_NOTICE`], si elle est encore la plus récente.
    fn minuteur(&self) -> Task<Message> {
        let generation = self.generation;
        Task::perform(attendre_la_notice(), move |()| {
            Message::NoticeExpiree(generation)
        })
    }

    /// Réduction d'un événement IPC (sans entrée/sortie : testable seul).
    pub fn apply_ipc(&mut self, event: ipc::Event) {
        match event {
            ipc::Event::Started(requester) => self.requester = Some(requester),
            ipc::Event::Connecting => self.connection = Connection::Connecting,
            ipc::Event::Ready { server, snapshot } => {
                self.mirror.reset(*snapshot);
                self.cables.close_dialogs();
                self.patchbay.recharge();
                self.notice = None;
                self.attente = None;
                self.connection = Connection::Ready { server };
            }
            ipc::Event::Lost { reason, retry_in } => {
                self.connection = Connection::Lost { reason, retry_in };
            }
            // L'état ne suit que les notifications : aucun optimisme local.
            ipc::Event::Notified(notification) => {
                // La notice se décide avant la réduction : c'est l'ancien
                // miroir qui dit si le câble annoncé est nouveau.
                let notice = self.notice_attendue(&notification);
                self.mirror.apply(&notification);
                // Le miroir a parlé : la valeur qu'une glissière montrait en
                // avance sur lui n'a plus lieu d'être.
                self.patchbay.notifie(&notification);
                if let Some(notice) = notice {
                    self.poser_notice(notice);
                }
            }
            // Une relecture demandée après un réglage de gain : le démon vient
            // de dire l'état réel, la valeur montrée en avance n'a plus lieu
            // d'être.
            ipc::Event::Relu(relecture) => match *relecture {
                ipc::Relecture::Nodes(nodes) => {
                    self.mirror.remplace_nodes(nodes);
                    self.patchbay.relu();
                }
                ipc::Relecture::Links(links) => {
                    self.mirror.remplace_links(links);
                    self.patchbay.relu();
                }
            },
            // Le message du démon dit déjà quoi faire (ADR-006).
            ipc::Event::Failed(message) => {
                // La commande n'a pas abouti : plus rien n'est attendu.
                self.attente = None;
                self.poser_notice(Notice::erreur(message));
            }
        }
    }

    /// La notice que cette notification mérite, si elle est la réponse à la
    /// commande attendue.
    ///
    /// À appeler **avant** [`Mirror::apply`](crate::model::Mirror::apply) : la
    /// création se reconnaît à ce que le câble annoncé est inconnu du miroir.
    fn notice_attendue(&mut self, notification: &Notification) -> Option<Notice> {
        let texte = match (self.attente.clone()?, notification) {
            (Attente::Creation, Notification::CableChanged { id, info: Some(_) })
                if self.mirror.cable(*id).is_none() =>
            {
                i18n::cable_created(&id.to_string())
            }
            (Attente::Suppression(cible), Notification::CableChanged { id, info: None })
                if cible == *id =>
            {
                i18n::cable_removed(&id.to_string())
            }
            // Le lien est encore au miroir : ses deux extrémités se nomment
            // avant que la réduction ne l'efface.
            (Attente::Delien(cible), Notification::LinkRemoved { id }) if cible == *id => {
                let (source, destination) = self.extremites_du_lien(*id)?;
                i18n::lien_supprime(&source, &destination)
            }
            (Attente::Generateur(nom), Notification::NodeAdded(node)) if node.label == nom => {
                i18n::generateur_cree(
                    &nom,
                    &format::hertz(patchbay::FREQUENCE_TEST),
                    &format::decibels(Db::new(patchbay::NIVEAU_TEST_DB)),
                )
            }
            _ => return None,
        };
        self.attente = None;
        Some(Notice::info(texte))
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
        Subscription::batch([demon, systeme, self.clavier()])
    }

    /// Le clavier de la vue Patchbay : Suppr et Retour arrière retirent le
    /// lien sélectionné.
    ///
    /// L'abonnement est **borné à l'onglet** : la vue Câbles a des champs de
    /// texte, où Retour arrière efface une lettre et ne supprime rien.
    /// [`keyboard::listen`] ne rapporte de toute façon que les touches
    /// qu'aucun widget n'a prises, mais l'onglet est une garantie qui ne
    /// dépend pas de cette subtilité.
    fn clavier(&self) -> Subscription<Message> {
        if self.tab != Tab::Patchbay {
            return Subscription::none();
        }
        keyboard::listen().filter_map(|evenement| match evenement {
            keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(Named::Delete | Named::Backspace),
                ..
            } => Some(Message::Patchbay(patchbay::Geste::Supprimer)),
            _ => None,
        })
    }

    /// Fenêtre : la coquille (barre latérale, en-tête, notice) et, dedans, la
    /// vue de l'onglet courant.
    pub fn view(&self) -> Element<'_, Message> {
        let enabled = self.connection.is_ready();
        let graisse = theme::jetons(&self.theme()).graisse_texte;
        let (actions, contenu) = match self.tab {
            Tab::Cables => (
                vec![shell::action_primaire(
                    Text::CablesAdd,
                    enabled.then_some(Message::AddCable),
                )],
                cables::view(&self.mirror, &self.cables, enabled, graisse),
            ),
            Tab::Patchbay => {
                let mut actions = Vec::new();
                // Le gain du lien et « Supprimer le lien » n'existent que s'il
                // y a un lien : la maquette ne montre pas d'action grisée ici.
                // Le réglage de gain n'est pas dans la maquette du tout — un
                // lien est une courbe, il n'a pas de carte où le poser —, mais
                // F-13 l'exige : il va là où le lien est déjà l'objet courant.
                if let Some(lien) = self
                    .patchbay
                    .lien_selectionne(&self.mirror.links)
                    .and_then(|id| self.mirror.link(id))
                {
                    actions.push(patchbay::reglage_de_lien(
                        lien,
                        &self.patchbay,
                        enabled,
                        graisse,
                    ));
                    actions.push(shell::action_lien(
                        Text::PatchbayLinkRemove,
                        enabled.then_some(Message::Patchbay(patchbay::Geste::Supprimer)),
                    ));
                }
                actions.push(shell::action_secondaire(
                    Text::PatchbayGenerator,
                    enabled.then_some(Message::AddGenerator),
                ));
                (
                    actions,
                    patchbay::view(&self.mirror, &self.patchbay, graisse),
                )
            }
            Tab::Diagnostic => (Vec::new(), shell::a_venir(Text::DiagnosticSoon, graisse)),
        };
        shell::fenetre(
            self.tab,
            &self.connection,
            &self.mirror,
            self.notice.as_ref(),
            graisse,
            actions,
            contenu,
        )
    }
}

/// Attend le temps d'affichage d'une notice.
///
/// Le minuteur n'est armé qu'au premier `poll` : `tokio::time::sleep` exige un
/// réacteur, dont les tests de réduction n'ont pas besoin puisqu'ils laissent
/// la tâche sans l'exécuter.
async fn attendre_la_notice() {
    tokio::time::sleep(DUREE_NOTICE).await;
}

/// Lit le fichier de préférences hors du fil de l'interface.
async fn lire_les_preferences() -> Preferences {
    tokio::task::spawn_blocking(crate::preferences::lire)
        .await
        .unwrap_or_default()
}

/// Écrit le fichier de préférences hors du fil de l'interface.
///
/// L'échec n'est pas une erreur d'interface : il est journalisé et oublié.
async fn ecrire_les_preferences(positions: patchbay::Positions) {
    let ecriture =
        tokio::task::spawn_blocking(move || crate::preferences::ecrire(&positions)).await;
    if let Ok(Err(erreur)) = ecriture {
        tracing::warn!(%erreur, "préférences d'interface non écrites");
    }
}

/// La relecture d'état à envoyer derrière une commande, s'il en faut une.
///
/// Seuls les réglages de gain et de coupure en demandent une : le protocole ne
/// diffuse aucune notification pour eux, alors que toutes les autres commandes
/// de l'interface se reflètent dans une `Notification`.
fn relecture_apres(commande: &Command) -> Option<Command> {
    match commande {
        Command::SetNodeGain { .. } => Some(Command::Nodes),
        Command::SetLinkGain { .. } => Some(Command::Links),
        _ => None,
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

    /// Le réglage émis et la relecture qui le suit, en échouant s'il n'y a pas
    /// exactement ces deux commandes.
    fn sent_avec_relecture(rx: &mut mpsc::Receiver<Command>) -> (Command, Command) {
        let reglage = rx.try_recv().expect("aucune commande émise");
        let relecture = rx.try_recv().expect("aucune relecture émise");
        assert!(rx.try_recv().is_err(), "deux commandes attendues");
        (reglage, relecture)
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

    /// Le bouton d'en-tête crée un câble stéréo sans nom : c'est le démon qui
    /// le nomme, et l'alias se saisit ensuite dans la ligne.
    #[test]
    fn adding_a_cable_sends_a_stereo_cable_and_lets_the_daemon_name_it() {
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
        // Aucun optimisme local : le miroir attend la notification.
        assert_eq!(a.mirror().cables.len(), 2);
        assert!(a.notice().is_none(), "rien n'est annoncé avant le démon");
    }

    /// La notice de création nomme le câble, et ne le nomme qu'une fois le
    /// démon revenu avec son `CableChanged`.
    #[test]
    fn a_created_cable_is_announced_by_its_system_name() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AddCable);
        let _ = sent(&mut rx);
        let _ = a.update(Message::Ipc(ipc::Event::Notified(Box::new(
            Notification::CableChanged {
                id: CableId(3),
                info: Some(cable(3, "Conduit 3")),
            },
        ))));
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Conduit 3 créé. Il apparaît dans les réglages audio du système.")
        );
        assert!(!a.notice().unwrap().erreur);
        // Un second câble annoncé sans demande ne réannonce rien.
        let _ = a.update(Message::NoticeExpiree(a.generation));
        let _ = a.update(Message::Ipc(ipc::Event::Notified(Box::new(
            Notification::CableChanged {
                id: CableId(4),
                info: Some(cable(4, "Conduit 4")),
            },
        ))));
        assert!(a.notice().is_none());
    }

    /// La notice de suppression attend, elle aussi, la notification.
    #[test]
    fn a_removed_cable_is_announced_when_the_daemon_confirms() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AskRemove(CableId(2)));
        let _ = a.update(Message::RemoveCable(CableId(2)));
        let _ = sent(&mut rx);
        assert!(a.notice().is_none(), "rien n'est annoncé avant le démon");
        let _ = a.update(Message::Ipc(ipc::Event::Notified(Box::new(
            Notification::CableChanged {
                id: CableId(2),
                info: None,
            },
        ))));
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Conduit 2 supprimé.")
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

    /// L'alias s'édite en ligne : la saisie tient dans l'état, la validation
    /// envoie `CableRename`, un alias vide ou inchangé n'envoie rien.
    #[test]
    fn the_alias_is_edited_in_place_and_ignores_an_empty_or_unchanged_one() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AliasEdited(CableId(1), "  Jeu ".into()));
        assert_eq!(
            a.cables().renaming,
            Some((CableId(1), "  Jeu ".to_string()))
        );
        assert!(rx.try_recv().is_err(), "la saisie n'envoie rien");
        let _ = a.update(Message::CommitAlias);
        assert_eq!(
            sent(&mut rx),
            Command::CableRename {
                id: CableId(1),
                name: "Jeu".into(),
            }
        );
        assert!(a.cables().renaming.is_none());

        let _ = a.update(Message::AliasEdited(CableId(1), "   ".into()));
        let _ = a.update(Message::CommitAlias);
        assert!(rx.try_recv().is_err(), "un alias vide n'envoie rien");

        let _ = a.update(Message::AliasEdited(CableId(1), "Conduit 1".into()));
        let _ = a.update(Message::CommitAlias);
        assert!(rx.try_recv().is_err(), "un alias inchangé n'envoie rien");
    }

    /// Commencer à éditer une autre ligne valide l'édition en cours plutôt
    /// que de la perdre.
    #[test]
    fn editing_another_row_commits_the_pending_alias() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AliasEdited(CableId(1), "Jeu".into()));
        let _ = a.update(Message::AliasEdited(CableId(2), "Micro".into()));
        assert_eq!(
            sent(&mut rx),
            Command::CableRename {
                id: CableId(1),
                name: "Jeu".into(),
            }
        );
        assert_eq!(
            a.cables().renaming,
            Some((CableId(2), "Micro".to_string())),
            "la nouvelle ligne prend la main"
        );
    }

    /// Les pas de canaux envoient la valeur visée et annoncent le silence à
    /// venir ; l'accord suit le nombre.
    #[test]
    fn changing_the_channels_sends_cable_set_channels_and_warns() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::SetChannels(
            CableId(2),
            ChannelCount::new(6).unwrap(),
        ));
        assert_eq!(
            sent(&mut rx),
            Command::CableSetChannels {
                id: CableId(2),
                channels: ChannelCount::new(6).unwrap(),
            }
        );
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Conduit 2 passe à 6 canaux : réactivation du câble, court silence.")
        );
        let _ = a.update(Message::SetChannels(CableId(2), ChannelCount::MONO));
        let _ = sent(&mut rx);
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Conduit 2 passe à 1 canal : réactivation du câble, court silence.")
        );
        // Aucun optimisme local : le miroir garde les canaux du démon.
        assert_eq!(
            a.mirror().cable(CableId(2)).unwrap().channels,
            ChannelCount::STEREO
        );
    }

    #[test]
    fn a_refused_command_shows_a_notice_until_the_next_action() {
        let (mut a, mut rx) = connected();
        assert!(a.notice().is_none());
        a.apply_ipc(ipc::Event::Failed(
            "limite de 8 câbles atteinte : supprimez un câble".into(),
        ));
        let notice = a.notice().expect("une commande refusée pose une notice");
        assert!(notice.texte.contains("supprimez un câble"));
        assert!(notice.erreur, "une commande refusée est une erreur");
        let _ = a.update(Message::NoticeExpiree(a.generation));
        assert!(a.notice().is_none());
        a.apply_ipc(ipc::Event::Failed("câble inconnu".into()));
        let _ = a.update(Message::AddCable);
        assert!(a.notice().is_none(), "une nouvelle action efface la notice");
        let _ = sent(&mut rx);
    }

    /// Le compteur de génération : une notice neuve survit au minuteur de
    /// celle qu'elle a remplacée.
    #[test]
    fn a_new_notice_survives_the_previous_timer() {
        let (mut a, _rx) = connected();
        a.apply_ipc(ipc::Event::Failed("première".into()));
        let ancienne = a.generation;
        let _ = a.update(Message::Notice(Notice::info("seconde")));
        assert_eq!(a.notice().unwrap().texte, "seconde");
        assert!(!a.notice().unwrap().erreur);
        assert_ne!(a.generation, ancienne, "une notice ouvre une génération");
        let _ = a.update(Message::NoticeExpiree(ancienne));
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("seconde"),
            "le minuteur de l'ancienne notice n'efface pas la neuve"
        );
        let _ = a.update(Message::NoticeExpiree(a.generation));
        assert!(a.notice().is_none(), "son propre minuteur l'efface");
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

    /// Déplacer une carte du patchbay est un réglage local : aucune commande
    /// ne part, seule la position retenue change.
    #[test]
    fn moving_a_patchbay_card_sends_no_command() {
        let (mut a, mut rx) = connected();
        let cle = a.mirror().nodes[0].key.to_string();
        let placements = patchbay::disposer(&a.mirror().nodes, &a.patchbay().positions);
        let depart = placements[0].position;

        let _ = a.update(Message::Patchbay(patchbay::Geste::Saisi(cle.clone())));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Deplace(depart)));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Deplace(
            depart + iced::Vector::new(60.0, 20.0),
        )));
        assert_eq!(
            a.patchbay().positions.get(&cle),
            Some(depart + iced::Vector::new(60.0, 20.0))
        );
        let _ = a.update(Message::Patchbay(patchbay::Geste::Relache));
        assert!(!a.patchbay().deplacement());
        assert!(
            rx.try_recv().is_err(),
            "le patchbay n'envoie aucune commande"
        );
    }

    /// Les préférences lues au démarrage garnissent le patchbay, mais
    /// n'écrasent pas un déplacement déjà fait.
    #[test]
    fn the_preferences_seed_the_patchbay_without_overwriting_it() {
        let (mut a, _rx) = connected();
        let mut positions = patchbay::Positions::default();
        positions.set("internal:a", iced::Point::new(12.0, 34.0));
        let _ = a.update(Message::Preferences(Box::new(Preferences {
            patchbay: positions,
        })));
        assert_eq!(
            a.patchbay().positions.get("internal:a"),
            Some(iced::Point::new(12.0, 34.0))
        );
        // Une seconde lecture — la fenêtre a déjà des positions — ne remplace
        // rien.
        let mut tardives = patchbay::Positions::default();
        tardives.set("internal:a", iced::Point::new(0.0, 0.0));
        let _ = a.update(Message::Preferences(Box::new(Preferences {
            patchbay: tardives,
        })));
        assert_eq!(
            a.patchbay().positions.get("internal:a"),
            Some(iced::Point::new(12.0, 34.0))
        );
    }

    /// Tirer un lien d'une sortie vers une entrée envoie `Link` — et rien
    /// n'apparaît au miroir avant que le démon ne l'annonce.
    #[test]
    fn tirer_un_lien_envoie_link_sans_rien_anticiper() {
        use conduit_core::graph::{Direction, PortId};

        let (mut a, mut rx) = connected();
        a.apply_ipc(ipc::Event::Notified(Box::new(Notification::NodeAdded(
            crate::model::fixtures::node(2, "c"),
        ))));
        let src = PortId::new(NodeId::new(1, 0), Direction::Output, 0);
        let dst = PortId::new(NodeId::new(2, 0), Direction::Input, 0);
        let _ = a.update(Message::Patchbay(patchbay::Geste::DebutLien(src)));
        let _ = a.update(Message::Patchbay(patchbay::Geste::SurvolPort(Some(dst))));
        let _ = a.update(Message::Patchbay(patchbay::Geste::FinLien));
        assert_eq!(sent(&mut rx), Command::Link { src, dst });
        assert_eq!(a.mirror().links.len(), 1, "aucun lien ajouté localement");
    }

    /// Un lien qui refermerait une boucle est refusé sur place (F-12) : le
    /// démon n'en entend pas parler, et la notice nomme les deux nœuds.
    #[test]
    fn un_lien_qui_boucle_est_refuse_avec_une_notice_et_sans_commande() {
        use conduit_core::graph::{Direction, PortId};

        let (mut a, mut rx) = connected();
        // Le chargement initial branche déjà « a » sur « b ».
        let src = PortId::new(NodeId::new(1, 0), Direction::Output, 0);
        let dst = PortId::new(NodeId::new(0, 0), Direction::Input, 0);
        let _ = a.update(Message::Patchbay(patchbay::Geste::DebutLien(src)));
        let _ = a.update(Message::Patchbay(patchbay::Geste::SurvolPort(Some(dst))));
        let _ = a.update(Message::Patchbay(patchbay::Geste::FinLien));
        assert!(rx.try_recv().is_err(), "le refus n'atteint pas le démon");
        let notice = a.notice().expect("le refus se dit");
        assert_eq!(
            notice.texte,
            "Lien refusé : il créerait une boucle (« a » alimente déjà « b »)."
        );
        assert!(notice.erreur, "un refus prend le filet garance");
    }

    /// Sélectionner un lien puis le supprimer envoie `Unlink` ; la notice,
    /// elle, attend la notification qui la confirme.
    #[test]
    fn supprimer_un_lien_envoie_unlink_et_attend_la_notification() {
        let (mut a, mut rx) = connected();
        let lien = LinkId::new(0, 0);
        let _ = a.update(Message::Patchbay(patchbay::Geste::Selection(Some(lien))));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Supprimer));
        assert_eq!(sent(&mut rx), Command::Unlink { link: lien });
        assert_eq!(a.mirror().links.len(), 1, "rien n'est retiré localement");
        assert!(a.notice().is_none(), "rien n'est annoncé avant le démon");
        let _ = a.update(Message::Ipc(ipc::Event::Notified(Box::new(
            Notification::LinkRemoved { id: lien },
        ))));
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Lien supprimé entre « a » et « b ».")
        );
        assert!(a.mirror().links.is_empty());
    }

    /// Le générateur de test est un sinus à 440 Hz et à niveau prudent ; sa
    /// notice attend, elle aussi, le `NodeAdded` du démon.
    #[test]
    fn le_generateur_de_test_est_un_sinus_prudent_annonce_par_le_demon() {
        let (mut a, mut rx) = connected();
        let _ = a.update(Message::AddGenerator);
        let Command::AddInternal { name, kind } = sent(&mut rx) else {
            panic!("un AddInternal était attendu");
        };
        assert_eq!(name, "Générateur de test");
        assert_eq!(
            kind,
            InternalKind::Sine {
                frequency: 440.0,
                amplitude: patchbay::amplitude_test(),
                channels: 2,
            }
        );
        assert!(a.notice().is_none(), "rien n'est annoncé avant le démon");
        let _ = a.update(Message::Ipc(ipc::Event::Notified(Box::new(
            Notification::NodeAdded(crate::model::fixtures::node(9, "Générateur de test")),
        ))));
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some(
                "« Générateur de test » créé : sinus à 440\u{a0}Hz, \u{2212}12,0\u{a0}dB. \
                 Reliez sa sortie à une entrée pour l'entendre."
            )
        );
    }

    /// La glissière de gain d'un nœud : rien pendant le geste, la commande au
    /// relâchement, et aucune notice — un geste continu n'en mérite pas.
    #[test]
    fn regler_un_gain_n_envoie_qu_au_relachement_et_sans_notice() {
        let (mut a, mut rx) = connected();
        let cible = patchbay::Cible::Noeud(NodeId::new(0, 0));
        for position in [-4.0, -9.0, -18.0] {
            let _ = a.update(Message::Patchbay(patchbay::Geste::Gain(cible, position)));
        }
        assert!(
            rx.try_recv().is_err(),
            "le glissement n'atteint pas le démon"
        );
        // Ce qui s'affiche entre-temps est la position du doigt, pas le
        // miroir, qui reste à son gain unité.
        assert_eq!(a.patchbay().gain_affiche(cible, Db::UNITY), Db::new(-18.0));
        assert_eq!(a.mirror().nodes[0].gain_db, Db::UNITY);

        let _ = a.update(Message::Patchbay(patchbay::Geste::FinGain(cible)));
        let (reglage, relecture) = sent_avec_relecture(&mut rx);
        assert_eq!(
            reglage,
            Command::SetNodeGain {
                node: NodeId::new(0, 0),
                gain_db: Some(Db::new(-18.0)),
                muted: None,
            }
        );
        // Le protocole ne notifie pas les gains : l'interface redemande.
        assert_eq!(relecture, Command::Nodes);
        assert!(a.notice().is_none(), "un réglage de gain ne se raconte pas");

        // La relecture rend la main au miroir.
        let mut noeud = crate::model::fixtures::node(0, "a");
        noeud.gain_db = Db::new(-18.0);
        a.apply_ipc(ipc::Event::Relu(Box::new(ipc::Relecture::Nodes(vec![
            noeud,
        ]))));
        assert_eq!(a.mirror().nodes[0].gain_db, Db::new(-18.0));
        assert_eq!(
            a.patchbay().gain_affiche(cible, Db::new(-18.0)),
            Db::new(-18.0)
        );
    }

    /// La butée basse de la glissière vaut silence : c'est `NEG_INF` qui part,
    /// pas −60 dB.
    #[test]
    fn la_butee_basse_de_la_glissiere_envoie_le_silence() {
        let (mut a, mut rx) = connected();
        let cible = patchbay::Cible::Lien(LinkId::new(0, 0));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Gain(
            cible,
            patchbay::GAIN_MIN,
        )));
        let _ = a.update(Message::Patchbay(patchbay::Geste::FinGain(cible)));
        let (reglage, relecture) = sent_avec_relecture(&mut rx);
        assert_eq!(
            reglage,
            Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: Some(Db::NEG_INF),
                muted: None,
            }
        );
        assert_eq!(relecture, Command::Links);
    }

    /// Le bouton de coupure envoie l'inverse du miroir, pose sa notice et
    /// n'anticipe rien : le miroir garde son état.
    #[test]
    fn couper_un_noeud_envoie_l_inverse_et_pose_une_notice() {
        let (mut a, mut rx) = connected();
        let cible = patchbay::Cible::Noeud(NodeId::new(0, 0));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Muet(cible)));
        let (reglage, relecture) = sent_avec_relecture(&mut rx);
        assert_eq!(
            reglage,
            Command::SetNodeGain {
                node: NodeId::new(0, 0),
                gain_db: None,
                muted: Some(true),
            }
        );
        assert_eq!(relecture, Command::Nodes);
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("« a » coupé : plus aucun son n'en sort.")
        );
        assert!(!a.notice().unwrap().erreur);
        assert!(!a.mirror().nodes[0].muted, "rien n'est coupé localement");

        // Le lien sélectionné se coupe par la même porte, et sa notice nomme
        // ses deux extrémités.
        let _ = a.update(Message::Patchbay(patchbay::Geste::Muet(
            patchbay::Cible::Lien(LinkId::new(0, 0)),
        )));
        let (reglage, relecture) = sent_avec_relecture(&mut rx);
        assert_eq!(relecture, Command::Links);
        assert_eq!(
            reglage,
            Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: None,
                muted: Some(true),
            }
        );
        assert_eq!(
            a.notice().map(|n| n.texte.as_str()),
            Some("Lien coupé entre « a » et « b ».")
        );
    }

    /// Un nœud suspendu n'envoie rien, ni gain ni coupure.
    #[test]
    fn les_reglages_d_un_noeud_suspendu_n_emettent_rien() {
        let (mut a, mut rx) = connected();
        a.apply_ipc(ipc::Event::Notified(Box::new(
            Notification::NodeStateChanged {
                id: NodeId::new(0, 0),
                state: conduit_protocol::NodeState::Suspended,
            },
        )));
        let cible = patchbay::Cible::Noeud(NodeId::new(0, 0));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Gain(cible, -30.0)));
        let _ = a.update(Message::Patchbay(patchbay::Geste::FinGain(cible)));
        let _ = a.update(Message::Patchbay(patchbay::Geste::Muet(cible)));
        assert!(rx.try_recv().is_err(), "un nœud suspendu ne règle rien");
        assert!(a.notice().is_none());
        assert_eq!(a.patchbay().gain_affiche(cible, Db::UNITY), Db::UNITY);
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
