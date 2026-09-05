//! Définition `clap` des commandes.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use conduit_core::graph::{LinkId, NodeId};
use conduit_core::nodes::EqBand;
use conduit_protocol::InternalKind;

use crate::CliError;

/// Contrôle et diagnostic du démon Conduit.
#[derive(Debug, Parser)]
#[command(name = "conduitctl", version, about, long_about = None)]
pub struct Args {
    /// Socket Unix ou named pipe du démon.
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// Sortie JSON (réponse brute du protocole).
    #[arg(long, global = true)]
    pub json: bool,
    /// Commande.
    #[command(subcommand)]
    pub cmd: Cmd,
}

/// Commandes.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// État du moteur : pilote, fréquence, xruns, périphériques.
    Status,
    /// Liste des nœuds (périphériques, câbles, nœuds internes).
    Nodes,
    /// Ports d'un nœud (identifiant `n3.0`, nom ou clé).
    Ports {
        /// Nœud.
        node: String,
    },
    /// Liste des liens.
    Links,
    /// Relie une sortie à une entrée : `<nœud>:<port>` (ex. `gen:FL Haut-parleurs:FL`).
    Link {
        /// Sortie source.
        src: String,
        /// Entrée destination.
        dst: String,
    },
    /// Supprime un lien par identifiant (`l3.0`).
    Unlink {
        /// Lien.
        link: String,
    },
    /// Règle le gain d'un nœud ou d'un lien : `-6`, `+3dB`, `-inf`, `mute`, `unmute`.
    Volume {
        /// Nœud (`n3.0`, nom) ou lien (`l3.0`).
        target: String,
        /// Valeur.
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Affiche ou change le pilote de graphe : `auto`, `internal` ou un périphérique.
    Driver {
        /// Choix (absent = afficher).
        choice: Option<String>,
    },
    /// Gestion des câbles virtuels.
    Cable {
        /// Sous-commande.
        #[command(subcommand)]
        cmd: CableCmd,
    },
    /// Affiche les événements en continu.
    Monitor {
        /// S'arrêter après N événements.
        #[arg(long)]
        count: Option<usize>,
        /// S'arrêter après T secondes.
        #[arg(long)]
        timeout: Option<f64>,
    },
    /// Compteurs de xruns et temps de cycle.
    Xruns {
        /// Remettre à zéro.
        #[arg(long)]
        reset: bool,
    },
    /// Rapport de diagnostic (à joindre à un rapport de bogue).
    Dump,
    /// Sauvegarde l'état maintenant.
    Save,
    /// Recharge l'état persisté.
    Load,
    /// Ajoute un nœud interne : `add <type> <nom> [options]`.
    Add {
        /// Type et nom.
        #[command(subcommand)]
        kind: AddKind,
    },
    /// Retire un nœud interne (ou un périphérique absent).
    Remove {
        /// Nœud.
        node: String,
    },
    /// Règle un paramètre d'un nœud interne (`frequency`, `amplitude`, `band.0.gain_db`).
    Param {
        /// Nœud.
        node: String,
        /// Paramètre.
        name: String,
        /// Valeur.
        #[arg(allow_hyphen_values = true)]
        value: f32,
    },
    /// Lit un VU-mètre.
    Meter {
        /// Nœud.
        node: String,
    },
    /// Renomme un nœud (alias Conduit).
    Label {
        /// Nœud.
        node: String,
        /// Nom.
        label: String,
    },
}

/// Sous-commandes câble.
#[derive(Debug, Subcommand)]
pub enum CableCmd {
    /// Liste les câbles.
    List,
    /// Crée un câble.
    Add {
        /// Nom OS.
        #[arg(long)]
        name: Option<String>,
        /// Canaux (1 à 8).
        #[arg(long, default_value_t = 2)]
        channels: u8,
    },
    /// Supprime un câble.
    Remove {
        /// Numéro.
        id: u32,
    },
    /// Renomme un câble.
    Rename {
        /// Numéro.
        id: u32,
        /// Nom.
        name: String,
    },
    /// Change le nombre de canaux (court silence).
    Channels {
        /// Numéro.
        id: u32,
        /// Canaux.
        channels: u8,
    },
}

