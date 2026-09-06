//! Session Windows du processus courant.
//!
//! Ce n'est pas de l'audio, mais c'en est la condition : le moteur audio de Windows
//! appartient à une **session ouverte par un utilisateur**. Un processus lancé dans
//! la session des services ([`SERVICES_SESSION`], session 0 — un service Windows,
//! une tâche planifiée « même si l'utilisateur n'est pas connecté », un agent
//! d'exécution à distance) n'y a pas accès : il voit peu ou pas d'endpoints, et ce
//! qu'il joue ne va nulle part. Une mesure y est **sans objet**, pas seulement
//! ratée.
//!
//! Le renseignement vit ici, dans le crate qui sait ce qu'est un endpoint audio,
//! parce qu'il ne sert qu'à interpréter un silence : `conduit-looptest` interdit
//! l'`unsafe` et n'a pas à ouvrir Win32 pour lui seul.

use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;

/// La session des services : aucun périphérique audio de l'utilisateur n'y est
/// visible.
pub const SERVICES_SESSION: u32 = 0;

/// Session Windows du processus courant ; `None` si Windows refuse de la dire.
///
/// Comparer à [`SERVICES_SESSION`] pour savoir si une mesure audio a un sens.
pub fn current_session_id() -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: `GetCurrentProcessId` n'a pas de précondition et son résultat est
    // toujours un identifiant valide ; `session` est une variable locale
    // initialisée, écrite seulement si l'appel réussit.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.is_ok();
    ok.then_some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les tests tournent dans la session de l'utilisateur qui lance `cargo test` :
    /// l'appel doit répondre, et répondre autre chose que la session des services.
    #[test]
    fn la_session_du_test_est_celle_d_un_utilisateur() {
        let session = current_session_id().expect("ProcessIdToSessionId");
        assert_ne!(
            session, SERVICES_SESSION,
            "les tests tournent dans la session des services : aucune mesure audio n'y a de sens"
        );
    }
}
