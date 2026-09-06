//! Couche IPC de l'interface : connexion, abonnement, envoi des commandes et
//! reconnexion automatique.
//!
//! Ce module **ne dépend pas d'`iced`** : il produit des [`Event`] dans un
//! [`EventSink`] quelconque, ce qui le rend testable sans ouvrir de fenêtre
//! (voir `tests/ipc.rs`). L'application branche simplement le puits de la
//! `Subscription` d'`iced` dessus.
//!
//! Cycle de vie d'une session :
//!
//! 1. [`Event::Started`] une seule fois, avec le [`Requester`] qui permet
//!    d'envoyer des commandes ;
//! 2. [`Event::Connecting`] avant chaque tentative ;
//! 3. connexion, `Subscribe` puis chargement initial, et [`Event::Ready`] ;
//! 4. [`Event::Notified`] pour chaque notification du démon,
//!    [`Event::Repondu`] pour ce qu'une commande a rapporté et
//!    [`Event::Failed`] pour chaque commande refusée ;
//! 5. à la perte de la connexion, [`Event::Lost`] puis nouvelle tentative
//!    après un délai croissant de [`RETRY_MIN`] à [`RETRY_MAX`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use conduit_protocol::client::{Client, ClientError};
use conduit_protocol::{
    Command, EngineStatus, LinkDescriptor, NodeDescriptor, Notification, Reply,
};
use tokio::sync::mpsc;

use crate::model::Snapshot;

/// Délai avant la première nouvelle tentative de connexion.
pub const RETRY_MIN: Duration = Duration::from_secs(1);

/// Délai maximal entre deux tentatives de connexion.
pub const RETRY_MAX: Duration = Duration::from_secs(5);

/// Profondeur de la file de commandes en attente d'envoi.
const COMMAND_QUEUE: usize = 64;

/// Nom annoncé au démon lors de la négociation.
pub fn client_name() -> String {
    format!("conduit-gui {}", env!("CARGO_PKG_VERSION"))
}

/// Socket (ou tube nommé) par défaut, identique à celui de `conduitctl`.
pub fn default_socket() -> PathBuf {
    directories::ProjectDirs::from("", "", "conduit")
        .map(|d| {
            d.runtime_dir()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| d.data_dir().to_path_buf())
        })
        .map(|dir| {
            if cfg!(windows) {
                let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
                PathBuf::from(format!(r"\\.\pipe\conduit-{user}"))
            } else {
                dir.join("conduitd.sock")
            }
        })
        .unwrap_or_else(|| PathBuf::from("conduitd.sock"))
}

/// Délai de la tentative suivante : doublement borné par [`RETRY_MAX`].
pub fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(RETRY_MAX)
}

/// Le destinataire des événements a disparu : la session doit s'arrêter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Poignée d'envoi de commandes au démon, clonable et non bloquante.
///
/// Les commandes sont mises en file ; la réponse n'est pas rendue à
/// l'appelant : l'état de l'interface se met à jour par les notifications
/// (aucun optimisme local). Une commande refusée produit [`Event::Failed`].
#[derive(Debug, Clone)]
pub struct Requester(mpsc::Sender<Command>);

impl Requester {
    /// Crée une poignée et le récepteur des commandes qu'elle met en file.
    ///
    /// [`run`] s'en sert pour sa session ; les tests s'en servent pour
    /// observer les commandes émises par l'interface sans démon.
    pub fn channel(capacity: usize) -> (Requester, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Requester(tx), rx)
    }

    /// Met une commande en file. Ignore silencieusement si la session est
    /// fermée ou la file pleine : la reconnexion rechargera l'état complet.
    pub fn send(&self, command: Command) {
        if self.0.try_send(command).is_err() {
            tracing::warn!("commande abandonnée : session IPC indisponible");
        }
    }
}

/// Événement produit par la couche IPC à destination de l'interface.
#[derive(Debug, Clone)]
pub enum Event {
    /// Première émission : poignée d'envoi des commandes.
    Started(Requester),
    /// Une tentative de connexion commence.
    Connecting,
    /// Connexion établie, abonnement actif et état initial chargé.
    Ready {
        /// Identification du démon (`HelloReply::server`).
        server: String,
        /// État complet du démon au moment de la connexion.
        snapshot: Box<Snapshot>,
    },
    /// Notification diffusée par le démon.
    Notified(Box<Notification>),
    /// Ce qu'une commande de l'interface a rapporté, au-delà d'un accusé de
    /// réception.
    ///
    /// Deux cas s'y rangent.
    ///
    /// **Les relectures d'état.** Le protocole ne diffuse aucune notification
    /// pour un changement de gain ou de coupure (`Command::SetNodeGain` et
    /// `SetLinkGain` répondent `Reply::Ok` et rien d'autre), ni pour
    /// `SetDriver` côté choix configuré, ni pour `ResetXruns`. Plutôt que de
    /// supposer localement le résultat, l'interface renvoie un `Nodes`, un
    /// `Links` ou un `Status` derrière la commande et remplace la partie
    /// correspondante du miroir par ce que le démon vient de dire.
    ///
    /// **Le rapport de diagnostic.** `Command::Dump` répond un texte que le
    /// démon a rédigé : il n'a rien à voir avec le miroir, il part vers un
    /// fichier.
    Repondu(Box<Reponse>),
    /// Une commande a été refusée : message destiné à l'utilisateur.
    Failed(String),
    /// Connexion perdue ou impossible.
    Lost {
        /// Cause, telle qu'affichée à l'utilisateur.
        reason: String,
        /// Délai avant la tentative suivante.
        retry_in: Duration,
    },
}

