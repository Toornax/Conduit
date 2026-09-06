//! Arrêt propre à la fermeture de session Windows (M1b-35).
//!
//! ## Pourquoi pas `SetConsoleCtrlHandler`
//!
//! On lit souvent qu'un programme console attrape la fermeture de session avec
//! `CTRL_LOGOFF_EVENT`. La documentation de `HandlerRoutine` dit le contraire pour notre
//! cas : « *Note that this signal is received only by services. Interactive applications
//! are terminated at logoff, so they are not present when the system sends this signal* »
//! (et pour `CTRL_SHUTDOWN_EVENT` : « *Interactive applications are not present by the
//! time the system sends this signal* »).
//! <https://learn.microsoft.com/en-us/windows/console/handlerroutine>
//!
//! `conduitd` est une application interactive (ADR-013 : surtout pas un service) : ces
//! deux signaux ne lui parviendront pas. Et lancé par une tâche planifiée, il n'a de
//! toute façon pas de console à laquelle les rattacher.
//!
//! ## Pourquoi pas une fenêtre `HWND_MESSAGE`
//!
//! Le réflexe suivant est la fenêtre *message-only*. Elle ne convient pas non plus :
//! « *A message-only window enables you to send and receive messages. It is not visible,
//! has no z-order, cannot be enumerated, and **does not receive broadcast messages***. »
//! <https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features>
//!
//! Or `WM_QUERYENDSESSION` est justement diffusé aux fenêtres **de premier niveau** :
//! « *the system sends the WM_QUERYENDSESSION message to each window* »
//! (<https://learn.microsoft.com/en-us/windows/win32/shutdown/logging-off>), et une
//! fenêtre message-only est en interne une fenêtre *enfant* de `HWND_MESSAGE` — elle est
//! hors de cette diffusion.
//!
//! ## Ce qu'on fait
//!
//! Un **fil dédié** crée une fenêtre de **premier niveau** jamais montrée
//! (`WS_OVERLAPPED` sans `WS_VISIBLE`, `WS_EX_TOOLWINDOW` pour ne pas apparaître dans
//! Alt+Tab), et fait tourner sa boucle de messages :
//!
//! - `WM_QUERYENDSESSION` → `TRUE` immédiatement. La documentation est explicite :
//!   « *Each application should return TRUE or FALSE immediately upon receiving this
//!   message, and defer any cleanup operations until it receives the WM_ENDSESSION
//!   message* ». On ne bloque jamais une fermeture de session : c'est la décision de
//!   l'utilisateur, pas la nôtre.
//! - `WM_ENDSESSION` (avec `wParam` vrai) → on réveille le démon, **puis on attend** que
//!   son arrêt propre soit terminé avant de rendre la main, parce que le système peut
//!   tuer le processus dès le retour de la procédure de fenêtre. L'attente est bornée à
//!   [`GRACE`] : au-delà, Windows tue de toute façon (`SPI_GETWAITTOKILLTIMEOUT`, 5 s par
//!   défaut).
//!
//! `ShutdownBlockReasonCreate` n'est pas utilisé : il sert à **retarder** l'arrêt en
//! affichant une raison à l'utilisateur, ce que Microsoft réserve aux travaux qu'on ne
//! peut pas interrompre (gravure, transaction). La sauvegarde de Conduit est l'écriture
//! atomique d'un petit JSON, et le service la refait déjà toutes les 500 ms : elle tient
//! très largement dans le budget ordinaire.
//!
//! Sous Unix rien ne change : `SIGTERM` et Ctrl-C restent le chemin d'arrêt.

// Seul module du démon à en contenir : les fenêtres Win32 n'ont pas de façade sûre dans
// le crate `windows`. Chaque bloc porte son `SAFETY:` (cf. docs/dev-guide.md §6).
#![allow(unsafe_code)]

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, MSG, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_OVERLAPPED,
};

/// Temps laissé au démon pour se fermer proprement pendant `WM_ENDSESSION`.
///
/// Windows tue les applications qui n'ont pas rendu la main au bout de
/// `SPI_GETWAITTOKILLTIMEOUT` (5 s par défaut) ; on s'arrête avant, pour que la fenêtre
/// rende la main d'elle-même plutôt que d'être tuée au milieu.
pub const GRACE: Duration = Duration::from_secs(3);

/// Nom de la classe de fenêtre.
const CLASS_NAME: PCWSTR = w!("ConduitdSessionEnd");

/// État partagé entre la procédure de fenêtre (fil Win32) et le démon (runtime tokio).
#[derive(Debug)]
struct Shared {
    /// Réveille le démon quand la session se termine.
    notify: tokio::sync::Notify,
    /// Vrai quand le démon a fini son arrêt propre.
    done: Mutex<bool>,
    /// Réveille le fil Win32 quand `done` passe à vrai.
    finished: Condvar,
}

/// La procédure de fenêtre est une `extern "system" fn` : elle ne reçoit aucun contexte,
/// l'état passe forcément par une variable globale.
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// Poignée rendue au démon.
#[derive(Debug)]
pub struct SessionEnd {
    shared: Arc<Shared>,
}

impl SessionEnd {
    /// Attend la fin de session.
    pub async fn wait(&self) {
        self.shared.notify.notified().await;
    }

