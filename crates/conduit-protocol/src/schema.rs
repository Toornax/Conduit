//! Export JSON Schema et génération de `docs/protocol.md`.

use schemars::{schema_for, JsonSchema};

use crate::api::{Command, Notification, Reply};
use crate::wire::{Message, PROTOCOL_VERSION};

/// Schéma JSON d'un type, sérialisé et indenté.
pub fn schema_json<T: JsonSchema>() -> String {
    serde_json::to_string_pretty(&schema_for!(T)).expect("schéma sérialisable")
}

/// Schéma JSON du message enveloppe (tout le protocole).
pub fn message_schema() -> String {
    schema_json::<Message>()
}

fn variant_names<T: JsonSchema>() -> Vec<String> {
    let schema = schema_for!(T);
    let value = serde_json::to_value(schema).expect("schéma");
    let mut names = Vec::new();
    if let Some(one_of) = value.get("oneOf").and_then(|v| v.as_array()) {
        for v in one_of {
            for tag in ["cmd", "reply", "event"] {
                if let Some(t) = v
                    .pointer(&format!("/properties/{tag}/const"))
                    .and_then(|c| c.as_str())
                {
                    names.push(t.to_string());
                }
            }
        }
    }
    names
}

/// Génère la documentation Markdown du protocole (liste des commandes, réponses,
/// événements et schéma complet).
pub fn protocol_markdown() -> String {
    let mut md = String::new();
    md.push_str("# Protocole de contrôle Conduit\n\n");
    md.push_str("<!-- Généré par `cargo run -p conduit-protocol --features schema --example gen-docs` ; ne pas éditer. -->\n\n");
    md.push_str(&format!(
        "Version du protocole : **{PROTOCOL_VERSION}**.\n\n"
    ));
    md.push_str("## Transport\n\n");
    md.push_str("Socket Unix (Linux, macOS) ou named pipe (Windows), jamais le réseau. ");
    md.push_str("Chaque message est une trame : `u32` petit-boutiste (longueur de la charge) puis charge **MessagePack** ");
    md.push_str("(champs nommés). Charge maximale : 1 MiB. Un client envoie d'abord `hello`, le démon répond `hello_reply`.\n\n");
    md.push_str("Les exemples ci-dessous sont en JSON pour la lisibilité ; l'encodage réel est MessagePack avec les mêmes clés.\n\n");
    md.push_str("## Commandes (`msg = request`, champ `command.cmd`)\n\n");
    for c in variant_names::<Command>() {
        md.push_str(&format!("- `{c}`\n"));
    }
    md.push_str("\n## Réponses (`msg = response`, `result.Ok.reply`)\n\n");
    for r in variant_names::<Reply>() {
        md.push_str(&format!("- `{r}`\n"));
    }
    md.push_str("\n## Événements (`msg = event`, champ `notification.event`)\n\n");
    for e in variant_names::<Notification>() {
        md.push_str(&format!("- `{e}`\n"));
    }
    md.push_str("\n## Exemple\n\n```json\n");
    md.push_str(&serde_json::to_string_pretty(&Message::request(1, Command::Status)).unwrap());
    md.push_str("\n```\n\n## Schéma JSON complet\n\n```json\n");
    md.push_str(&message_schema());
    md.push_str("\n```\n");
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_every_command() {
        let names = variant_names::<Command>();
        assert!(names.contains(&"status".to_string()));
        assert!(names.contains(&"cable_add".to_string()));
        assert!(names.len() >= 25, "{names:?}");
        assert!(variant_names::<Reply>().contains(&"nodes".to_string()));
        assert!(variant_names::<Notification>().contains(&"shutdown".to_string()));
        let md = protocol_markdown();
        assert!(md.contains("Version du protocole"));
        assert!(md.contains("\"msg\": \"request\""));
        assert!(message_schema().contains("hello_reply"));
    }
}