/// Types de nœuds internes.
#[derive(Debug, Subcommand)]
pub enum AddKind {
    /// Générateur sinusoïdal.
    Sine {
        /// Nom unique du nœud.
        name: String,
        /// Fréquence (Hz).
        #[arg(long, default_value_t = 440.0)]
        frequency: f32,
        /// Amplitude (0–1).
        #[arg(long, default_value_t = 0.5)]
        amplitude: f32,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Bruit rose (ou blanc).
    Noise {
        /// Nom unique du nœud.
        name: String,
        /// Blanc au lieu de rose.
        #[arg(long)]
        white: bool,
        /// Amplitude.
        #[arg(long, default_value_t = 0.1)]
        amplitude: f32,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Silence.
    Silence {
        /// Nom unique du nœud.
        name: String,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Mixeur N bus → 1.
    Mixer {
        /// Nom unique du nœud.
        name: String,
        /// Bus.
        #[arg(long, default_value_t = 2)]
        buses: usize,
        /// Canaux par bus.
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Duplicateur 1 → N.
    Splitter {
        /// Nom unique du nœud.
        name: String,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
        /// Copies.
        #[arg(long, default_value_t = 2)]
        copies: usize,
    },
    /// VU-mètre passe-plat.
    Meter {
        /// Nom unique du nœud.
        name: String,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
    },
    /// Égaliseur paramétrique : bandes `type:fréquence:Q:gain` (ex. `peak:1000:1:-3`,
    /// `lowshelf:120:0.7:3`, `lowpass:8000:0.7`).
    Eq {
        /// Nom unique du nœud.
        name: String,
        /// Canaux.
        #[arg(long, default_value_t = 2)]
        channels: usize,
        /// Bandes.
        #[arg(long = "band", allow_hyphen_values = true)]
        bands: Vec<String>,
    },
    /// Adaptation de canaux (mono ↔ stéréo, 5.1 → stéréo).
    Adapter {
        /// Nom unique du nœud.
        name: String,
        /// Entrées.
        #[arg(long)]
        inputs: usize,
        /// Sorties.
        #[arg(long)]
        outputs: usize,
    },
}

impl AddKind {
    /// Convertit en nom et type du protocole.
    pub fn into_kind(self) -> Result<(String, InternalKind), CliError> {
        Ok(match self {
            AddKind::Sine {
                name,
                frequency,
                amplitude,
                channels,
            } => (
                name,
                InternalKind::Sine {
                    frequency,
                    amplitude,
                    channels,
                },
            ),
            AddKind::Noise {
                name,
                white,
                amplitude,
                channels,
            } => (
                name,
                InternalKind::Noise {
                    pink: !white,
                    amplitude,
                    channels,
                },
            ),
            AddKind::Silence { name, channels } => (name, InternalKind::Silence { channels }),
            AddKind::Mixer {
                name,
                buses,
                channels,
            } => (name, InternalKind::Mixer { buses, channels }),
            AddKind::Splitter {
                name,
                channels,
                copies,
            } => (name, InternalKind::Splitter { channels, copies }),
            AddKind::Meter { name, channels } => (name, InternalKind::Meter { channels }),
            AddKind::Eq {
                name,
                channels,
                bands,
            } => (
                name,
                InternalKind::Equalizer {
                    channels,
                    bands: bands
                        .iter()
                        .map(|b| parse_band(b))
                        .collect::<Result<_, _>>()?,
                },
            ),
            AddKind::Adapter {
                name,
                inputs,
                outputs,
            } => (name, InternalKind::Adapter { inputs, outputs }),
        })
    }
}

/// Analyse `type:fréquence[:Q[:gain]]`.
pub fn parse_band(s: &str) -> Result<EqBand, CliError> {
    use conduit_core::dsp::FilterKind;
    let parts: Vec<&str> = s.split(':').collect();
    let usage = || {
        CliError::Usage(format!("bande invalide « {s} » : type:fréquence[:Q[:gain]] avec type ∈ peak, lowshelf, highshelf, lowpass, highpass, notch, bandpass"))
    };
    if parts.len() < 2 {
        return Err(usage());
    }
    let kind = match parts[0].to_lowercase().as_str() {
        "peak" | "peaking" | "bell" => FilterKind::Peaking,
        "lowshelf" | "ls" => FilterKind::LowShelf,
        "highshelf" | "hs" => FilterKind::HighShelf,
        "lowpass" | "lp" => FilterKind::LowPass,
        "highpass" | "hp" => FilterKind::HighPass,
        "notch" => FilterKind::Notch,
        "bandpass" | "bp" => FilterKind::BandPass,
        "allpass" => FilterKind::AllPass,
        _ => return Err(usage()),
    };
    let frequency: f32 = parts[1].parse().map_err(|_| usage())?;
    let q: f32 = parts
        .get(2)
        .map(|q| q.parse().map_err(|_| usage()))
        .transpose()?
        .unwrap_or(0.707);
    let gain_db: f32 = parts
        .get(3)
        .map(|g| g.parse().map_err(|_| usage()))
        .transpose()?
        .unwrap_or(0.0);
    Ok(EqBand {
        kind,
        frequency,
        q,
        gain_db,
        enabled: true,
    })
}

/// Cible d'un réglage de volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeTarget {
    /// Un nœud.
    Node(NodeId),
    /// Un lien.
    Link(LinkId),
}
