//! Messages d'erreur orientés action (M2-13, ADR-006).
//!
//! « Les messages d'erreur disent quoi faire, pas seulement ce qui a échoué »
//! (ADR-006). Ce module ne réécrit rien : il **complète**.
//!
//! Le message du démon reste affiché **tel quel et en premier** — c'est lui
//! qui connaît le détail, et lui seul sait quel nœud, quel câble, quel
//! périphérique (ADR-010 : les erreurs du moteur sont converties en
//! [`ProtocolError`] dans le moteur lui-même). Ce module y ajoute un
//! **conseil**, choisi sur le seul [`ErrorCode`] : rafraîchir, supprimer un
//! lien, vérifier le pilote, réessayer, consulter le journal.
//!
//! Le `match` de [`conseil`] est **exhaustif**, sans bras `_` : le jour où le
//! protocole gagne un code d'erreur, le compilateur le dira ici plutôt que de
//! laisser passer un conseil générique.
//!
//! Les erreurs **locales** de la GUI — un lien refusé pour boucle, un démon
//! qu'on n'a pas su lancer, un rapport qu'on n'a pas su écrire — passent par
//! [`locale`], qui compose exactement de la même façon : l'utilisateur n'a pas
//! à savoir qui, de la fenêtre ou du démon, a constaté l'erreur.

use conduit_protocol::{ErrorCode, ProtocolError};

use crate::i18n::{self, Text};

/// Le conseil qui complète le message du démon pour ce code d'erreur.
///
/// Chaque code a le sien, et il dit **quoi faire ensuite**.
pub fn conseil(code: ErrorCode) -> Text {
    match code {
        ErrorCode::NotFound => Text::AdviceNotFound,
        ErrorCode::WouldCycle => Text::AdviceWouldCycle,
        ErrorCode::Invalid => Text::AdviceInvalid,
        ErrorCode::Device => Text::AdviceDevice,
        ErrorCode::Cable => Text::AdviceCable,
        ErrorCode::Busy => Text::AdviceBusy,
        ErrorCode::Unsupported => Text::AdviceUnsupported,
        ErrorCode::Internal => Text::AdviceInternal,
    }
}

/// Le texte complet d'une erreur du démon : son message, puis le conseil de
/// son code.
pub fn du_demon(erreur: &ProtocolError) -> String {
    locale(&erreur.message, conseil(erreur.code))
}

/// Le texte complet d'une erreur constatée par la GUI elle-même.
///
/// Le format est celui de [`du_demon`], et c'est tout l'intérêt : un refus de
/// boucle décidé sur place (F-12) se lit comme un refus du démon.
pub fn locale(message: &str, conseil: Text) -> String {
    i18n::erreur_conseillee(message, i18n::t(conseil))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les huit codes du protocole. Le `match` de [`conseil`] étant exhaustif,
    /// une variante ajoutée casse la compilation ; cette liste, elle, garde le
    /// test complet.
    const CODES: [ErrorCode; 8] = [
        ErrorCode::NotFound,
        ErrorCode::WouldCycle,
        ErrorCode::Invalid,
        ErrorCode::Device,
        ErrorCode::Cable,
        ErrorCode::Busy,
        ErrorCode::Unsupported,
        ErrorCode::Internal,
    ];

    /// Chaque code a un conseil, aucun n'est vide, et deux codes ne partagent
    /// pas le même — un conseil qui vaudrait pour tout ne dirait rien.
    #[test]
    fn chaque_code_a_son_conseil_et_aucun_n_est_vide() {
        let mut vus = Vec::new();
        for code in CODES {
            let texte = i18n::t(conseil(code));
            assert!(!texte.trim().is_empty(), "conseil vide pour {code:?}");
            assert!(!vus.contains(&texte), "conseil partagé : {code:?}");
            vus.push(texte);
        }
        assert_eq!(vus.len(), CODES.len());
    }

    /// Le message du démon vient en premier et tel quel ; le conseil le suit.
    #[test]
    fn le_message_du_demon_reste_en_premier_et_intact() {
        let erreur = ProtocolError::new(ErrorCode::NotFound, "nœud inconnu : 3");
        let texte = du_demon(&erreur);
        assert!(texte.starts_with("nœud inconnu : 3"), "{texte}");
        assert!(texte.ends_with(i18n::t(Text::AdviceNotFound)), "{texte}");
        assert!(texte.contains(" — "), "le tiret sépare les deux : {texte}");
    }

    /// Un message déjà ponctué n'est pas recousu au tiret : deux phrases se
    /// suivent d'une espace.
    #[test]
    fn un_message_ponctue_enchaine_sans_tiret() {
        let texte = locale(
            "Lien refusé : il créerait une boucle.",
            Text::AdviceWouldCycle,
        );
        assert_eq!(
            texte,
            "Lien refusé : il créerait une boucle. \
             Supprimez d'abord un lien du chemin de retour, puis refaites celui-ci."
        );
    }

    /// Un démon qui ne dit rien laisse le conseil seul, plutôt qu'un tiret
    /// suspendu en tête de notice.
    #[test]
    fn un_message_vide_laisse_le_conseil_seul() {
        let erreur = ProtocolError::new(ErrorCode::Busy, "  ");
        assert_eq!(du_demon(&erreur), i18n::t(Text::AdviceBusy));
    }
}
