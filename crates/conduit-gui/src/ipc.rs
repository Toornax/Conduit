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
//!    [`Event::Failed`] pour chaque commande refusée ;
//! 5. à la perte de la connexion, [`Event::Lost`] puis nouvelle tentative
//!    après un délai croissant de [`RETRY_MIN`] à [`RETRY_MAX`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use conduit_protocol::client::{Client, ClientError};
use conduit_protocol::Command;
use conduit_protocol::Notification;
use tokio::sync::mpsc;

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
    /// Connexion établie et abonnement actif.
    Ready {
        /// Identification du démon (`HelloReply::server`).
        server: String,
    },
    /// Notification diffusée par le démon.
    Notified(Box<Notification>),
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
    let (tx, mut rx) = mpsc::channel(COMMAND_QUEUE);
    if sink.send(Event::Started(Requester(tx))).await.is_err() {
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
    // chargement sont mises de côté par le client et appliquées ensuite.
    client.subscribe().await?;
    *connected = true;
    if sink.send(Event::Ready { server }).await.is_err() {
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
            Step::Command(Some(command)) => {
                if let Err(error) = client.request(command).await? {
                    if sink.send(Event::Failed(error.message)).await.is_err() {
                        return Ok(());
                    }
                }
            }
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
