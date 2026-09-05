//! Binaire `conduit` : interface graphique de Conduit.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;

/// Interface graphique de Conduit : câbles, patchbay et diagnostic.
#[derive(Debug, Parser)]
#[command(name = "conduit", version, about, long_about = None)]
struct Args {
    /// Socket (Unix) ou tube nommé (Windows) du démon ; par défaut celui de
    /// l'utilisateur courant.
    #[arg(long, value_name = "CHEMIN")]
    socket: Option<PathBuf>,
}

fn main() -> iced::Result {
    let args = Args::parse();
    conduit_gui::init_logging();
    conduit_gui::run(args.socket.unwrap_or_else(conduit_gui::ipc::default_socket))
}