/// Ce qu'une réponse du démon a rapporté à l'interface.
#[derive(Debug, Clone, PartialEq)]
pub enum Reponse {
    /// Les nœuds du graphe, tels que le démon les décrit maintenant.
    Nodes(Vec<NodeDescriptor>),
    /// Les liens du graphe, tels que le démon les décrit maintenant.
    Links(Vec<LinkDescriptor>),
    /// L'état global, tel que le démon le décrit maintenant.
    ///
    /// Encadré : [`EngineStatus`] pèse près de dix fois les autres variantes.
    Statut(Box<EngineStatus>),
    /// Le rapport de diagnostic, déjà rédigé par le démon
    /// (`conduitd::service::Service::dump`).
    Rapport(String),
}

/// Puits d'événements : abstrait le canal de la `Subscription` d'`iced`, ce qui
/// permet aux tests d'utiliser un simple canal `tokio`.
pub trait EventSink {
    /// Transmet un événement ; `Err(Closed)` si le destinataire a disparu.
    fn send(&mut self, event: Event) -> impl Future<Output = Result<(), Closed>> + Send;
}

impl EventSink for mpsc::Sender<Event> {
    async fn send(&mut self, event: Event) -> Result<(), Closed> {
        mpsc::Sender::send(self, event).await.map_err(|_| Closed)
    }
}

impl EventSink for mpsc::UnboundedSender<Event> {
    async fn send(&mut self, event: Event) -> Result<(), Closed> {
        mpsc::UnboundedSender::send(self, event).map_err(|_| Closed)
    }
}

/// Boucle complète : connexion, sessions successives et reconnexion.
///
/// Rend la main quand le puits (fenêtre fermée) ou la file de commandes est
/// fermé ; sinon elle tourne indéfiniment.
pub async fn run<S: EventSink>(socket: PathBuf, mut sink: S) {
    let (requester, mut rx) = Requester::channel(COMMAND_QUEUE);
    if sink.send(Event::Started(requester)).await.is_err() {
        return;
    }
    let mut delay = RETRY_MIN;
    loop {
        if sink.send(Event::Connecting).await.is_err() {
            return;
        }
        let mut connected = false;
        match session(&socket, &mut rx, &mut sink, &mut connected).await {
            Ok(()) => return,
            Err(error) => {
                if connected {
                    delay = RETRY_MIN;
                }
                let lost = Event::Lost {
                    reason: error.to_string(),
                    retry_in: delay,
                };
                tracing::debug!("session IPC terminée : {error}");
                if sink.send(lost).await.is_err() {
                    return;
                }
                tokio::time::sleep(delay).await;
                delay = next_delay(delay);
            }
        }
    }
}

/// Charge l'état initial du démon : `Status`, `Nodes`, `Links`, `CableList`.
///
/// `CableList` est toléré en erreur : un backend sans câbles virtuels le
/// refuse, ce qui n'empêche pas d'afficher le reste.
pub async fn load(client: &mut Client) -> Result<Snapshot, ClientError> {
    let status = match client.call(Command::Status).await? {
        Reply::Status(status) => status,
        _ => return Err(ClientError::Unexpected),
    };
    let nodes = match client.call(Command::Nodes).await? {
        Reply::Nodes { nodes } => nodes,
        _ => return Err(ClientError::Unexpected),
    };
    let links = match client.call(Command::Links).await? {
        Reply::Links { links } => links,
        _ => return Err(ClientError::Unexpected),
    };
    let cables = match client.request(Command::CableList).await? {
        Ok(Reply::Cables { cables }) => cables,
        Ok(_) => return Err(ClientError::Unexpected),
        Err(error) => {
            tracing::info!("câbles indisponibles sur ce démon : {}", error.message);
            Vec::new()
        }
    };
    Ok(Snapshot {
        status,
        nodes,
        links,
        cables,
    })
}

/// Traduit une réponse du démon en [`Reponse`], s'il y a quelque chose à en
/// tirer.
///
/// Les autres réponses sont de simples accusés de réception : l'état vient des
/// notifications.
fn reponse(reply: Reply) -> Option<Reponse> {
    match reply {
        Reply::Nodes { nodes } => Some(Reponse::Nodes(nodes)),
        Reply::Links { links } => Some(Reponse::Links(links)),
        Reply::Status(status) => Some(Reponse::Statut(Box::new(status))),
        Reply::Dump { text } => Some(Reponse::Rapport(text)),
        _ => None,
    }
}

