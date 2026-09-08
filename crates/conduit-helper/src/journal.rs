//! Le journal du service : **qui** a demandé **quoi**, et ce que ça a donné.
//!
//! Un service `LocalSystem` qui modifie la configuration de la machine sur ordre d'un
//! processus utilisateur doit laisser une trace nominative. Le critère est simple, et
//! c'est celui qu'on vérifie : après coup, le journal doit permettre de dire **qui a
//! activé un câble**.
//!
//! # Ce qui est journalisé, et à quel niveau
//!
//! - tout ordre qui **modifie** l'état ([`crate::protocole::Requete::modifie`]) est
//!   journalisé en `INFO` avec l'identité de l'appelant, avant et après ;
//! - les refus le sont aussi, avec leur cause : un refus répété est le signe qu'on
//!   cherche, et un journal qui ne garde que les succès ne montre pas les tentatives ;
//! - les lectures (`version`, `lister`) sont en `DEBUG` : elles n'engagent rien et
//!   noieraient le reste.
//!
//! # Le formatage est pur, l'horloge est à part
//!
//! [`ligne`] ne fait que mettre en forme ; l'heure lui est **donnée**. C'est ce qui
//! permet de la vérifier en table de cas sur n'importe quelle plateforme, y compris le
//! cas qui compte : un horodatage à un chiffre doit sortir sur deux.

use core::fmt;
use std::io::Write;
use std::sync::Mutex;

/// Le niveau d'une ligne de journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Niveau {
    /// Détail, pour le diagnostic : les ordres qui ne modifient rien.
    Debug,
    /// Marche normale : un ordre qui a modifié l'état, le démarrage, l'arrêt.
    Info,
    /// Quelque chose de contraire à ce qu'on attend, sans empêcher de servir.
    Alerte,
    /// Le service n'a pas pu faire ce qu'on lui demandait.
    Erreur,
}

impl Niveau {
    /// L'étiquette de niveau, cadrée sur six colonnes pour que les lignes s'alignent.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG ",
            Self::Info => "INFO  ",
            Self::Alerte => "ALERTE",
            Self::Erreur => "ERREUR",
        }
    }
}

impl fmt::Display for Niveau {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label().trim_end())
    }
}

/// Un instant, découpé comme `GetLocalTime` le rend.
///
/// Une structure et non un `SystemTime` : le journal d'un service Windows s'horodate à
/// l'**heure locale**, celle de l'administrateur qui le relit à côté de l'Observateur
/// d'événements, et la conversion depuis un instant absolu demanderait une bibliothèque
/// de fuseaux dont ce crate n'a pas besoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Horodatage {
    /// Année.
    pub annee: u16,
    /// Mois, de 1 à 12.
    pub mois: u16,
    /// Jour, de 1 à 31.
    pub jour: u16,
    /// Heure, de 0 à 23.
    pub heure: u16,
    /// Minute.
    pub minute: u16,
    /// Seconde.
    pub seconde: u16,
    /// Milliseconde.
    pub milli: u16,
}

impl Horodatage {
    /// L'instant zéro, pour les tests et le repli hors Windows.
    pub const ZERO: Self = Self {
        annee: 0,
        mois: 1,
        jour: 1,
        heure: 0,
        minute: 0,
        seconde: 0,
        milli: 0,
    };

    /// L'heure locale, maintenant.
    ///
    /// Hors Windows — où ce service ne tourne pas — rend [`Self::ZERO`] : le module
    /// reste compilable et testable partout, ce qui est le point.
    #[must_use]
    pub fn maintenant() -> Self {
        #[cfg(windows)]
        {
            // SAFETY: `GetLocalTime` ne prend aucun paramètre et rend une `SYSTEMTIME`
            // par valeur ; rien n'est emprunté, alloué ni partagé.
            let systeme = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            Self {
                annee: systeme.wYear,
                mois: systeme.wMonth,
                jour: systeme.wDay,
                heure: systeme.wHour,
                minute: systeme.wMinute,
                seconde: systeme.wSecond,
                milli: systeme.wMilliseconds,
            }
        }
        #[cfg(not(windows))]
        {
            Self::ZERO
        }
    }
}

