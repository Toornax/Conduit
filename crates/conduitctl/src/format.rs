//! Mise en forme tabulaire.

use conduit_protocol::{EngineStatus, Notification, Reply};

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .take(cols)
            .map(|(i, c)| format!("{:<w$}", c, w = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = line(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>());
    out.push('\n');
    for r in rows {
        out.push_str(&line(r));
        out.push('\n');
    }
    out
}

/// Formate une réponse.
pub fn reply(r: &Reply) -> String {
    match r {
        Reply::Ok => "ok\n".into(),
        Reply::Status(s) => status(s),
        Reply::Nodes { nodes } => table(
            &[
                "ID", "ÉTAT", "TYPE", "NOM", "ENTRÉES", "SORTIES", "GAIN", "CLÉ",
            ],
            &nodes
                .iter()
                .map(|n| {
                    vec![
                        n.id.to_string(),
                        format!("{:?}", n.state).to_lowercase(),
                        n.type_name.clone(),
                        n.label.clone(),
                        n.inputs.len().to_string(),
                        n.outputs.len().to_string(),
                        format!("{}{}", n.gain_db, if n.muted { " (muet)" } else { "" }),
                        n.key.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        Reply::Node(n) => {
            let mut out = format!(
                "{} {} « {} » ({:?}) gain {}{}\n",
                n.id,
                n.type_name,
                n.label,
                n.state,
                n.gain_db,
                if n.muted { " muet" } else { "" }
            );
            if let Some(d) = &n.device {
                out.push_str(&format!(
                    "périphérique : {} ({}, {} canaux, {})\n",
                    d.id, d.direction, d.channels, d.sample_rate
                ));
            }
            out.push_str("entrées :\n");
            for (i, p) in n.inputs.iter().enumerate() {
                out.push_str(&format!("  in{i}  {}\n", p.name));
            }
            out.push_str("sorties :\n");
            for (i, p) in n.outputs.iter().enumerate() {
                out.push_str(&format!("  out{i}  {}\n", p.name));
            }
            out
        }
        Reply::Links { links } => table(
            &["ID", "SOURCE", "DESTINATION", "GAIN"],
            &links
                .iter()
                .map(|l| {
                    vec![
                        l.link.id.to_string(),
                        l.link.src.to_string(),
                        l.link.dst.to_string(),
                        format!("{}{}", l.gain_db, if l.muted { " (muet)" } else { "" }),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        Reply::Link(l) => format!("lien {} : {} → {}\n", l.link.id, l.link.src, l.link.dst),
        Reply::Meter { channels } => table(
            &["CANAL", "CRÊTE", "RMS", "MAX"],
            &channels
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    vec![
                        i.to_string(),
                        format!("{:.1} dBFS", m.peak_dbfs()),
                        format!("{:.1} dBFS", m.rms_dbfs()),
                        format!("{:.3}", m.peak_hold),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        Reply::Cables { cables } => table(
            &["N°", "NOM", "CANAUX", "ACTIF", "RENDU", "CAPTURE"],
            &cables
                .iter()
                .map(|c| {
                    vec![
                        c.id.0.to_string(),
                        c.name.clone(),
                        c.channels.to_string(),
                        if c.active { "oui" } else { "non" }.into(),
                        c.render.to_string(),
                        c.capture.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        Reply::Cable(c) => format!(
            "câble {} « {} » {} : rendu {}, capture {}\n",
            c.id.0, c.name, c.channels, c.render, c.capture
        ),
        Reply::Dump { text } => text.clone(),
    }
}

/// Formate l'état.
pub fn status(s: &EngineStatus) -> String {
    let driver = match &s.driver {
        conduit_protocol::DriverStatus::None => "aucun".to_string(),
        conduit_protocol::DriverStatus::Internal => "horloge interne".to_string(),
        conduit_protocol::DriverStatus::Device { id } => id.to_string(),
    };
    let mut out = format!(
        "backend {}  {}  quantum {}  pilote {}  nœuds {}  liens {}\n",
        s.backend, s.sample_rate, s.quantum, driver, s.nodes, s.links
    );
    out.push_str(&xruns(s));
    if !s.devices.is_empty() {
        out.push_str(&table(
            &[
                "PÉRIPHÉRIQUE",
                "ÉTAT",
                "SOUS-ALIM",
                "DÉBORD",
                "REMPLISSAGE",
                "RATIO",
                "DLL",
            ],
            &s.devices
                .iter()
                .map(|d| {
                    vec![
                        d.id.to_string(),
                        format!("{:?}", d.state).to_lowercase(),
                        d.underruns.to_string(),
                        d.overruns.to_string(),
                        d.fill.to_string(),
                        format!("{:.6}", d.ratio),
                        if d.locked { "verrouillée" } else { "-" }.into(),
                    ]
                })
                .collect::<Vec<_>>(),
        ));
    }
    out
}

/// Formate les compteurs de xruns.
pub fn xruns(s: &EngineStatus) -> String {
    let t = &s.timing;
    format!(
        "xruns {}  cycles {}  temps de cycle min/moy/max {}/{}/{} µs  budget {} µs  dépassements {}\n",
        s.xruns,
        t.count,
        t.min_ns / 1000,
        t.avg_ns / 1000,
        t.max_ns / 1000,
        t.budget_ns / 1000,
        t.overruns
    )
}

/// Formate un événement sur une ligne.
pub fn event(n: &Notification) -> String {
    match n {
        Notification::NodeAdded(d) => {
            format!("nœud ajouté {} « {} » ({:?})", d.id, d.label, d.state)
        }
        Notification::NodeRemoved { id, key } => format!("nœud retiré {id} ({key})"),
        Notification::NodeStateChanged { id, state } => format!("nœud {id} : {state:?}"),
        Notification::LinkAdded(l) => format!(
            "lien ajouté {} : {} → {}",
            l.link.id, l.link.src, l.link.dst
        ),
        Notification::LinkRemoved { id } => format!("lien retiré {id}"),
        Notification::DriverChanged(d) => format!("pilote : {d:?}"),
        Notification::CableChanged { id, info } => match info {
            Some(i) => format!("câble {} : « {} » {}", id.0, i.name, i.channels),
            None => format!("câble {} supprimé", id.0),
        },
        Notification::Rt(e) => format!("temps réel : {e:?}"),
        Notification::Shutdown => "le démon s'arrête".into(),
    }
}
