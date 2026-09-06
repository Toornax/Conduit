//! Journalisation de débogage du pilote (driver-design.md §7).
//!
//! `kmd_log!` écrit vers le débogueur noyau (fenêtre de `kd`/WinDbg, ou DebugView
//! « Capture Kernel » dans l'invité), préfixé par `conduit_kmd: `. La macro est **vide en
//! release** : aucune chaîne de format ni appel au débogueur ne subsiste dans le binaire
//! livré. WPP n'est pas disponible côté Rust ; M1b évaluera `EtwWrite`.
//!
//! # Pourquoi le niveau « erreur » pour des traces qui n'en sont pas
//!
//! Les traces partent par `DbgPrintEx(DPFLTR_IHVAUDIO_ID, DPFLTR_ERROR_LEVEL, …)` : elles
//! s'annoncent donc comme des *erreurs* alors que la plupart ne sont que de l'avancement.
//! C'est délibéré. Le masque d'un composant est un champ de bits, et **seul
//! `DPFLTR_ERROR_LEVEL` (niveau 0, bit 0) est armé par défaut**, pour tous les composants.
//! Une trace émise à ce niveau arrive **sans configuration** : ni valeur de registre, ni
//! commande du débogueur. Toute autre sévérité exige d'élargir le masque avant de voir
//! quoi que ce soit. C'est ce que font les exemples du WDK, SYSVAD compris.
//!
//! Le motif est une soirée de trois séances de débogage noyau (2026-09-06/07), débogueur
//! série connecté dès l'amorçage à chaque fois, avec l'implémentation précédente
//! (`wdk::println!`, donc `DbgPrint`, donc le composant `DPFLTR_DEFAULT_ID` au niveau
//! `DPFLTR_INFO_LEVEL`, qui n'est pas dans le masque par défaut) :
//!
//! | Séance | Commandes initiales de kd | Registre | Traces du pilote |
//! |---|---|---|---|
//! | 1 | `ed nt!Kd_IHVDRIVER_Mask 0xf` | `DEFAULT` et `IHVAUDIO` à `0xFFFFFFFF` | aucune sur 100 cycles |
//! | 2 | + `ed nt!Kd_DEFAULT_Mask 0xf` | idem | 10 traces, cycle 1 |
//! | 3 | identiques à la séance 2 | idem | aucune sur 100 cycles |
//!
//! La livraison au niveau « information » s'est donc révélée **intermittente à
//! configuration identique**, et sa cause n'a pas été identifiée : la séance 3 interdit de
//! conclure que le masque `DEFAULT` était le facteur. Plutôt que de continuer à chercher,
//! on supprime la dépendance. Une sévérité juste et un journal vide valent moins qu'une
//! trace qui arrive toujours.
//!
//! Composant `DPFLTR_IHVAUDIO_ID` (79) et non `DPFLTR_IHVDRIVER_ID` (77) : Conduit est un
//! pilote **audio** (PortCls/WaveRT), et c'est le composant que le WDK réserve à ce cas.
//! Le choix ne change rien à la livraison — le bit 0 est armé partout — mais il classe nos
//! lignes correctement le jour où l'on élargit un masque pour observer autre chose.
//!
//! # Sûreté
//!
//! Le texte formaté n'est **jamais** passé comme chaîne de format : le format est la
//! constante `c"%s"` et le message n'est qu'un argument. Un `%` dans une trace serait
//! sinon une faille de chaîne de format en mode noyau.
//!
//! Aucune allocation : le crate est `no_std` et `kmd_log!` est appelé jusqu'à
//! `DISPATCH_LEVEL` (`Cable::log_counters`). Le formatage va dans un tampon de **pile** de
//! 512 octets — la limite d'un appel à `DbgPrintEx` — et un message plus long est tronqué
//! sur une frontière de caractère, puis marqué par des points de suspension.
//!
//! IRQL : comme `DbgPrint`, `DbgPrintEx` s'appelle à IRQL ≤ `DIRQL`.

/// Journalise un message de débogage (syntaxe de `format!`), rien en release.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! kmd_log {
    ($($arg:tt)*) => {
        $crate::log::emit(::core::format_args!($($arg)*))
    };
}

/// Journalise un message de débogage (syntaxe de `format!`), rien en release.
///
/// Les arguments sont tout de même vérifiés par le compilateur (`format_args!` dans un
/// bloc jamais évalué) pour que les deux profils compilent le même code appelant.
#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! kmd_log {
    ($($arg:tt)*) => {
        if false {
            let _ = ::core::format_args!($($arg)*);
        }
    };
}

#[cfg(debug_assertions)]
pub(crate) use self::sink::emit;

/// Émission d'une ligne de trace, sans allocation et sans chaîne de format contrôlée par
/// le message. Réservé au profil debug : rien de tout ceci n'existe en release.
#[cfg(debug_assertions)]
mod sink {
    use core::fmt::{self, Write as _};

    use wdk_sys::{
        _DPFLTR_TYPE::DPFLTR_IHVAUDIO_ID, CHAR, DPFLTR_ERROR_LEVEL, ULONG, ntddk::DbgPrintEx,
    };

    /// Préfixe de toutes nos lignes, pour les repérer dans le flot du débogueur.
    const PREFIX: &str = "conduit_kmd: ";