/// Ce qui a réveillé la boucle d'une session.
enum Step {
    /// Une commande à envoyer, ou `None` si la file est fermée.
    Command(Option<Command>),
    /// Une notification, ou l'erreur qui a rompu la connexion.
    Event(Result<Notification, ClientError>),
}

/// Une session : connexion, abonnement, chargement initial puis boucle.
///
/// `connected` passe à `true` dès que le chargement initial a réussi, pour que
/// l'appelant réarme le délai de reconnexion.
async fn session<S: EventSink>(
    socket: &Path,
    commands: &mut mpsc::Receiver<Command>,
    sink: &mut S,
    connected: &mut bool,
) -> Result<(), ClientError> {
    let mut client = Client::connect(socket, &client_name()).await?;
    let server = client.server().server.clone();
    // Abonnement avant le chargement : les notifications reçues pendant le
    // chargement sont mises de côté par le client et rejouées ensuite, donc
    // aucun changement n'est perdu entre les deux.
    client.subscribe().await?;
    let snapshot = Box::new(load(&mut client).await?);
    *connected = true;
    if sink.send(Event::Ready { server, snapshot }).await.is_err() {
        return Ok(());
    }
    loop {
        // Le futur d'attente d'une notification emprunte `client` ; il est
        // abandonné à la sortie du bloc pour libérer l'emprunt avant l'envoi
        // d'une éventuelle commande. `next_event` et `recv` sont sûrs à
        // l'annulation : rien n'est perdu.
        let step = {
            let next = client.next_event();
            tokio::pin!(next);
            tokio::select! {
                command = commands.recv() => Step::Command(command),
                event = &mut next => Step::Event(event),
            }
        };
        match step {
            Step::Command(None) => return Ok(()),
            Step::Command(Some(command)) => match client.request(command).await? {
                Err(error) => {
                    if sink.send(Event::Failed(error.message)).await.is_err() {
                        return Ok(());
                    }
                }
                // Seules les réponses demandées par l'interface rapportent
                // quelque chose : les autres sont des accusés de réception,
                // l'état venant des notifications.
                Ok(reply) => {
                    if let Some(reponse) = reponse(reply) {
                        if sink.send(Event::Repondu(Box::new(reponse))).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            },
            Step::Event(event) => {
                if sink.send(Event::Notified(Box::new(event?))).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les listes, l'état et le rapport rapportent quelque chose ; le reste
    /// n'est qu'un accusé de réception.
    #[test]
    fn seules_certaines_reponses_rapportent_quelque_chose() {
        assert_eq!(
            reponse(Reply::Nodes { nodes: vec![] }),
            Some(Reponse::Nodes(vec![]))
        );
        assert_eq!(
            reponse(Reply::Links { links: vec![] }),
            Some(Reponse::Links(vec![]))
        );
        assert_eq!(
            reponse(Reply::Dump {
                text: "conduitd 0.1.0".into()
            }),
            Some(Reponse::Rapport("conduitd 0.1.0".into()))
        );
        assert_eq!(reponse(Reply::Ok), None);
    }

    #[test]
    fn backoff_grows_from_one_to_five_seconds() {
        let mut d = RETRY_MIN;
        let mut seen = vec![d];
        for _ in 0..5 {
            d = next_delay(d);
            seen.push(d);
        }
        assert_eq!(
            seen,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                RETRY_MAX,
                RETRY_MAX,
                RETRY_MAX,
            ]
        );
    }

    #[test]
    fn default_socket_is_named_after_the_daemon() {
        let s = default_socket();
        let text = s.display().to_string();
        assert!(text.contains("conduit"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn missing_daemon_is_reported_then_retried() {
        let (tx, mut rx) = mpsc::channel(8);
        let socket = std::env::temp_dir().join("conduit-gui-inexistant.sock");
        let _ = std::fs::remove_file(&socket);
        let task = tokio::spawn(run(socket, tx));
        let mut requester = None;
        let mut connecting = 0;
        let mut lost = 0;
        while let Some(event) = rx.recv().await {
            match event {
                Event::Started(r) => requester = Some(r),
                Event::Connecting => connecting += 1,
                Event::Lost { retry_in, .. } => {
                    lost += 1;
                    assert!(retry_in >= RETRY_MIN && retry_in <= RETRY_MAX);
                    if lost == 2 {
                        break;
                    }
                }
                other => panic!("événement inattendu : {other:?}"),
            }
        }
        assert!(requester.is_some(), "la poignée d'envoi doit être fournie");
        assert_eq!(connecting, 2, "une tentative par cycle");
        task.abort();
    }
}
