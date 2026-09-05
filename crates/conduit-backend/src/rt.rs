//! Priorité temps réel du fil courant (M0-62).
//!
//! La promotion est **tentée** ; son échec n'est jamais fatal : le moteur continue
//! avec une priorité normale et l'échec est journalisé par l'appelant.

use core::fmt;

/// Résultat de la promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtOutcome {
    /// Priorité temps réel obtenue (valeur effective, dépendante de l'OS).
    Promoted {
        /// Description lisible (`"SCHED_FIFO 70"`, `"TIME_CRITICAL"`).
        detail: String,
    },
    /// Refusée par l'OS ; explication et remède.
    Refused {
        /// Explication.
        reason: String,
    },
    /// Non implémenté sur cette plateforme.
    Unsupported,
}

impl fmt::Display for RtOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtOutcome::Promoted { detail } => write!(f, "priorité temps réel obtenue ({detail})"),
            RtOutcome::Refused { reason } => write!(f, "priorité temps réel refusée : {reason}"),
            RtOutcome::Unsupported => {
                f.write_str("priorité temps réel non supportée sur cette plateforme")
            }
        }
    }
}

impl RtOutcome {
    /// Vrai si promu.
    pub fn is_promoted(&self) -> bool {
        matches!(self, RtOutcome::Promoted { .. })
    }
}

/// Priorité SCHED_FIFO demandée sur les systèmes POSIX (1–99).
pub const POSIX_PRIORITY: i32 = 70;

/// Tente de promouvoir le fil courant en temps réel.
pub fn promote_current_thread() -> RtOutcome {
    imp::promote()
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{RtOutcome, POSIX_PRIORITY};

    pub fn promote() -> RtOutcome {
        let param = libc::sched_param {
            sched_priority: POSIX_PRIORITY,
        };
        // SAFETY: `pthread_self` est toujours valide ; `param` est initialisé et vit
        // pendant l'appel ; `pthread_setschedparam` ne conserve pas le pointeur.
        let rc =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
        if rc == 0 {
            RtOutcome::Promoted {
                detail: format!("SCHED_FIFO {POSIX_PRIORITY}"),
            }
        } else {
            let err = std::io::Error::from_raw_os_error(rc);
            RtOutcome::Refused {
                reason: format!(
                    "{err} — ajoutez l'utilisateur au groupe audio ou une limite rtprio dans /etc/security/limits.d, ou installez rtkit"
                ),
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{RtOutcome, POSIX_PRIORITY};

    pub fn promote() -> RtOutcome {
        // macOS accepte SCHED_FIFO sans privilège pour les fils ordinaires ; la
        // politique « time constraint » (thread_policy_set) viendra avec le backend
        // CoreAudio, qui fournit déjà ses propres fils temps réel.
        let param = libc::sched_param {
            sched_priority: POSIX_PRIORITY,
        };
        // SAFETY: voir la variante Linux.
        let rc =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
        if rc == 0 {
            RtOutcome::Promoted {
                detail: format!("SCHED_FIFO {POSIX_PRIORITY}"),
            }
        } else {
            RtOutcome::Refused {
                reason: std::io::Error::from_raw_os_error(rc).to_string(),
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::RtOutcome;
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };

    pub fn promote() -> RtOutcome {
        // SAFETY: `GetCurrentThread` renvoie un pseudo-handle toujours valide ;
        // `SetThreadPriority` n'a pas de précondition mémoire.
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
        if ok != 0 {
            RtOutcome::Promoted {
                detail: "TIME_CRITICAL".into(),
            }
        } else {
            RtOutcome::Refused {
                reason: std::io::Error::last_os_error().to_string(),
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::RtOutcome;

    pub fn promote() -> RtOutcome {
        RtOutcome::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_is_attempted_and_never_panics() {
        let outcome = std::thread::spawn(promote_current_thread).join().unwrap();
        // Sans privilège, le refus est la réponse normale ; avec, la promotion.
        match &outcome {
            RtOutcome::Promoted { detail } => assert!(!detail.is_empty()),
            RtOutcome::Refused { reason } => assert!(!reason.is_empty()),
            RtOutcome::Unsupported => {}
        }
        assert!(!outcome.to_string().is_empty());
        let _ = outcome.is_promoted();
    }
}
