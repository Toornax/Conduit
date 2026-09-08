//! Le descripteur de sécurité du canal nommé — le vrai sujet de ce service.
//!
//! Un service `LocalSystem` qui accepte des ordres par un canal nommé **est** une
//! élévation de privilège s'il est mal fermé : tout ce qu'un attaquant a à faire, c'est
//! de s'y connecter. Ce module construit le descripteur explicitement, et le contrôle
//! après construction.
//!
//! # Jamais de DACL nulle
//!
//! `CreateNamedPipeW(…, lpSecurityAttributes = NULL, …)` pose un descripteur par défaut
//! dérivé du jeton du créateur ; passer un `SECURITY_DESCRIPTOR` dont la DACL est
//! **nulle** (`D:NO_ACCESS_CONTROL`, ou un `SetSecurityDescriptorDacl(…, NULL, …)`)
//! accorde en revanche **tout à tout le monde**. C'est le défaut catastrophique
//! classique de cette famille de code, et il ne se voit pas : le canal marche
//! parfaitement, simplement n'importe quel processus de la machine peut donner des
//! ordres au service qui écrit dans le pilote.
//!
//! D'où deux garde-fous, l'un à la compilation, l'autre à l'exécution :
//!
//! 1. le descripteur vient d'un **SDDL écrit à la main** ([`SDDL_TUBE`]), analysé par
//!    des tests de table qui vérifient chaque ACE ;
//! 2. après conversion, [`DescripteurTube::nouveau`] relit la DACL par
//!    `GetSecurityDescriptorDacl` et **refuse de démarrer** si elle est absente ou
//!    nulle. Un service qui ne peut pas prouver que son canal est protégé ne s'ouvre
//!    pas.
//!
//! # Qui a le droit, et pourquoi
//!
//! | Groupe | SID | Droits | Raison |
//! |---|---|---|---|
//! | `SYSTEM` | `SY` (S-1-5-18) | contrôle total | le service lui-même ; il crée les instances du canal |
//! | `Administrateurs` | `BA` (S-1-5-32-544) | contrôle total | ils peuvent déjà devenir `SYSTEM` ; leur refuser l'accès n'ajouterait rien et empêcherait le diagnostic |
//! | `INTERACTIVE` | `IU` (S-1-5-4) | lire et écrire des données, rien de plus | les utilisateurs **réellement connectés** à la machine, ceux dont le démon porte le jeton |
//!
//! `INTERACTIVE` et non `Tout le monde` (`WD`) : `Tout le monde` inclut les ouvertures
//! de session par le réseau, les comptes de service et `ANONYMOUS LOGON` sur les
//! systèmes anciens. `INTERACTIVE` et non `Utilisateurs authentifiés` (`AU`) : un compte
//! de service quelconque est authentifié sans être devant la machine. Le démon
//! `conduitd` démarre par une tâche planifiée **à l'ouverture de session** (ADR-013,
//! M1b-35), donc avec un jeton d'ouverture de session interactive, qui porte ce SID.
//!
//! **Le piège qu'une tâche planifiée « exécuter même si l'utilisateur n'est pas
//! connecté » ferait tomber** : ce mode-là ouvre une session de type `BATCH`, dont le
//! jeton ne porte **pas** `INTERACTIVE`, et le démon se verrait refuser l'ouverture du
//! canal par `ERROR_ACCESS_DENIED`. C'est une conséquence voulue — un démon qui tourne
//! sans utilisateur devant l'écran n'a pas à commander le pilote — mais il faut savoir
//! que c'est ce choix-ci qui la produit, et pas une panne.
//!
//! # Pourquoi le masque du client n'est pas `GRGW`
//!
//! `GENERIC_WRITE` sur un canal nommé s'étend en `FILE_GENERIC_WRITE`, qui contient
//! `FILE_APPEND_DATA` (0x0004) — et sur un canal nommé, ce bit **est**
//! `FILE_CREATE_PIPE_INSTANCE`. Accorder `GW` à `INTERACTIVE` donnerait donc à tout
//! utilisateur connecté le droit de créer de **nouvelles instances** de
//! `\\.\pipe\conduit-helper`, c'est-à-dire de se faire passer pour le service auprès du
//! démon. C'est le squattage de canal nommé, et c'est une élévation de privilège en
//! bonne et due forme.
//!
//! On accorde donc un masque numérique explicite, [`ACCES_CLIENT`], qui vaut exactement
//! ce dont un client a besoin : lire des données, écrire des données, lire les attributs,
//! et `SYNCHRONIZE` (que `CreateFileW` demande toujours pour un handle synchrone).
//! Symétriquement, le client ouvre avec [`ACCES_OUVERTURE_CLIENT`] et **pas**
//! `GENERIC_READ | GENERIC_WRITE` : demander plus que ce qui est accordé ferait échouer
//! l'ouverture.

