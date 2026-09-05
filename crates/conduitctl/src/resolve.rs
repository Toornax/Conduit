//! Résolution des nœuds, ports et périphériques désignés par l'utilisateur.

use conduit_backend::DeviceId;
use conduit_core::graph::{Direction, NodeId, PortIndex};
use conduit_protocol::client::Client;
use conduit_protocol::{Command, NodeDescriptor, Reply};

use crate::cli::VolumeTarget;
use crate::{parse_link_id, parse_node_id, CliError};

async fn nodes(client: &mut Client) -> Result<Vec<NodeDescriptor>, CliError> {
    match client.request(Command::Nodes).await? {
        Ok(Reply::Nodes { nodes }) => Ok(nodes),
        Ok(_) => Err(CliError::Daemon("réponse inattendue".into())),
        Err(e) => Err(CliError::Daemon(e.message)),
    }
}

/// Trouve un nœud par identifiant (`n3.0`), clé, nom interne, nom d'affichage ou
/// identifiant de périphérique (insensible à la casse ; préfixe accepté s'il est unique).
pub async fn node(client: &mut Client, spec: &str) -> Result<NodeId, CliError> {
    let all = nodes(client).await?;
    find_node(&all, spec).map(|n| n.id)
}

/// Version pure de [`node`].
pub fn find_node<'a>(
    all: &'a [NodeDescriptor],
    spec: &str,
) -> Result<&'a NodeDescriptor, CliError> {
    if let Some(id) = parse_node_id(spec) {
        return all.iter().find(|n| n.id == id).ok_or_else(|| {
            CliError::Usage(format!("nœud {spec} inconnu (voir `conduitctl nodes`)"))
        });
    }
    let s = spec.to_lowercase();
    let exact: Vec<&NodeDescriptor> = all
        .iter()
        .filter(|n| {
            n.label.to_lowercase() == s
                || n.key.to_string().to_lowercase() == s
                || matches!(&n.key, conduit_protocol::api::NodeKey::Internal { name } if name.to_lowercase() == s)
                || n.device.as_ref().is_some_and(|d| d.id.as_str().to_lowercase() == s || d.name.to_lowercase() == s)
        })
        .collect();
    match exact.len() {
        1 => return Ok(exact[0]),
        n if n > 1 => {
            return Err(CliError::Usage(format!(
                "« {spec} » désigne {n} nœuds : précisez l'identifiant ({})",
                exact
                    .iter()
                    .map(|n| format!("{} = {}", n.id, n.label))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
        _ => {}
    }
    let prefix: Vec<&NodeDescriptor> = all
        .iter()
        .filter(|n| {
            n.label.to_lowercase().starts_with(&s) || n.key.to_string().to_lowercase().contains(&s)
        })
        .collect();
    match prefix.len() {
        1 => Ok(prefix[0]),
        0 => Err(CliError::Usage(format!(
            "aucun nœud ne correspond à « {spec} » (voir `conduitctl nodes`)"
        ))),
        n => Err(CliError::Usage(format!(
            "« {spec} » est ambigu ({n} nœuds) : {}",
            prefix
                .iter()
                .map(|n| format!("{} = {}", n.id, n.label))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Résout `<nœud>:<port>` (port par nom ou index) dans une direction.
pub async fn port(
    client: &mut Client,
    spec: &str,
    direction: Direction,
) -> Result<(NodeId, PortIndex), CliError> {
    let all = nodes(client).await?;
    find_port(&all, spec, direction)
}

/// Version pure de [`port`].
pub fn find_port(
    all: &[NodeDescriptor],
    spec: &str,
    direction: Direction,
) -> Result<(NodeId, PortIndex), CliError> {
    let (node_spec, port_spec) = spec.rsplit_once(':').ok_or_else(|| {
        CliError::Usage(format!(
            "port invalide « {spec} » : attendu <nœud>:<port> (ex. gen:FL)"
        ))
    })?;
    let n = find_node(all, node_spec)?;
    let ports = match direction {
        Direction::Input => &n.inputs,
        Direction::Output => &n.outputs,
    };
    if let Ok(i) = port_spec.parse::<usize>() {
        if i < ports.len() {
            return Ok((n.id, i as PortIndex));
        }
    }
    ports
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(port_spec))
        .map(|i| (n.id, i as PortIndex))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "port « {port_spec} » inconnu sur {} ({direction}s : {})",
                n.label,
                ports
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// `l3.0` → lien ; sinon nœud.
pub async fn volume_target(client: &mut Client, spec: &str) -> Result<VolumeTarget, CliError> {
    if spec.starts_with('l') && spec[1..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return parse_link_id(spec).map(VolumeTarget::Link);
    }
    node(client, spec).await.map(VolumeTarget::Node)
}

/// Résout un périphérique par nœud.
pub async fn device_id(client: &mut Client, spec: &str) -> Result<DeviceId, CliError> {
    let all = nodes(client).await?;
    let n = find_node(&all, spec)?;
    n.device
        .as_ref()
        .map(|d| d.id.clone())
        .ok_or_else(|| CliError::Usage(format!("{} n'est pas un périphérique", n.label)))
}
