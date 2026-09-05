//! Journalisation `tracing` : stderr + fichier tournant quotidien (M0-81).

use std::path::Path;

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Garde à conserver tant que le démon tourne (vide le tampon du fichier à la fin).
pub type LogGuard = Option<tracing_appender::non_blocking::WorkerGuard>;

/// Initialise la journalisation. `level` est un niveau ou un filtre `tracing`
/// (`info`, `conduitd=debug,conduit_engine=trace`). `log_dir = None` = stderr seul.
/// Retourne `None` si un abonné global existe déjà (tests).
pub fn init(level: &str, log_dir: Option<&Path>) -> LogGuard {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(true);
    let registry = tracing_subscriber::registry().with(filter).with(stderr);
    match log_dir {
        Some(dir) => {
            let appender = tracing_appender::rolling::daily(dir, "conduitd.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let file = fmt::layer().with_writer(writer).with_ansi(false);
            let _ = registry.with(file).try_init();
            Some(guard)
        }
        None => {
            let _ = registry.try_init();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_logging_writes_rotating_file() {
        let dir = tempfile::tempdir().unwrap();
        let guard = init("debug", Some(dir.path()));
        tracing::info!("bonjour");
        drop(guard);
        // Le nom porte la date : conduitd.log.YYYY-MM-DD.
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.starts_with("conduitd.log")),
            "{names:?}"
        );
        // Un second init ne panique pas (abonné global déjà présent).
        let _ = init("garbage level!!", None);
    }
}
