//! Binaire `conduit-looptest` (ROADMAP M1a-10) : tout est dans la bibliothèque
//! du même crate, ce fichier ne fait que traduire le résultat en code de retour.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use conduit_looptest::cli::Args;

fn main() -> ExitCode {
    let args = Args::parse();
    match conduit_looptest::run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("erreur : {message}");
            ExitCode::from(conduit_looptest::EXIT_ENVIRONMENT)
        }
    }
}
