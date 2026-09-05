//! Client asynchrone (feature `client`) : connexion, `Hello`, requêtes, événements.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::api::{Command, Notification, ProtocolError, Reply};
use crate::framing::{encode_message, Decoder, FramingError};
use crate::wire::{negotiate_client, Hello, HelloReply, Message, Request, PROTOCOL_VERSION};

/// Erreurs du client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Connexion impossible.
    #[error("impossible de joindre le démon sur {path} : {source}. Est-il démarré ? (conduitd)")]
    Connect {
        /// Chemin.
        path: String,
        /// Cause.
        source: std::io::Error,
    },
    /// Entrée/sortie.
    #[error("connexion au démon perdue : {0}")]
    Io(#[from] std::io::Error),
    /// Trame invalide.
    #[error("{0}")]
    Framing(#[from] FramingError),
    /// Version refusée par le démon.
    #[error("{0}")]
    Refused(String),
    /// Réponse inattendue.
    #[error("réponse inattendue du démon")]
    Unexpected,
    /// Le démon a fermé.
    #[error("le démon a fermé la connexion")]
    Closed,
}

type Stream = Box<dyn StreamLike>;

/// Flux asynchrone générique.
pub trait StreamLike: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> StreamLike for T {}

/// Client connecté.
pub struct Client {
    stream: Stream,
    decoder: Decoder,
    next_id: u32,
    pending_events: std::collections::VecDeque<Notification>,
    server: HelloReply,
}

impl core::fmt::Debug for Client {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Client")
            .field("server", &self.server.server)
            .finish()
    }
}

impl Client {
    /// Se connecte et négocie la version.
    pub async fn connect(path: &Path, client_name: &str) -> Result<Self, ClientError> {
        let stream: Stream = connect_stream(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.display().to_string(),
                source,
            })?;
        Self::handshake(stream, client_name).await
    }

    /// Se connecte sur un flux déjà ouvert (tests).
    pub async fn handshake(stream: Stream, client_name: &str) -> Result<Self, ClientError> {
        let mut c = Self {
            stream,
            decoder: Decoder::new(),
            next_id: 1,
            pending_events: Default::default(),
            server: HelloReply {
                version: 0,
                server: String::new(),
                accepted: false,
                reason: None,
            },
        };
        let hello = Hello {
            version: PROTOCOL_VERSION,
            client: client_name.to_string(),
        };
        c.stream
            .write_all(&encode_message(&Message::Hello(hello))?)
            .await?;
        match c.read_message().await? {
            Message::HelloReply(r) => {
                if let Err(reason) = negotiate_client(&r) {
                    return Err(ClientError::Refused(reason));
                }
                c.server = r;
                Ok(c)
            }
            _ => Err(ClientError::Unexpected),
        }
    }

    /// Informations du démon.
    pub fn server(&self) -> &HelloReply {
        &self.server
    }

    async fn read_message(&mut self) -> Result<Message, ClientError> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(m) = self.decoder.next_message::<Message>()? {
                return Ok(m);
            }
            let n = self.stream.read(&mut buf).await?;
            if n == 0 {
                return Err(ClientError::Closed);
            }
            self.decoder.feed(&buf[..n]);
        }
    }

    /// Envoie une commande et attend sa réponse. Les événements reçus entre-temps
    /// sont conservés pour [`next_event`](Self::next_event).
    pub async fn request(
        &mut self,
        command: Command,
    ) -> Result<Result<Reply, ProtocolError>, ClientError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.stream
            .write_all(&encode_message(&Message::Request(Request { id, command }))?)
            .await?;
        loop {
            match self.read_message().await? {
                Message::Response(r) if r.id == id => return Ok(r.result),
                Message::Event(e) => self.pending_events.push_back(e.notification),
                _ => {}
            }
        }
    }

    /// Envoie une commande et convertit l'erreur du démon en erreur de client.
    pub async fn call(&mut self, command: Command) -> Result<Reply, ClientError> {
        match self.request(command).await? {
            Ok(r) => Ok(r),
            Err(e) => Err(ClientError::Refused(e.message)),
        }
    }

    /// S'abonne aux notifications.
    pub async fn subscribe(&mut self) -> Result<(), ClientError> {
        self.request(Command::Subscribe { enabled: true })
            .await?
            .map_err(|e| ClientError::Refused(e.message))?;
        Ok(())
    }

    /// Attend la prochaine notification.
    pub async fn next_event(&mut self) -> Result<Notification, ClientError> {
        if let Some(e) = self.pending_events.pop_front() {
            return Ok(e);
        }
        loop {
            if let Message::Event(e) = self.read_message().await? {
                return Ok(e.notification);
            }
        }
    }

    /// Attend la prochaine notification, avec délai.
    pub async fn next_event_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<Notification>, ClientError> {
        match tokio::time::timeout(timeout, self.next_event()).await {
            Ok(r) => r.map(Some),
            Err(_) => Ok(None),
        }
    }
}

async fn connect_stream(path: &Path) -> std::io::Result<Stream> {
    #[cfg(unix)]
    {
        let s = tokio::net::UnixStream::connect(path).await?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        for _ in 0..50 {
            match ClientOptions::new().open(path) {
                Ok(s) => return Ok(Box::new(s)),
                Err(e) if e.raw_os_error() == Some(231) => {
                    // ERROR_PIPE_BUSY : toutes les instances sont occupées, réessayer.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "named pipe occupé",
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "plateforme sans IPC",
        ))
    }
}