    /// Signale que l'arrêt propre est terminé : la procédure de fenêtre peut rendre la
    /// main, et le système achever la fermeture de session.
    pub fn acknowledge(&self) {
        let mut done = lock(&self.shared.done);
        *done = true;
        self.shared.finished.notify_all();
    }
}

/// Verrouille sans jamais paniquer : un verrou empoisonné ne doit pas empêcher un arrêt.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Démarre le fil qui écoute la fin de session.
///
/// Rend `None` si un autre appel l'a déjà fait dans ce processus. Si la fenêtre ne peut
/// pas être créée (pas de station de fenêtres — un service, un conteneur), le fil le
/// journalise et la poignée ne se réveillera jamais : le démon garde son comportement
/// habituel.
pub fn spawn() -> Option<SessionEnd> {
    let shared = Arc::new(Shared {
        notify: tokio::sync::Notify::new(),
        done: Mutex::new(false),
        finished: Condvar::new(),
    });
    if SHARED.set(Arc::clone(&shared)).is_err() {
        tracing::debug!("fenêtre de fin de session déjà installée dans ce processus");
        return None;
    }
    let thread = std::thread::Builder::new()
        .name("conduitd-session-end".into())
        .spawn(message_loop);
    match thread {
        Ok(_) => Some(SessionEnd { shared }),
        Err(e) => {
            tracing::warn!("fil de fin de session : {e} — pas d'arrêt propre à la déconnexion");
            None
        }
    }
}

/// Crée la fenêtre cachée et pompe ses messages jusqu'à la fin du processus.
fn message_loop() {
    // SAFETY: `GetModuleHandleW(None)` rend le module du processus courant ; l'appel ne
    // déréférence rien et ne peut échouer que si le module n'existe pas.
    let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
        Ok(m) => m.into(),
        Err(e) => {
            tracing::warn!("GetModuleHandleW : {e} — pas d'arrêt propre à la fin de session");
            return;
        }
    };
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` est valide et vit jusqu'à la fin de l'appel ; `RegisterClassW` en
    // copie le contenu. `lpszClassName` pointe une chaîne large statique.
    if unsafe { RegisterClassW(&class) } == 0 {
        tracing::warn!(
            "RegisterClassW : {} — pas d'arrêt propre à la fin de session",
            std::io::Error::last_os_error()
        );
        return;
    }
    // SAFETY: la classe vient d'être enregistrée ; tous les pointeurs sont des chaînes
    // larges statiques et les poignées facultatives sont `None`.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Conduit"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    if let Err(e) = window {
        tracing::warn!("CreateWindowExW : {e} — pas d'arrêt propre à la fin de session");
        return;
    }
    tracing::debug!("fenêtre de fin de session installée (WM_QUERYENDSESSION/WM_ENDSESSION)");
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` est une structure valide et exclusivement à nous ; `None`
        // demande les messages de toutes les fenêtres de ce fil.
        let more = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if more.0 <= 0 {
            break;
        }
        // SAFETY: `message` vient d'être rempli par `GetMessageW`.
        unsafe { DispatchMessageW(&message) };
    }
}

/// Procédure de la fenêtre cachée.
extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // « Acceptez-vous la fin de session ? » — toujours oui, sans rien faire d'autre.
        WM_QUERYENDSESSION => LRESULT(1),
        // La session se termine pour de bon : c'est ici qu'on nettoie.
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                end_session();
            }
            LRESULT(0)
        }
        // SAFETY: paramètres reçus tels quels du système, rendus tels quels.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Réveille le démon et attend son arrêt, au plus [`GRACE`].
fn end_session() {
    let Some(shared) = SHARED.get() else {
        return;
    };
    shared.notify.notify_one();
    let deadline = Instant::now() + GRACE;
    let mut done = lock(&shared.done);
    while !*done {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let (guard, timeout) = shared
            .finished
            .wait_timeout(done, remaining)
            .unwrap_or_else(|e| e.into_inner());
        done = guard;
        if timeout.timed_out() {
            break;
        }
    }
    // Le fil ne pompera plus de messages : le processus est sur le point de disparaître.
    // SAFETY: appel sans argument, sur le fil qui possède la fenêtre.
    unsafe { PostQuitMessage(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La poignée se réveille quand la procédure de fenêtre signale la fin de session,
    /// et l'accusé de réception débloque le fil Win32.
    #[tokio::test(flavor = "multi_thread")]
    async fn end_session_wakes_the_daemon_and_waits_for_it() {
        let Some(session) = spawn() else {
            eprintln!("test sauté : fenêtre de fin de session indisponible");
            return;
        };
        // Le fil « Win32 » simulé : signale, puis attend l'accusé de réception.
        let start = Instant::now();
        let worker = std::thread::spawn(end_session);
        session.wait().await;
        assert!(!*lock(&SHARED.get().unwrap().done), "pas encore acquitté");
        session.acknowledge();
        worker.join().unwrap();
        assert!(
            start.elapsed() < GRACE,
            "l'accusé de réception doit débloquer avant l'expiration du délai"
        );
        // Un second `spawn` dans le même processus est refusé (une seule globale).
        assert!(spawn().is_none());
    }
}