use core::fmt;

// ---------------------------------------------------------------------------------
// Les masques d'accès (`winnt.h`), et le SDDL.
// ---------------------------------------------------------------------------------

/// `FILE_READ_DATA` (`winnt.h`) : lire des octets du canal.
pub const FILE_READ_DATA: u32 = 0x0000_0001;
/// `FILE_WRITE_DATA` : écrire des octets dans le canal.
pub const FILE_WRITE_DATA: u32 = 0x0000_0002;
/// `FILE_APPEND_DATA`, qui sur un canal nommé **est** `FILE_CREATE_PIPE_INSTANCE`.
///
/// Nommé ici uniquement pour pouvoir affirmer, en test, qu'il n'est **pas** dans
/// [`ACCES_CLIENT`].
pub const FILE_CREATE_PIPE_INSTANCE: u32 = 0x0000_0004;
/// `FILE_READ_ATTRIBUTES`.
pub const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
/// `SYNCHRONIZE` : exigé par tout handle synchrone.
pub const SYNCHRONIZE: u32 = 0x0010_0000;

/// Ce qu'un client a le droit de faire : lire, écrire, lire les attributs, attendre.
///
/// **Et rien d'autre** : ni créer une instance du canal, ni lire ou modifier son
/// descripteur de sécurité.
pub const ACCES_CLIENT: u32 = FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

/// Ce qu'un client **demande** à `CreateFileW`.
///
/// Un sous-ensemble strict de [`ACCES_CLIENT`] : les attributs ne servent à rien à notre
/// client, et `CreateFileW` ajoute `SYNCHRONIZE` de son côté quand le handle n'est pas
/// en recouvrement — on le demande explicitement pour que le masque accordé et le masque
/// demandé se lisent l'un en face de l'autre.
pub const ACCES_OUVERTURE_CLIENT: u32 = FILE_READ_DATA | FILE_WRITE_DATA | SYNCHRONIZE;

// Le masque accordé ne contient pas le droit de créer une instance du canal : c'est la
// propriété qui empêche le squattage, et elle est vérifiée à la **compilation**.
const _: () = assert!(ACCES_CLIENT & FILE_CREATE_PIPE_INSTANCE == 0);
// Et ce que le client demande est bien couvert par ce qu'on accorde.
const _: () = assert!(ACCES_OUVERTURE_CLIENT & !ACCES_CLIENT == 0);
const _: () = assert!(ACCES_CLIENT == 0x0010_0083);

/// Le descripteur de sécurité du canal, en SDDL.
///
/// - `D:P` — une DACL **protégée** : rien n'est hérité, la liste ci-dessous est la
///   totalité de ce qui est accordé ;
/// - `(A;;GA;;;SY)` — `SYSTEM`, contrôle total : c'est le compte du service ;
/// - `(A;;GA;;;BA)` — `Administrateurs`, contrôle total ;
/// - `(A;;0x00100083;;;IU)` — `INTERACTIVE`, [`ACCES_CLIENT`] et rien de plus.
///
/// Ni propriétaire (`O:`) ni groupe (`G:`) : la valeur par défaut est celle du créateur,
/// et l'imposer exigerait `SeRestorePrivilege` — ce qui ferait échouer le mode
/// `console`, qui sert justement au diagnostic sous un compte ordinaire.
///
/// Aucune SACL : l'audit des accès à ce canal se règle par la stratégie du poste, pas
/// par nous, et poser une `S:` exigerait `SeSecurityPrivilege`.
pub const SDDL_TUBE: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100083;;;IU)";

