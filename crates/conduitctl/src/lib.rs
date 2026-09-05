//! `conduitctl` — client en ligne de commande du démon Conduit (F-41).
//!
//! Chaque commande produit une sortie tabulaire lisible ou, avec `--json`, la
//! réponse brute du protocole.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod format;
pub mod resolve;

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use conduit_backend::{CableId, CableSpec};
use conduit_core::graph::{Direction, LinkId, NodeId, PortId};
use conduit_core::types::{ChannelCount, Db};
use conduit_protocol::client::{Client, ClientError};
use conduit_protocol::{Command, DriverChoice, Notification, Reply};

use cli::{Args, CableCmd, Cmd, VolumeTarget};

/// Erreurs de la CLI, affichées telles quelles à l'utilisateur.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Client / transport.
    #[error("{0}")]
    Client(#[from] ClientError),
    /// Erreur renvoyée par le démon.
    #[error("{0}")]
    Daemon(String),
    /// Argument invalide.
    #[error("{0}")]
    Usage(String),
}

/// Point d'entrée : analyse `args` (sans le nom du programme), exécute, retourne la
/// sortie à afficher.
pub async fn run(args: Vec<String>) -> Result<String, CliError> {
    let parsed = Args::try_parse_from(std::iter::once("conduitctl".to_string()).chain(args))
        .map_err(|e| CliError::Usage(e.to_string()))?;
    execute(parsed).await
}

/// Socket par défaut.
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

async fn execute(args: Args) -> Result<String, CliError> {
    let socket = args.socket.clone().unwrap_or_else(default_socket);
    let mut client = Client::connect(
        &socket,
        &format!("conduitctl {}", env!("CARGO_PKG_VERSION")),
    )
    .await?;
    let json = args.json;
    let reply = match args.cmd {
        Cmd::Status => call(&mut client, Command::Status).await?,
        Cmd::Nodes => call(&mut client, Command::Nodes).await?,
        Cmd::Ports { node } => {
            let id = resolve::node(&mut client, &node).await?;
            call(&mut client, Command::Ports { node: id }).await?
        }
        Cmd::Links => call(&mut client, Command::Links).await?,
        Cmd::Link { src, dst } => {
            let (sn, sp) = resolve::port(&mut client, &src, Direction::Output).await?;
            let (dn, dp) = resolve::port(&mut client, &dst, Direction::Input).await?;
            call(
                &mut client,
                Command::Link {
                    src: PortId::new(sn, Direction::Output, sp),
                    dst: PortId::new(dn, Direction::Input, dp),
                },
            )
            .await?
        }
        Cmd::Unlink { link } => {
            let id = parse_link_id(&link)?;
            call(&mut client, Command::Unlink { link: id }).await?
        }
        Cmd::Volume { target, value } => {
            let (gain_db, muted) = parse_volume(&value)?;
            let cmd = match resolve::volume_target(&mut client, &target).await? {
                VolumeTarget::Node(node) => Command::SetNodeGain {
                    node,
                    gain_db,
                    muted,
                },
                VolumeTarget::Link(link) => Command::SetLinkGain {
                    link,
                    gain_db,
                    muted,
                },
            };
            call(&mut client, cmd).await?
        }
        Cmd::Driver { choice } => match choice {
            None => call(&mut client, Command::Status).await?,
            Some(c) => {
                let choice = match c.as_str() {
                    "auto" => DriverChoice::Auto,
                    "internal" => DriverChoice::Internal,
                    other => DriverChoice::Device {
                        id: resolve::device_id(&mut client, other).await?,
                    },
                };
                call(&mut client, Command::SetDriver { choice }).await?
            }
        },
        Cmd::Cable { cmd } => {
            let cmd = match cmd {
                CableCmd::List => Command::CableList,
                CableCmd::Add { name, channels } => Command::CableAdd {
                    spec: CableSpec {
                        name,
                        channels: ChannelCount::new(channels).ok_or_else(|| {
                            CliError::Usage(format!("canaux : entre 1 et {}", ChannelCount::MAX))
                        })?,
                    },
                },
                CableCmd::Remove { id } => Command::CableRemove { id: CableId(id) },
                CableCmd::Rename { id, name } => Command::CableRename {
                    id: CableId(id),
                    name,
                },
                CableCmd::Channels { id, channels } => Command::CableSetChannels {
                    id: CableId(id),
                    channels: ChannelCount::new(channels).ok_or_else(|| {
                        CliError::Usage(format!("canaux : entre 1 et {}", ChannelCount::MAX))
                    })?,
                },
            };
            call(&mut client, cmd).await?
        }
        Cmd::Monitor { count, timeout } => {
            return monitor(&mut client, count, timeout, json).await;
        }
        Cmd::Xruns { reset } => {
            if reset {
                call(&mut client, Command::ResetXruns).await?;
            }
            let Reply::Status(s) = call(&mut client, Command::Status).await? else {
                return Err(CliError::Daemon("réponse inattendue".into()));
            };
            return Ok(if json {
                serde_json::to_string_pretty(&s).unwrap()
            } else {
                format::xruns(&s)
            });
        }
        Cmd::Dump => call(&mut client, Command::Dump).await?,
        Cmd::Save => call(&mut client, Command::Save).await?,
        Cmd::Load => call(&mut client, Command::Load).await?,
        Cmd::Add { kind } => {
            let (name, kind) = kind.into_kind()?;
            call(&mut client, Command::AddInternal { name, kind }).await?
        }
        Cmd::Remove { node } => {
            let id = resolve::node(&mut client, &node).await?;
            call(&mut client, Command::RemoveNode { node: id }).await?
        }
        Cmd::Param { node, name, value } => {
            let id = resolve::node(&mut client, &node).await?;
            call(
                &mut client,
                Command::SetParam {
                    node: id,
                    name,
                    value,
                },
            )
            .await?
        }
        Cmd::Meter { node } => {
            let id = resolve::node(&mut client, &node).await?;
            call(&mut client, Command::ReadMeter { node: id }).await?
        }
        Cmd::Label { node, label } => {
            let id = resolve::node(&mut client, &node).await?;
            call(&mut client, Command::SetLabel { node: id, label }).await?
        }
    };
    Ok(if json {
        serde_json::to_string_pretty(&reply).unwrap()
    } else {
        format::reply(&reply)
    })
}