impl fmt::Display for Horodatage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            self.annee, self.mois, self.jour, self.heure, self.minute, self.seconde, self.milli
        )
    }
}

/// L'identité du processus qui a envoyé l'ordre.
///
/// Trois renseignements, parce qu'aucun ne suffit seul : le **SID** est l'identité que
/// Windows connaît et la seule qui ne change pas ; le **nom** est celui qu'un
/// administrateur reconnaît ; le **PID** distingue deux ordres du même utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appelant {
    /// `DOMAINE\utilisateur`, ou `?` quand la résolution du nom a échoué.
    pub nom: String,
    /// Le SID sous sa forme textuelle (`S-1-5-21-…`), ou `?`.
    pub sid: String,
    /// L'identifiant du processus client.
    pub pid: u32,
}

impl Appelant {
    /// L'appelant qu'on n'a pas su identifier.
    ///
    /// Un ordre dont on ne connaît pas l'auteur reste servi — le contrôle d'accès est
    /// fait par le descripteur du canal, pas par cette lecture — mais la ligne le dit,
    /// plutôt que d'inventer un nom.
    #[must_use]
    pub fn inconnu(pid: u32) -> Self {
        Self {
            nom: "?".to_owned(),
            sid: "?".to_owned(),
            pid,
        }
    }
}

impl fmt::Display for Appelant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}) pid {}", self.nom, self.sid, self.pid)
    }
}

/// Met en forme une ligne de journal. **Pure** : l'heure est un paramètre.
///
/// Le format est `AAAA-MM-JJ hh:mm:ss.mmm  NIVEAU  message` — trié par le temps quand on
/// trie le fichier, et lisible par `Get-Content -Wait`.
#[must_use]
pub fn ligne(quand: Horodatage, niveau: Niveau, message: &str) -> String {
    format!("{quand}  {}  {message}", niveau.label())
}

/// Où le journal écrit.
///
/// Deux destinations et pas une : un service n'a pas de console, donc il lui faut un
/// fichier ; un binaire lancé en mode `console` pour le diagnostic n'a pas envie
/// d'aller lire un fichier. Le mode `console` écrit donc aux deux.
enum Sortie {
    /// La sortie d'erreur du processus.
    Erreur,
    /// Un fichier ouvert en ajout.
    Fichier(std::fs::File),
    /// Les deux.
    Deux(std::fs::File),
}

/// Le journal du service.
///
/// Un verrou et une destination. Le verrou est ce qui garantit qu'une ligne écrite par
/// le fil d'une connexion ne s'entrelace pas avec celle d'une autre : le serveur sert
/// plusieurs clients de front, et un journal illisible ne prouve rien de qui a fait
/// quoi.
pub struct Journal {
    sortie: Mutex<Sortie>,
    /// En dessous de ce niveau, rien n'est écrit.
    plancher: Niveau,
}

impl fmt::Debug for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Journal")
            .field("plancher", &self.plancher)
            .finish_non_exhaustive()
    }
}

impl Journal {
    /// Un journal qui n'écrit que sur la sortie d'erreur.
    #[must_use]
    pub fn vers_stderr(plancher: Niveau) -> Self {
        Self {
            sortie: Mutex::new(Sortie::Erreur),
            plancher,
        }
    }

    /// Un journal qui écrit dans `chemin`, en **ajout**, et éventuellement aussi sur la
    /// sortie d'erreur.
    ///
    /// Le répertoire parent est créé si besoin. Un échec d'ouverture n'est pas fatal du
    /// point de vue de l'appelant — c'est à lui de décider — mais il est rendu, parce
    /// qu'un service qui ne journalise pas ne remplit pas la moitié de son cahier des
    /// charges.
    ///
    /// # Erreurs
    ///
    /// L'erreur d'entrée-sortie de la création du répertoire ou de l'ouverture.
    pub fn vers_fichier(
        chemin: &std::path::Path,
        plancher: Niveau,
        aussi_stderr: bool,
    ) -> std::io::Result<Self> {
        if let Some(parent) = chemin.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let fichier = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(chemin)?;
        Ok(Self {
            sortie: Mutex::new(if aussi_stderr {
                Sortie::Deux(fichier)
            } else {
                Sortie::Fichier(fichier)
            }),
            plancher,
        })
    }