/// Une ACE de [`SDDL_TUBE`], telle que [`aces`] la découpe.
///
/// Les six champs de la forme SDDL `(type;drapeaux;droits;objet;héritage;sid)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ace<'a> {
    /// `A` (autoriser) ou `D` (refuser).
    pub type_ace: &'a str,
    /// Les drapeaux d'héritage — vides dans notre cas.
    pub drapeaux: &'a str,
    /// Les droits, soit une abréviation SDDL (`GA`), soit un masque `0x…`.
    pub droits: &'a str,
    /// Le SID du bénéficiaire, abrégé (`SY`, `BA`, `IU`).
    pub sid: &'a str,
}

/// Découpe les ACE d'un SDDL de DACL.
///
/// Analyseur volontairement minimal — il ne sert qu'à **éprouver notre propre constante**
/// en test, pas à interpréter un SDDL quelconque. Il est pur, donc les tests tournent sur
/// toutes les plateformes : la constante la plus sensible du service se relit depuis
/// Linux.
#[must_use]
pub fn aces(sddl: &str) -> Vec<Ace<'_>> {
    let mut sorties = Vec::new();
    let mut reste = sddl;
    while let Some(debut) = reste.find('(') {
        let apres = reste.get(debut.saturating_add(1)..).unwrap_or("");
        let Some(fin) = apres.find(')') else { break };
        let corps = apres.get(..fin).unwrap_or("");
        let champs: Vec<&str> = corps.split(';').collect();
        if let (Some(type_ace), Some(drapeaux), Some(droits), Some(sid)) =
            (champs.first(), champs.get(1), champs.get(2), champs.get(5))
        {
            sorties.push(Ace {
                type_ace,
                drapeaux,
                droits,
                sid,
            });
        }
        reste = apres.get(fin.saturating_add(1)..).unwrap_or("");
    }
    sorties
}

/// Ce qui a empêché de construire ou de valider le descripteur de sécurité.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurSecurite {
    /// `ConvertStringSecurityDescriptorToSecurityDescriptorW` a refusé le SDDL.
    Conversion {
        /// Le code Win32 rendu, tel quel.
        code: u32,
    },
    /// `GetSecurityDescriptorDacl` a échoué : on ne peut donc pas prouver que la DACL
    /// n'est pas nulle, et on refuse de démarrer.
    LectureDacl {
        /// Le code Win32 rendu, tel quel.
        code: u32,
    },
    /// La DACL est absente ou **nulle** : tout serait accordé à tout le monde.
    ///
    /// Ne devrait jamais arriver avec [`SDDL_TUBE`] ; c'est le filet qui attrape une
    /// modification malheureuse de cette constante.
    DaclNulle,
}

impl fmt::Display for ErreurSecurite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conversion { code } => write!(
                f,
                "SDDL du canal refusé par le système (erreur Win32 {code}) : « {SDDL_TUBE} »"
            ),
            Self::LectureDacl { code } => write!(
                f,
                "GetSecurityDescriptorDacl a échoué (erreur Win32 {code}) : impossible de \
                 prouver que la DACL du canal n'est pas nulle, le service ne démarre pas"
            ),
            Self::DaclNulle => f.write_str(
                "DACL nulle ou absente : le canal serait ouvert à tout le monde, le service \
                 ne démarre pas",
            ),
        }
    }
}

impl std::error::Error for ErreurSecurite {}

#[cfg(windows)]
pub use fenetre::DescripteurTube;

