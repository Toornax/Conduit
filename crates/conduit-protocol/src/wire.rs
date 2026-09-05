//! Enveloppe des messages et négociation de version.

use serde::{Deserialize, Serialize};

use crate::api::{Command, Notification, ProtocolError, Reply};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// Version majeure du protocole. Incrémentée à tout changement incompatible.
pub const PROTOCOL_VERSION: u32 = 1;

/// Plus ancienne version de client encore acceptée par ce démon.
pub const MIN_CLIENT_VERSION: u32 = 1;

/// Identifiant de requête, choisi par le client, renvoyé dans la réponse.
pub type RequestId = u32;

/// Premier message envoyé par un client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Hello {
    /// Version du protocole parlée par le client.
    pub version: u32,
    /// Nom du client (`"conduitctl 0.1.0"`), pour les journaux.
    pub client: String,
}

/// Réponse du démon à [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct HelloReply {
    /// Version du protocole du démon.
    pub version: u32,
    /// Nom et version du démon.
    pub server: String,
    /// Vrai si le démon accepte de continuer avec ce client.
    pub accepted: bool,
    /// Explication en cas de refus (quoi faire).
    pub reason: Option<String>,
}

/// Résultat de la négociation côté démon.
pub fn negotiate(hello: &Hello, server: &str) -> HelloReply {
    let (accepted, reason) = if hello.version > PROTOCOL_VERSION {
        (
            false,
            Some(format!(
                "client trop récent (protocole {}) pour ce démon (protocole {PROTOCOL_VERSION}) : mettez à jour Conduit",
                hello.version
            )),
        )
    } else if hello.version < MIN_CLIENT_VERSION {
        (
            false,
            Some(format!(
                "client trop ancien (protocole {}) : ce démon exige au moins la version {MIN_CLIENT_VERSION}, mettez à jour le client",
                hello.version
            )),
        )
    } else {
        (true, None)
    };
    HelloReply {
        version: PROTOCOL_VERSION,
        server: server.to_string(),
        accepted,
        reason,
    }
}

/// Vérifie côté client la réponse du démon.
pub fn negotiate_client(reply: &HelloReply) -> Result<(), String> {
    if !reply.accepted {
        return Err(reply
            .reason
            .clone()
            .unwrap_or_else(|| "connexion refusée par le démon".into()));
    }
    if reply.version != PROTOCOL_VERSION {
        return Err(format!(
            "le démon parle le protocole {} et ce client le protocole {PROTOCOL_VERSION} : mettez à jour Conduit",
            reply.version
        ));
    }
    Ok(())
}

/// Requête : identifiant + commande.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Request {
    /// Identifiant choisi par le client.
    pub id: RequestId,
    /// Commande.
    pub command: Command,
}

/// Réponse : identifiant + résultat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Response {
    /// Identifiant de la requête.
    pub id: RequestId,
    /// Résultat.
    pub result: Result<Reply, ProtocolError>,
}

/// Événement diffusé aux abonnés.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Event {
    /// Notification.
    pub notification: Notification,
}

/// Tout message circulant sur le transport, dans les deux sens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Message {
    /// Client → démon, premier message.
    Hello(Hello),
    /// Démon → client, en réponse à `Hello`.
    HelloReply(HelloReply),
    /// Client → démon.
    Request(Request),
    /// Démon → client.
    Response(Response),
    /// Démon → abonnés.
    Event(Event),
}

impl Message {
    /// Requête.
    pub fn request(id: RequestId, command: Command) -> Self {
        Message::Request(Request { id, command })
    }

    /// Réponse réussie.
    pub fn reply(id: RequestId, reply: Reply) -> Self {
        Message::Response(Response {
            id,
            result: Ok(reply),
        })
    }

    /// Réponse en erreur.
    pub fn error(id: RequestId, error: ProtocolError) -> Self {
        Message::Response(Response {
            id,
            result: Err(error),
        })
    }

    /// Événement.
    pub fn event(notification: Notification) -> Self {
        Message::Event(Event { notification })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ErrorCode;

    #[test]
    fn negotiation_accepts_current_and_refuses_others_with_advice() {
        let ok = negotiate(
            &Hello {
                version: PROTOCOL_VERSION,
                client: "test".into(),
            },
            "conduitd 0.1",
        );
        assert!(ok.accepted);
        assert_eq!(ok.version, PROTOCOL_VERSION);
        assert!(ok.reason.is_none());
        let newer = negotiate(
            &Hello {
                version: PROTOCOL_VERSION + 1,
                client: "futur".into(),
            },
            "conduitd",
        );
        assert!(!newer.accepted);
        assert!(newer.reason.as_deref().unwrap().contains("trop récent"));
        let older = negotiate(
            &Hello {
                version: 0,
                client: "vieux".into(),
            },
            "conduitd",
        );
        assert!(!older.accepted);
        assert!(older.reason.as_deref().unwrap().contains("trop ancien"));
    }

    #[test]
    fn messages_roundtrip_in_json_and_msgpack() {
        let msgs = vec![
            Message::Hello(Hello {
                version: 1,
                client: "c".into(),
            }),
            Message::HelloReply(HelloReply {
                version: 1,
                server: "s".into(),
                accepted: true,
                reason: None,
            }),
            Message::request(7, Command::Status),
            Message::reply(7, Reply::Ok),
            Message::error(8, ProtocolError::new(ErrorCode::NotFound, "nœud inconnu")),
            Message::event(Notification::Shutdown),
        ];
        for m in &msgs {
            let json = serde_json::to_string(m).unwrap();
            let back: Message = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, m, "{json}");
            let bin = rmp_serde::to_vec_named(m).unwrap();
            let back: Message = rmp_serde::from_slice(&bin).unwrap();
            assert_eq!(&back, m);
        }
        let j = serde_json::to_value(Message::request(1, Command::Status)).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"msg": "request", "id": 1, "command": {"cmd": "status"}})
        );
        let j = serde_json::to_value(Message::event(Notification::Shutdown)).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"msg": "event", "notification": {"event": "shutdown"}})
        );
        let j = serde_json::to_value(Message::error(
            2,
            ProtocolError::new(ErrorCode::Busy, "occupé"),
        ))
        .unwrap();
        assert_eq!(
            j,
            serde_json::json!({"msg": "response", "id": 2, "result": {"Err": {"code": "busy", "message": "occupé"}}})
        );
    }
}