    /// Écrit une ligne, si son niveau atteint le plancher.
    ///
    /// **N'échoue jamais visiblement** : un journal qui ferait remonter ses erreurs
    /// d'écriture obligerait chaque appelant à décider quoi en faire, au milieu du
    /// service d'un ordre. Un disque plein ne doit pas empêcher de déconnecter un câble.
    pub fn ecrire(&self, niveau: Niveau, message: &str) {
        if niveau < self.plancher {
            return;
        }
        let texte = ligne(Horodatage::maintenant(), niveau, message);
        let Ok(mut sortie) = self.sortie.lock() else {
            // Verrou empoisonné : un fil a paniqué en tenant le journal. On ne peut plus
            // rien garantir de l'ordre des lignes, mais perdre le journal serait pire.
            eprintln!("{texte}");
            return;
        };
        match &mut *sortie {
            Sortie::Erreur => {
                eprintln!("{texte}");
            }
            Sortie::Fichier(fichier) => {
                let _ = writeln!(fichier, "{texte}");
                let _ = fichier.flush();
            }
            Sortie::Deux(fichier) => {
                eprintln!("{texte}");
                let _ = writeln!(fichier, "{texte}");
                let _ = fichier.flush();
            }
        }
    }

    /// Raccourci : niveau [`Niveau::Debug`].
    pub fn debug(&self, message: &str) {
        self.ecrire(Niveau::Debug, message);
    }

    /// Raccourci : niveau [`Niveau::Info`].
    pub fn info(&self, message: &str) {
        self.ecrire(Niveau::Info, message);
    }

    /// Raccourci : niveau [`Niveau::Alerte`].
    pub fn alerte(&self, message: &str) {
        self.ecrire(Niveau::Alerte, message);
    }

    /// Raccourci : niveau [`Niveau::Erreur`].
    pub fn erreur(&self, message: &str) {
        self.ecrire(Niveau::Erreur, message);
    }
}

