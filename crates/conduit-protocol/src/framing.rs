//! Trames : `u32` petit-boutiste (longueur de la charge) + charge MessagePack.
//!
//! Le décodeur est incrémental (les octets arrivent par morceaux arbitraires) et
//! borné : une trame annoncée plus grande que [`MAX_FRAME_LEN`] est une erreur, le
//! démon déconnecte alors le client. Aucune entrée ne fait paniquer.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::wire::Message;

/// Taille maximale d'une charge utile (1 MiB).
pub const MAX_FRAME_LEN: usize = 1 << 20;

/// Taille de l'en-tête.
pub const HEADER_LEN: usize = 4;

/// Erreurs de framing / encodage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FramingError {
    /// Trame trop grande.
    #[error("trame de {len} octets : maximum {max}")]
    TooLarge {
        /// Longueur annoncée.
        len: usize,
        /// Maximum.
        max: usize,
    },
    /// Charge MessagePack invalide.
    #[error("message invalide : {0}")]
    Decode(String),
    /// Échec d'encodage (ne devrait pas arriver avec les types du protocole).
    #[error("encodage impossible : {0}")]
    Encode(String),
}

/// Encode une valeur en trame complète (en-tête + charge).
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FramingError> {
    let payload =
        rmp_serde::to_vec_named(value).map_err(|e| FramingError::Encode(e.to_string()))?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(FramingError::TooLarge {
            len: payload.len(),
            max: MAX_FRAME_LEN,
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Encode un [`Message`].
pub fn encode_message(msg: &Message) -> Result<Vec<u8>, FramingError> {
    encode(msg)
}

/// Décode une charge (sans en-tête).
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FramingError> {
    rmp_serde::from_slice(payload).map_err(|e| FramingError::Decode(e.to_string()))
}

/// Décodeur incrémental.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    max_len: usize,
}

impl Decoder {
    /// Décodeur avec la limite par défaut.
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            max_len: MAX_FRAME_LEN,
        }
    }

    /// Décodeur avec une limite personnalisée.
    pub fn with_max_len(max_len: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_len,
        }
    }

    /// Ajoute des octets reçus.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Octets en attente.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Extrait la prochaine charge complète, s'il y en a une. `Ok(None)` = attendre
    /// plus d'octets. Après une erreur, le décodeur doit être abandonné.
    pub fn next_payload(&mut self) -> Result<Option<Vec<u8>>, FramingError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > self.max_len {
            return Err(FramingError::TooLarge {
                len,
                max: self.max_len,
            });
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(payload))
    }

    /// Extrait et décode le prochain message complet.
    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FramingError> {
        match self.next_payload()? {
            Some(p) => decode_payload(&p).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Command, Reply};
    use proptest::prelude::*;

    #[test]
    fn encode_then_decode_in_one_piece() {
        let msg = Message::request(3, Command::Status);
        let bytes = encode_message(&msg).unwrap();
        assert_eq!(&bytes[..4], &((bytes.len() - 4) as u32).to_le_bytes());
        let mut d = Decoder::new();
        d.feed(&bytes);
        let back: Message = d.next_message().unwrap().unwrap();
        assert_eq!(back, msg);
        assert_eq!(d.pending(), 0);
        assert!(d.next_message::<Message>().unwrap().is_none());
    }

    #[test]
    fn decoder_handles_arbitrary_chunking_and_multiple_frames() {
        let msgs: Vec<Message> = (0..5).map(|i| Message::reply(i, Reply::Ok)).collect();
        let mut stream = Vec::new();
        for m in &msgs {
            stream.extend(encode_message(m).unwrap());
        }
        let mut d = Decoder::new();
        let mut got = Vec::new();
        for chunk in stream.chunks(3) {
            d.feed(chunk);
            while let Some(m) = d.next_message::<Message>().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, msgs);
    }

    #[test]
    fn truncated_frame_waits() {
        let bytes = encode_message(&Message::request(1, Command::Nodes)).unwrap();
        let mut d = Decoder::new();
        d.feed(&bytes[..bytes.len() - 1]);
        assert_eq!(d.next_message::<Message>().unwrap(), None);
        d.feed(&bytes[bytes.len() - 1..]);
        assert!(d.next_message::<Message>().unwrap().is_some());
        let mut d = Decoder::new();
        d.feed(&bytes[..2]);
        assert_eq!(d.next_payload().unwrap(), None);
    }

    #[test]
    fn oversized_frame_is_refused_before_buffering() {
        let mut d = Decoder::new();
        d.feed(&(MAX_FRAME_LEN as u32 + 1).to_le_bytes());
        assert!(matches!(
            d.next_payload(),
            Err(FramingError::TooLarge { .. })
        ));
        let mut small = Decoder::with_max_len(8);
        small.feed(&encode_message(&Message::request(1, Command::Status)).unwrap());
        assert!(matches!(
            small.next_payload(),
            Err(FramingError::TooLarge { len: _, max: 8 })
        ));
        let huge = vec![0u8; MAX_FRAME_LEN + 1];
        assert!(matches!(encode(&huge), Err(FramingError::TooLarge { .. })));
    }

    #[test]
    fn corrupted_payload_is_an_error_not_a_panic() {
        let mut d = Decoder::new();
        let payload = [0xc1u8, 0xff, 0x00]; // 0xc1 est réservé en MessagePack
        d.feed(&(payload.len() as u32).to_le_bytes());
        d.feed(&payload);
        assert!(matches!(
            d.next_message::<Message>(),
            Err(FramingError::Decode(_))
        ));
        // Charge valide mais d'un autre type.
        let bytes = encode(&"juste une chaîne").unwrap();
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert!(matches!(
            d.next_message::<Message>(),
            Err(FramingError::Decode(_))
        ));
        assert!(FramingError::Decode("x".into())
            .to_string()
            .contains("invalide"));
    }

    proptest! {
        /// Aucune séquence d'octets ne fait paniquer le décodeur.
        #[test]
        fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
            let mut d = Decoder::with_max_len(256);
            for chunk in bytes.chunks(7) {
                d.feed(chunk);
                loop {
                    match d.next_message::<Message>() {
                        Ok(Some(_)) => continue,
                        Ok(None) => break,
                        Err(_) => return Ok(()),
                    }
                }
            }
        }

        /// Encodage puis décodage par morceaux de taille aléatoire : identité.
        #[test]
        fn chunked_roundtrip(chunk in 1usize..64, id in any::<u32>()) {
            let msg = Message::request(id, Command::CableRename { id: conduit_backend::CableId(id), name: "x".repeat(id as usize % 50) });
            let bytes = encode_message(&msg).unwrap();
            let mut d = Decoder::new();
            let mut out = None;
            for c in bytes.chunks(chunk) {
                d.feed(c);
                if let Some(m) = d.next_message::<Message>().unwrap() {
                    out = Some(m);
                }
            }
            prop_assert_eq!(out, Some(msg));
        }
    }
}