    /// Taille maximale transmise par un appel à `DbgPrintEx`, terminateur compris.
    const MAX_TXN: usize = 512;

    /// Marqueur ajouté au message quand il n'a pas tenu dans le tampon.
    const ELLIPSIS: &[u8] = b"...";

    /// Octets réservés en fin de tampon : le marqueur de troncature, le saut de ligne et
    /// le terminateur nul. Ils doivent tenir **même** pour un message tronqué, d'où leur
    /// retrait de la capacité offerte au texte. (`saturating_*` par discipline : le lint
    /// `arithmetic_side_effects` du workspace refuse `+` et `-` sur les entiers.)
    const TAIL: usize = ELLIPSIS.len().saturating_add(2);

    /// Place laissée au texte formaté, préfixe compris.
    const TEXT_CAPACITY: usize = MAX_TXN.saturating_sub(TAIL);

    /// Composant annoncé à `DbgPrintEx` : `DPFLTR_IHVAUDIO_ID` vaut 79, la conversion vers
    /// `ULONG` est exacte.
    const COMPONENT: ULONG = DPFLTR_IHVAUDIO_ID as ULONG;

    /// Tampon de pile où `core::fmt` écrit le message, avant l'unique appel au débogueur.
    struct StackWriter {
        buffer: [u8; MAX_TXN],
        /// Octets utiles écrits jusqu'ici, toujours ≤ `TEXT_CAPACITY` avant `flush`.
        used: usize,
        /// Vrai dès qu'un fragment n'est pas entré en entier : le tampon n'accepte plus
        /// rien ensuite.
        truncated: bool,
    }

    impl fmt::Write for StackWriter {
        /// Copie ce qui tient encore, en coupant **sur une frontière de caractère**.
        ///
        /// `s` est un `&str`, donc de l'UTF-8 valide, et `is_char_boundary` garantit que
        /// `s[..take]` en est un préfixe complet : la concaténation des fragments copiés
        /// reste de l'UTF-8 valide, jamais un caractère multi-octets coupé en deux.
        /// Passé une troncature, plus rien n'est ajouté : la fin du message est perdue,
        /// jamais recollée à ce qui la suivait.
        fn write_str(&mut self, s: &str) -> fmt::Result {
            if self.truncated {
                return Ok(());
            }

            let room = TEXT_CAPACITY.saturating_sub(self.used);
            let mut take = room.min(s.len());
            // Recule jusqu'à une frontière ; `is_char_boundary(0)` est vrai, donc la
            // boucle s'arrête au pire à 0, et `take <= s.len()` la garde dans les bornes.
            while !s.is_char_boundary(take) {
                take = take.saturating_sub(1);
            }
            if let Some(head) = s.as_bytes().get(..take) {
                self.push(head);
            }

            if take < s.len() {
                self.truncated = true;
            }
            Ok(())
        }
    }

    impl StackWriter {
        /// Tampon vide, prêt à recevoir le préfixe.
        const fn new() -> Self {
            Self {
                buffer: [0; MAX_TXN],
                used: 0,
                truncated: false,
            }
        }

        /// Ajoute des octets déjà bornés par l'appelant ; ne déborde jamais du tampon
        /// (une tranche hors limites rend `None` et l'ajout est simplement abandonné).
        fn push(&mut self, bytes: &[u8]) {
            let end = self.used.saturating_add(bytes.len());
            if let Some(slot) = self.buffer.get_mut(self.used..end) {
                slot.copy_from_slice(bytes);
                self.used = end;
            }
        }

        /// Termine la ligne et l'émet en un seul appel à `DbgPrintEx`.
        fn flush(&mut self) {
            if self.truncated {
                self.push(ELLIPSIS);
            }
            self.push(b"\n");
            // Terminateur nul : `TAIL` a réservé sa place, l'index est donc dans le
            // tampon même sur un message tronqué.
            if let Some(slot) = self.buffer.get_mut(self.used) {
                *slot = 0;
            }

            // SAFETY: `self.buffer` contient une chaîne C valide : les octets utiles sont
            // en [0, self.used), et celui d'indice self.used vient d'être mis à zéro —
            // `TAIL` garantit self.used < MAX_TXN, donc un terminateur dans le tampon. Le
            // format est la constante `c"%s"`, jamais notre texte : `DbgPrintEx`
            // n'interprète aucun `%` du message. Le tampon vit sur la pile jusqu'au retour
            // de `emit`, donc au-delà de l'appel.
            unsafe {
                DbgPrintEx(
                    COMPONENT,
                    DPFLTR_ERROR_LEVEL,
                    c"%s".as_ptr().cast(),
                    self.buffer.as_ptr().cast::<CHAR>(),
                );
            }
        }
    }

    /// Formate et émet une ligne de trace. Point d'entrée de `kmd_log!`.
    ///
    /// Hors ligne à dessein : le tampon de 512 octets n'existe que dans ce cadre de pile,
    /// et non dans celui de chacun des appelants de la macro.
    #[inline(never)]
    pub(crate) fn emit(args: fmt::Arguments<'_>) {
        let mut writer = StackWriter::new();
        // `write_str` ne rend jamais d'erreur : la troncature est un état du tampon, pas
        // un échec, et une trace perdue ne doit de toute façon rien interrompre.
        let _ = writer.write_str(PREFIX);
        let _ = fmt::write(&mut writer, args);
        writer.flush();
    }
}