async fn call(client: &mut Client, cmd: Command) -> Result<Reply, CliError> {
    match client.request(cmd).await? {
        Ok(r) => Ok(r),
        Err(e) => Err(CliError::Daemon(e.message)),
    }
}

async fn monitor(
    client: &mut Client,
    count: Option<usize>,
    timeout: Option<f64>,
    json: bool,
) -> Result<String, CliError> {
    client.subscribe().await?;
    let mut out = String::new();
    let mut n = 0;
    let deadline = timeout.map(|t| std::time::Instant::now() + Duration::from_secs_f64(t));
    loop {
        if count.is_some_and(|c| n >= c) {
            break;
        }
        let wait = match deadline {
            Some(d) => d.saturating_duration_since(std::time::Instant::now()),
            None => Duration::from_secs(3600),
        };
        if wait.is_zero() {
            break;
        }
        match client.next_event_timeout(wait).await? {
            Some(ev) => {
                let line = if json {
                    serde_json::to_string(&ev).unwrap()
                } else {
                    format::event(&ev)
                };
                println!("{line}");
                out.push_str(&line);
                out.push('\n');
                n += 1;
                if matches!(ev, Notification::Shutdown) {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(out)
}

/// Analyse `-6`, `-6dB`, `+3`, `mute`, `unmute`, `-inf`.
pub fn parse_volume(value: &str) -> Result<(Option<Db>, Option<bool>), CliError> {
    let v = value.trim().to_lowercase();
    match v.as_str() {
        "mute" | "muet" => Ok((None, Some(true))),
        "unmute" | "on" => Ok((None, Some(false))),
        "-inf" | "off" => Ok((Some(Db::NEG_INF), None)),
        _ => {
            let num = v.trim_end_matches("db").trim();
            num.parse::<f32>()
                .map(|f| (Some(Db::new(f)), None))
                .map_err(|_| {
                    CliError::Usage(format!(
                        "volume invalide « {value} » : nombre en dB, -inf, mute ou unmute"
                    ))
                })
        }
    }
}

/// Analyse un identifiant de lien `l3.0` ou `3`.
pub fn parse_link_id(s: &str) -> Result<LinkId, CliError> {
    let t = s.trim().trim_start_matches('l');
    let (idx, gen) = t.split_once('.').unwrap_or((t, "0"));
    match (idx.parse::<u32>(), gen.parse::<u32>()) {
        (Ok(i), Ok(g)) => Ok(LinkId::new(i, g)),
        _ => Err(CliError::Usage(format!(
            "identifiant de lien invalide « {s} » : attendu l<index>.<génération>"
        ))),
    }
}

/// Analyse un identifiant de nœud `n3.0`.
pub fn parse_node_id(s: &str) -> Option<NodeId> {
    let t = s.strip_prefix('n')?;
    let (idx, gen) = t.split_once('.')?;
    Some(NodeId::new(idx.parse().ok()?, gen.parse().ok()?))
}
