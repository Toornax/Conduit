//! Liste les endpoints audio Windows, puis affiche les événements pendant
//! `--watch <secondes>` (0 par défaut : on s'arrête après la liste).
//!
//! ```sh
//! cargo run -p conduit-backend-wasapi --example list -- --watch 30
//! ```

#[cfg(windows)]
fn main() -> Result<(), conduit_backend::BackendError> {
    use std::time::{Duration, Instant};

    use conduit_backend::{Backend, DeviceEvent};
    use conduit_backend_wasapi::WasapiBackend;

    let watch = watch_seconds(std::env::args().skip(1));

    let mut backend = WasapiBackend::new()?;
    // L'abonnement précède la liste : rien de ce qui change ensuite n'est perdu.
    let events = backend.subscribe();
    let devices = backend.devices()?;
    println!("{} périphérique(s) actif(s) :", devices.len());
    for device in &devices {
        println!("{}", describe(device));
    }

    if watch == 0 {
        return Ok(());
    }
    println!("\nsurveillance pendant {watch} s (branchez ou débranchez un périphérique)…");
    let deadline = Instant::now() + Duration::from_secs(watch);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(DeviceEvent::Added(info)) => println!("+ ajouté   {}", describe(&info)),
            Ok(DeviceEvent::Removed { id }) => println!("- retiré   {id}"),
            Ok(DeviceEvent::DefaultChanged { direction, id }) => println!(
                "* défaut   {direction} → {}",
                id.map_or_else(|| "(aucun)".to_string(), |id| id.to_string())
            ),
            Ok(DeviceEvent::CableChanged { id, info }) => println!("~ câble    {id} → {info:?}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("le fil MMDevice s'est arrêté");
                break;
            }
        }
    }
    Ok(())
}

/// Valeur de `--watch <secondes>` (ou `--watch=<secondes>`), 0 par défaut.
#[cfg(windows)]
fn watch_seconds(mut args: impl Iterator<Item = String>) -> u64 {
    while let Some(arg) = args.next() {
        if arg == "--watch" {
            return args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        if let Some(value) = arg.strip_prefix("--watch=") {
            return value.parse().unwrap_or(0);
        }
    }
    0
}

#[cfg(windows)]
fn describe(device: &conduit_backend::DeviceInfo) -> String {
    let rates: Vec<String> = device
        .sample_rates
        .iter()
        .map(|r| r.hz().to_string())
        .collect();
    format!(
        "  [{}] {}{}\n      format de mixage {} canaux à {} Hz (acceptées : {}), bloc {} trames{}\n      id {}",
        device.direction,
        device.name,
        if device.is_default { " (défaut)" } else { "" },
        device.channels,
        device.sample_rate.hz(),
        if rates.is_empty() {
            "aucune autre".to_string()
        } else {
            rates.join(", ")
        },
        device.default_block,
        device
            .cable
            .map_or_else(String::new, |c| format!(", câble {c}")),
        device.id,
    )
}

#[cfg(not(windows))]
fn main() {}