/// Le chemin du journal du service, quand aucun n'est imposé.
///
/// `%ProgramData%\Conduit\conduit-helper.log` : un emplacement lisible par tout
/// administrateur et écrit par `LocalSystem`, hors de tout profil utilisateur — le
/// service n'en a pas. Le repli sur le répertoire courant n'arrive que si
/// `%ProgramData%` n'est pas défini, ce qui n'est pas un Windows normal.
#[must_use]
pub fn chemin_par_defaut() -> std::path::PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("Conduit").join("conduit-helper.log")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Table du formatage d'une ligne : c'est la forme qu'un administrateur lira.
    #[test]
    fn ligne_table() {
        let quand = Horodatage {
            annee: 2026,
            mois: 9,
            jour: 8,
            heure: 14,
            minute: 3,
            seconde: 11,
            milli: 7,
        };
        let texte = ligne(quand, Niveau::Info, "câble 3 activé");
        // Les champs à un chiffre sortent sur deux (trois pour les millisecondes) :
        // c'est ce qui rend le fichier triable et les colonnes alignées.
        assert_eq!(texte, "2026-09-08 14:03:11.007  INFO    câble 3 activé");
        assert!(texte.starts_with("2026-09-08 14:03:11.007"));

        // Les quatre niveaux tiennent sur six colonnes, donc le message commence
        // toujours au même endroit.
        let debuts: Vec<usize> = [Niveau::Debug, Niveau::Info, Niveau::Alerte, Niveau::Erreur]
            .into_iter()
            .map(|n| ligne(quand, n, "x").find('x').expect("le message"))
            .collect();
        assert!(
            debuts.windows(2).all(|p| p[0] == p[1]),
            "colonnes désalignées : {debuts:?}"
        );

        // L'instant zéro se formate aussi, sans panique ni champ vide.
        assert_eq!(
            ligne(Horodatage::ZERO, Niveau::Debug, ""),
            "0000-01-01 00:00:00.000  DEBUG   "
        );
    }

    /// Les niveaux sont ordonnés, et c'est cet ordre qui fait le filtre.
    #[test]
    fn les_niveaux_sont_ordonnes() {
        assert!(Niveau::Debug < Niveau::Info);
        assert!(Niveau::Info < Niveau::Alerte);
        assert!(Niveau::Alerte < Niveau::Erreur);
        assert_eq!(Niveau::Info.to_string(), "INFO");
        assert_eq!(Niveau::Alerte.to_string(), "ALERTE");
    }

    /// L'identité d'un appelant se lit d'un coup d'œil, et l'inconnu se dit.
    #[test]
    fn l_appelant_se_lit() {
        let connu = Appelant {
            nom: "POSTE\\nathan".to_owned(),
            sid: "S-1-5-21-1-2-3-1001".to_owned(),
            pid: 4812,
        };
        let texte = connu.to_string();
        assert!(texte.contains("POSTE\\nathan"), "{texte}");
        assert!(texte.contains("S-1-5-21-1-2-3-1001"), "{texte}");
        assert!(texte.contains("4812"), "{texte}");

        // Un appelant non identifié le dit plutôt que d'inventer.
        let inconnu = Appelant::inconnu(7);
        assert_eq!(inconnu.nom, "?");
        assert_eq!(inconnu.sid, "?");
        assert!(inconnu.to_string().contains("pid 7"));
    }

    /// Le plancher filtre bien, et un journal vers un fichier écrit ce qu'on lui donne.
    ///
    /// Le fichier est un temporaire de ce test : rien n'est installé sur la machine.
    #[test]
    fn le_plancher_filtre_et_le_fichier_recoit() {
        let repertoire = std::env::temp_dir().join(format!(
            "conduit-helper-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let chemin = repertoire.join("journal.log");
        let journal = Journal::vers_fichier(&chemin, Niveau::Info, false).expect("journal");
        journal.debug("invisible");
        journal.info("visible");
        journal.erreur("visible aussi");
        drop(journal);

        let contenu = std::fs::read_to_string(&chemin).expect("relecture");
        assert!(!contenu.contains("invisible"), "{contenu}");
        assert!(contenu.contains("visible"), "{contenu}");
        assert!(contenu.contains("visible aussi"), "{contenu}");
        assert_eq!(contenu.lines().count(), 2, "{contenu}");
        // Chaque ligne porte son niveau.
        assert!(contenu.contains("INFO  "), "{contenu}");
        assert!(contenu.contains("ERREUR"), "{contenu}");

        let _ = std::fs::remove_dir_all(&repertoire);
    }

    /// Le chemin par défaut est sous `Conduit`, et porte le nom du service.
    #[test]
    fn le_chemin_par_defaut_est_sous_conduit() {
        let chemin = chemin_par_defaut();
        assert!(chemin.ends_with("conduit-helper.log"), "{chemin:?}");
        assert!(
            chemin
                .components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("Conduit")),
            "{chemin:?}"
        );
    }

    /// L'heure locale rendue par le système est plausible.
    ///
    /// Un contrôle de forme, pas de valeur : on ne compare pas à une horloge de
    /// référence, on vérifie que les champs sont dans leurs bornes — ce qui attrape un
    /// champ recopié au mauvais endroit depuis la `SYSTEMTIME`.
    #[cfg(windows)]
    #[test]
    fn l_heure_locale_est_plausible() {
        let quand = Horodatage::maintenant();
        assert!(quand.annee >= 2024, "{quand:?}");
        assert!((1..=12).contains(&quand.mois), "{quand:?}");
        assert!((1..=31).contains(&quand.jour), "{quand:?}");
        assert!(quand.heure <= 23, "{quand:?}");
        assert!(quand.minute <= 59, "{quand:?}");
        // 60 est possible : Windows rend les secondes intercalaires quand elles sont
        // activées.
        assert!(quand.seconde <= 60, "{quand:?}");
        assert!(quand.milli <= 999, "{quand:?}");
    }
}
