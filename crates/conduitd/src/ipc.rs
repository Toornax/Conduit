//! Serveur IPC multi-clients (M0-82, M0-84) : socket Unix (0600) ou named pipe.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use conduit_protocol::framing::{encode_message, Decoder};
use conduit_protocol::wire::{negotiate, Message, Request, Response};
use conduit_protocol::{Command, Notification};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::service::Service;

/// Nombre maximal de clients simultanés.
pub const MAX_CLIENTS: usize = 32;

/// Délai accordé au `Hello`.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Flux client générique.
pub type ClientStream = Box<dyn AsyncStream>;

/// Lecture + écriture asynchrones.
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// Écouteur de la plateforme.
pub struct Listener {
    inner: Inner,
    path: PathBuf,
}

enum Inner {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Pipe(Option<tokio::net::windows::named_pipe::NamedPipeServer>),
}

impl core::fmt::Debug for Listener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Listener")
            .field("path", &self.path)
            .finish()
    }
}

impl Listener {
    /// Ouvre l'écouteur. Sur Unix, un fichier de socket abandonné est supprimé et le
    /// fichier créé est en 0600.
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            if path.exists() {
                match std::os::unix::net::UnixStream::connect(path) {
                    Ok(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AddrInUse,
                            format!(
                                "un démon écoute déjà sur {} : arrêtez-le ou utilisez --socket",
                                path.display()
                            ),
                        ))
                    }
                    Err(_) => std::fs::remove_file(path)?,
                }
            }
            let listener = tokio::net::UnixListener::bind(path)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self {
                inner: Inner::Unix(listener),
                path: path.to_path_buf(),
            })
        }
        #[cfg(windows)]
        {
            // Le chemin reçu (`--socket`, `--root`, tests) est projeté sur un nom de pipe
            // par la même fonction que le client : voir `conduit_protocol::client::pipe_name`.
            let name = conduit_protocol::client::pipe_name(path);
            let server = tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .map_err(|e| {
                    // ERROR_ACCESS_DENIED (5) et ERROR_PIPE_BUSY (231) sur
                    // `first_pipe_instance` veulent dire « ce nom est déjà tenu » : c'est
                    // le même cas que `AddrInUse` sous Unix, et `main` en fait le code de
                    // retour 3 (crate::single_instance). Cette voie est la course des
                    // deux démons lancés en même temps ; le cas normal est détecté avant.
                    let kind = match e.raw_os_error() {
                        Some(5) | Some(231) => std::io::ErrorKind::AddrInUse,
                        _ => e.kind(),
                    };
                    std::io::Error::new(
                        kind,
                        format!(
                            "named pipe {} : {e} (un démon écoute déjà ? arrêtez-le ou utilisez --socket)",
                            name.display()
                        ),
                    )
                })?;
            Ok(Self {
                inner: Inner::Pipe(Some(server)),
                path: name,
            })
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

    /// Chemin ou nom.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Attend un client.
    pub async fn accept(&mut self) -> std::io::Result<ClientStream> {
        match &mut self.inner {
            #[cfg(unix)]
            Inner::Unix(l) => {
                let (stream, _) = l.accept().await?;
                Ok(Box::new(stream))
            }
            #[cfg(windows)]
            Inner::Pipe(slot) => {
                // L'instance en attente reste dans `slot` pendant `connect()` : `accept` est
                // appelé dans un `select!` et peut être annulé ; la retirer avant l'attente
                // laissait un `None` et faisait paniquer l'appel suivant.
                slot.as_ref().expect("instance de pipe").connect().await?;
                let server = slot.take().expect("instance de pipe");
                // Une instance par client : la suivante est créée avant de servir celle-ci,
                // sinon un client qui se présente entre-temps reçoit ERROR_PIPE_BUSY.
                *slot =
                    Some(tokio::net::windows::named_pipe::ServerOptions::new().create(&self.path)?);
                Ok(Box::new(server))
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Boucle d'acceptation : sert les clients jusqu'au signal d'arrêt.
pub async fn serve(
    mut listener: Listener,
    service: Arc<Service>,
    mut shutdown: watch::Receiver<bool>,
    server_name: String,
) {
    let clients = Arc::new(tokio::sync::Semaphore::new(MAX_CLIENTS));
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let Ok(permit) = Arc::clone(&clients).try_acquire_owned() else {
                            tracing::warn!("client refusé : {MAX_CLIENTS} clients déjà connectés");
                            continue;
                        };
                        let service = Arc::clone(&service);
                        let name = server_name.clone();
                        let shutdown = shutdown.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) = handle_client(stream, service, name, shutdown).await {
                                tracing::debug!("client terminé : {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("accept : {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
    tracing::info!("serveur IPC arrêté");
}

/// Erreurs d'une session client.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Entrée/sortie.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Trame invalide : le client est déconnecté.
    #[error("trame invalide : {0}")]
    Framing(#[from] conduit_protocol::FramingError),
    /// Le premier message n'est pas un `Hello`.
    #[error("le client n'a pas envoyé Hello")]
    NoHello,
    /// Version refusée.
    #[error("version refusée : {0}")]
    Refused(String),
    /// Le client a fermé.
    #[error("connexion fermée")]
    Closed,
}

async fn read_message(
    stream: &mut ClientStream,
    decoder: &mut Decoder,
) -> Result<Message, SessionError> {
    let mut buf = [0u8; 4096];
    loop {
        if let Some(m) = decoder.next_message::<Message>()? {
            return Ok(m);
        }
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(SessionError::Closed);
        }
        decoder.feed(&buf[..n]);
    }
}

async fn handle_client(
    mut stream: ClientStream,
    service: Arc<Service>,
    server_name: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), SessionError> {
    let mut decoder = Decoder::new();
    // 1. Hello.
    let hello =
        match tokio::time::timeout(HELLO_TIMEOUT, read_message(&mut stream, &mut decoder)).await {
            Ok(Ok(Message::Hello(h))) => h,
            Ok(Ok(_)) => return Err(SessionError::NoHello),
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(SessionError::NoHello),
        };
    let reply = negotiate(&hello, &server_name);
    stream
        .write_all(&encode_message(&Message::HelloReply(reply.clone()))?)
        .await?;
    if !reply.accepted {
        return Err(SessionError::Refused(reply.reason.unwrap_or_default()));
    }
    tracing::info!(client = %hello.client, "client connecté");
    // 2. Boucle : requêtes du client, événements du service.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(256);
    let mut events: Option<tokio::sync::broadcast::Receiver<Notification>> = None;
    let mut buf = [0u8; 4096];
    loop {
        // Vide les messages décodés avant de lire.
        while let Some(msg) = decoder.next_message::<Message>()? {
            match msg {
                Message::Request(Request { id, command }) => {
                    if let Command::Subscribe { enabled } = &command {
                        events = if *enabled {
                            Some(service.subscribe())
                        } else {
                            None
                        };
                    }
                    let result = service.execute(command).await;
                    let frame = encode_message(&Message::Response(Response { id, result }))?;
                    let _ = out_tx.send(frame).await;
                }
                Message::Hello(_) => {
                    let frame =
                        encode_message(&Message::HelloReply(negotiate(&hello, &server_name)))?;
                    let _ = out_tx.send(frame).await;
                }
                _ => {}
            }
        }
        let event_fut = async {
            match &mut events {
                Some(rx) => rx.recv().await.ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            read = stream.read(&mut buf) => {
                let n = read?;
                if n == 0 { return Err(SessionError::Closed); }
                decoder.feed(&buf[..n]);
            }
            Some(frame) = out_rx.recv() => {
                stream.write_all(&frame).await?;
            }
            ev = event_fut => {
                if let Some(n) = ev {
                    let frame = encode_message(&Message::event(n))?;
                    stream.write_all(&frame).await?;
                } else {
                    events = None;
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = stream.write_all(&encode_message(&Message::event(Notification::Shutdown))?).await;
                    return Ok(());
                }
            }
        }
    }
}