/// La construction du descripteur, propre à Windows.
#[cfg(windows)]
mod fenetre {
    use super::{ErreurSecurite, ACCES_CLIENT, SDDL_TUBE};
    use core::fmt;
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    };

    /// Le descripteur de sécurité du canal, **construit et vérifié**, libéré à la
    /// destruction.
    ///
    /// Il doit vivre aussi longtemps que les appels à `CreateNamedPipeW` qui s'en
    /// servent : le `SECURITY_ATTRIBUTES` que rend [`Self::attributs`] emprunte son
    /// pointeur, et le serveur crée une instance de canal par connexion.
    pub struct DescripteurTube {
        /// Le descripteur auto-relatif rendu par la conversion, à libérer par
        /// `LocalFree`.
        descripteur: PSECURITY_DESCRIPTOR,
    }

    impl fmt::Debug for DescripteurTube {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("DescripteurTube")
                .field("sddl", &SDDL_TUBE)
                .field("acces_client", &format_args!("{ACCES_CLIENT:#010x}"))
                .finish_non_exhaustive()
        }
    }

    // Le descripteur n'est qu'un bloc d'octets appartenant à ce type : rien d'autre ne
    // le lit ni ne l'écrit tant qu'il vit, et le système ne le modifie pas. Le partager
    // entre les fils du serveur (chaque connexion crée l'instance suivante du canal) est
    // donc sûr.
    // SAFETY: propriété exclusive d'un bloc mémoire immuable après construction ;
    // `LocalFree` n'est appelé qu'une fois, dans `Drop`.
    unsafe impl Send for DescripteurTube {}
    // SAFETY: même raison — aucune mutation après construction, donc aucun accès
    // concurrent à protéger.
    unsafe impl Sync for DescripteurTube {}

    impl DescripteurTube {
        /// Convertit [`SDDL_TUBE`] en descripteur, puis **vérifie que sa DACL n'est ni
        /// absente ni nulle**.
        ///
        /// La vérification n'est pas décorative : c'est elle qui transforme « on croit
        /// avoir posé une DACL » en « on l'a constaté ». Une DACL nulle accorde tout à
        /// tout le monde et ne se manifeste par aucun symptôme.
        ///
        /// # Erreurs
        ///
        /// [`ErreurSecurite`] : le SDDL refusé, la DACL illisible, ou la DACL nulle.
        pub fn nouveau() -> Result<Self, ErreurSecurite> {
            let large: Vec<u16> = SDDL_TUBE
                .encode_utf16()
                .chain(core::iter::once(0))
                .collect();
            let mut descripteur = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `large` est une chaîne large terminée par NUL, vivante pendant tout
            // l'appel ; `descripteur` est une variable de cette pile, écrite par l'appelé
            // seul. La taille de sortie ne nous intéresse pas (le descripteur est
            // auto-relatif et se libère par `LocalFree`), d'où `None`.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(large.as_ptr()),
                    SDDL_REVISION_1,
                    &mut descripteur,
                    None,
                )
            }
            .map_err(|_| ErreurSecurite::Conversion {
                // SAFETY: `GetLastError` ne prend aucun paramètre et lit le code du fil
                // courant, posé par l'appel qui vient d'échouer.
                code: unsafe { GetLastError() }.0,
            })?;

            let garde = Self { descripteur };
            garde.verifier_dacl()?;
            Ok(garde)
        }

        /// Relit la DACL et refuse qu'elle soit absente ou nulle.
        fn verifier_dacl(&self) -> Result<(), ErreurSecurite> {
            let mut presente = BOOL::default();
            let mut dacl: *mut ACL = core::ptr::null_mut();
            let mut par_defaut = BOOL::default();
            // SAFETY: `self.descripteur` vient de la conversion et vit tant que `self` ;
            // les trois sorties sont des variables de cette pile, écrites par l'appelé
            // seul.
            unsafe {
                GetSecurityDescriptorDacl(
                    self.descripteur,
                    &mut presente,
                    &mut dacl,
                    &mut par_defaut,
                )
            }
            .map_err(|_| ErreurSecurite::LectureDacl {
                // SAFETY: voir plus haut.
                code: unsafe { GetLastError() }.0,
            })?;
            // Les deux moitiés du refus : « pas de DACL du tout » et « DACL présente mais
            // pointeur nul » sont deux façons de tout accorder à tout le monde, et
            // `GetSecurityDescriptorDacl` les distingue par ce couple.
            if !presente.as_bool() || dacl.is_null() {
                return Err(ErreurSecurite::DaclNulle);
            }
            Ok(())
        }

        /// Les `SECURITY_ATTRIBUTES` à passer à `CreateNamedPipeW`.
        ///
        /// La structure rendue **emprunte** le descripteur : elle ne doit pas survivre à
        /// `self`. `bInheritHandle` est faux — le service ne lance aucun processus
        /// enfant, et un handle de canal hérité par un processus moins privilégié serait
        /// exactement le trou qu'on vient de fermer.
        #[must_use]
        pub fn attributs(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
                lpSecurityDescriptor: self.descripteur.0,
                bInheritHandle: false.into(),
            }
        }
    }

    impl Drop for DescripteurTube {
        fn drop(&mut self) {
            if self.descripteur.0.is_null() {
                return;
            }
            // SAFETY: le bloc vient de
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, qui documente
            // `LocalFree` comme sa libération ; le type n'est ni `Copy` ni clonable, donc
            // l'appel n'a lieu qu'une fois.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.descripteur.0))) };
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Le SDDL accorde exactement trois choses, à exactement trois SID.
    ///
    /// Test **portable** : la constante la plus sensible du service se relit sans
    /// Windows.
    #[test]
    fn le_sddl_accorde_exactement_ce_qui_est_documente() {
        let aces = aces(SDDL_TUBE);
        assert_eq!(aces.len(), 3, "{aces:?}");
        let attendu = [
            ("A", "GA", "SY"),
            ("A", "GA", "BA"),
            ("A", "0x00100083", "IU"),
        ];
        for (ace, (type_ace, droits, sid)) in aces.iter().zip(attendu) {
            assert_eq!(ace.type_ace, type_ace, "{ace:?}");
            assert_eq!(ace.droits, droits, "{ace:?}");
            assert_eq!(ace.sid, sid, "{ace:?}");
            // Aucun drapeau d'héritage : un canal nommé n'a pas de conteneur parent, et
            // un `CI`/`OI` égaré ici ne voudrait rien dire.
            assert!(ace.drapeaux.is_empty(), "{ace:?}");
        }
        // Aucune ACE de refus : une DACL faite d'ACE d'autorisation seulement se lit
        // sans avoir à raisonner sur l'ordre des entrées.
        assert!(aces.iter().all(|ace| ace.type_ace == "A"), "{aces:?}");
    }

    /// La DACL est protégée, et jamais nulle — les deux formes que prendrait le défaut.
    #[test]
    fn la_dacl_est_protegee_et_jamais_nulle() {
        assert!(SDDL_TUBE.starts_with("D:P"), "{SDDL_TUBE}");
        // `NO_ACCESS_CONTROL` est l'écriture SDDL d'une DACL nulle : tout à tout le
        // monde. C'est très exactement ce qu'on ne veut pas.
        assert!(!SDDL_TUBE.contains("NO_ACCESS_CONTROL"), "{SDDL_TUBE}");
        // Ni `Tout le monde` (WD), ni `Utilisateurs authentifiés` (AU), ni
        // `ANONYMOUS LOGON` (AN) : voir l'en-tête de module.
        for interdit in ["WD", "AU", "AN", "WR"] {
            assert!(
                !aces(SDDL_TUBE).iter().any(|ace| ace.sid == interdit),
                "le SID {interdit} n'a rien à faire sur ce canal : {SDDL_TUBE}"
            );
        }
        // Aucune SACL : elle exigerait `SeSecurityPrivilege`.
        assert!(!SDDL_TUBE.contains("S:"), "{SDDL_TUBE}");
    }

    /// Le masque du client ne porte **pas** le droit de créer une instance du canal.
    ///
    /// C'est la propriété qui empêche le squattage de `\\.\pipe\conduit-helper`, et
    /// c'est la raison pour laquelle l'ACE d'`INTERACTIVE` est un masque numérique et
    /// non `GRGW`.
    #[test]
    fn le_client_ne_peut_pas_creer_d_instance_du_canal() {
        assert_eq!(ACCES_CLIENT & FILE_CREATE_PIPE_INSTANCE, 0);
        assert_eq!(ACCES_CLIENT, 0x0010_0083);
        // Le masque du SDDL est bien celui de la constante : deux écritures, une seule
        // valeur.
        let ace_client = aces(SDDL_TUBE)
            .into_iter()
            .find(|ace| ace.sid == "IU")
            .expect("l'ACE d'INTERACTIVE");
        let masque = u32::from_str_radix(ace_client.droits.trim_start_matches("0x"), 16)
            .expect("masque hexadécimal");
        assert_eq!(masque, ACCES_CLIENT);

        // `GENERIC_WRITE` étendu contiendrait le bit interdit : c'est bien pour cela
        // qu'on ne l'écrit pas.
        const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
        assert_ne!(FILE_GENERIC_WRITE & FILE_CREATE_PIPE_INSTANCE, 0);
    }

    /// Ce que le client demande est couvert par ce qu'on lui accorde.
    ///
    /// L'inverse — demander plus que ce qui est accordé — ferait échouer `CreateFileW`
    /// avec `ERROR_ACCESS_DENIED`, et le symptôme ressemblerait à un service absent.
    #[test]
    fn le_client_demande_moins_qu_on_ne_lui_accorde() {
        assert_eq!(ACCES_OUVERTURE_CLIENT & !ACCES_CLIENT, 0);
        assert_eq!(ACCES_OUVERTURE_CLIENT, 0x0010_0003);
        // `SYNCHRONIZE` est indispensable : `CreateFileW` le demande pour tout handle
        // qui n'est pas en recouvrement.
        assert_ne!(ACCES_OUVERTURE_CLIENT & SYNCHRONIZE, 0);
        assert_ne!(ACCES_CLIENT & SYNCHRONIZE, 0);
    }

    /// Le découpeur d'ACE fait ce qu'on lui demande, y compris sur des entrées bancales.
    #[test]
    fn le_decoupeur_d_ace_table() {
        assert!(aces("").is_empty());
        assert!(aces("D:P").is_empty());
        // Parenthèse jamais fermée : rien plutôt qu'une lecture hors du texte.
        assert!(aces("D:P(A;;GA;;;SY").is_empty());
        // Champs manquants : l'ACE est ignorée plutôt que devinée.
        assert!(aces("D:P(A;;GA)").is_empty());
        let une = aces("D:P(A;;GA;;;SY)");
        assert_eq!(une.len(), 1);
        assert_eq!(une[0].sid, "SY");
        assert_eq!(une[0].droits, "GA");
        // Deux ACE se lisent dans l'ordre du texte.
        let deux = aces("D:P(D;;FA;;;WD)(A;OICI;GR;;;BA)");
        assert_eq!(deux.len(), 2);
        assert_eq!(deux[0].type_ace, "D");
        assert_eq!(deux[0].sid, "WD");
        assert_eq!(deux[1].drapeaux, "OICI");
        assert_eq!(deux[1].sid, "BA");
    }

    /// Sur Windows, le SDDL est réellement accepté par le système et la DACL constatée
    /// non nulle.
    ///
    /// Ce test **n'installe rien** et n'ouvre aucun canal : il ne fait que convertir une
    /// chaîne et relire le résultat. Il tourne sans élévation.
    #[cfg(windows)]
    #[test]
    fn le_systeme_accepte_le_sddl_et_la_dacl_n_est_pas_nulle() {
        let descripteur = DescripteurTube::nouveau().expect("le SDDL du canal est valide");
        let attributs = descripteur.attributs();
        assert_eq!(
            attributs.nLength as usize,
            size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>()
        );
        assert!(!attributs.lpSecurityDescriptor.is_null());
        assert!(!attributs.bInheritHandle.as_bool());
    }
}
