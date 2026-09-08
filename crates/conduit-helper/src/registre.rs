//! Le nom d'un endpoint audio dans le registre : le renommage de M1b-21, **sans le
//! pilote**.
//!
//! # Le principe qui commande ce module
//!
//! Le pilote noyau doit rester le plus léger possible ; tout ce qui peut être fait en
//! espace utilisateur y est fait. Renommer un endpoint en est l'illustration : rien de
//! ce fichier ne descend dans `drivers/windows`, et un défaut ici redémarre un service
//! au lieu d'écrire un écran bleu.
//!
//! # La valeur qu'on écrit, et ce qui l'établit
//!
//! Le nom affiché d'un endpoint vit dans
//!
//! ```text
//! HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\{Render|Capture}
//!     \{id-endpoint}\Properties
//! ```
//!
//! dans des valeurs nommées `{GUID},pid` — la représentation textuelle d'un
//! `PROPERTYKEY`. Trois d'entre elles portent un nom, et il a fallu établir laquelle
//! est *le* nom :
//!
//! | Valeur | `PROPERTYKEY` | Contenu, mesuré |
//! |---|---|---|
//! | `{a45c254e-df1c-4efd-8020-67d146a850e0},2` | `PKEY_Device_DeviceDesc` | `Conduit 1` |
//! | `{b3f8fa53-0004-438e-9003-51a46e139bfc},6` | privé au moteur audio | `Conduit — câbles audio virtuels` (le **périphérique**, pas l'endpoint) |
//! | `{a45c254e-df1c-4efd-8020-67d146a850e0},14` | `PKEY_Device_FriendlyName` | **absent du registre** |
//!
//! Les deux `PROPERTYKEY` sont ceux de `Functiondiscoverykeys_devpkey.h`, tels que le
//! crate `windows` les définit (`PKEY_Device_DeviceDesc` = fmtid `a45c254e…`, pid 2 ;
//! `PKEY_Device_FriendlyName` = **le même fmtid**, pid 14 — pas `b3f8fa53…`, qui est un
//! espace privé du moteur audio, non documenté).
//!
//! La documentation Microsoft tranche la question dans « Friendly Names for Audio
//! Endpoint Devices » (WDK, section « audio ») : le magasin de propriétés d'un endpoint
//! prend sa valeur **initiale** de `PKEY_Device_DeviceDesc` dans le nom associé à la
//! catégorie de broche KS, les applications ne peuvent pas l'écrire, « *however, users
//! can modify the name by using the Windows multimedia control panel, Mmsys.cpl* » — et
//! la page décrit ensuite pas à pas le renommage par `mmsys.cpl`, en concluant que « les
//! étapes précédentes changent le nom convivial rangé dans le magasin de propriétés de
//! l'endpoint ». C'est donc **`PKEY_Device_DeviceDesc` qui porte le nom personnalisé** :
//! il n'existe pas de valeur « nom personnalisé » séparée.
//!
//! Le relevé du registre le confirme : sur les 39 endpoints de la machine de
//! développement, `{a45c254e…},2` est présent sur **tous**, `{a45c254e…},14` sur
//! **aucun**. `PKEY_Device_FriendlyName` n'est donc pas rangé mais **composé** à la
//! volée — « `<DeviceDesc>` (`<nom du périphérique>`) », ce que la machine virtuelle
//! avait déjà montré avec `Conduit 1 (Conduit — câbles audio virtuels)`.
//!
//! **On écrit donc `{a45c254e-df1c-4efd-8020-67d146a850e0},2`, en `REG_SZ`, et rien
//! d'autre.** Écrire aussi `,14` reviendrait à figer une composition que Windows refait
//! seul, c'est-à-dire à recréer le défaut que `devices::cable_id_from_endpoint` a déjà
//! corrigé une fois.
//!
//! # La marque : comment on retrouve un câble qu'on vient de renommer
//!
//! Le pont entre un numéro de câble et ses clés MMDevices est la **description** —
//! `Conduit 1`, égalité exacte, jamais un préfixe (« Conduit 1 » attraperait « Conduit
//! 10 » à « Conduit 16 »), jamais le nom composé. Mais renommer *écrase* cette
//! description : après un `renommer 3 Musique`, plus rien dans la clé ne dit que cet
//! endpoint est le câble 3. Un second renommage ne le retrouverait pas, le retour au nom
//! d'origine non plus, et `DeviceInfo::cable` redeviendrait `None`.
//!
//! Le module écrit donc, **avant** de toucher à la description, une valeur qui lui
//! appartient et qui dit ce que la description disait :
//!
//! ```text
//! {3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b},1 = "Conduit 3"
//! ```
//!
//! Le fmtid est [`conduit_kmd_core::config::KSPROPSETID_CONDUIT`], le GUID **gravé** de
//! Conduit — pas un second GUID inventé pour l'occasion — dans un espace de `pid` qui
//! nous est propre. La marque vit dans la clé de l'endpoint : si Windows supprime
//! l'endpoint, elle disparaît avec lui, et il n'y a pas d'arborescence orpheline à
//! nettoyer.
//!
//! L'ordre des écritures n'est pas indifférent. À l'aller : la marque **puis** la
//! description ; au retour : la description **puis** la suppression de la marque. Dans
//! les deux sens, une interruption entre les deux écritures laisse un endpoint qui est
//! encore retrouvable — jamais un endpoint anonyme.
//!
//! # F-52 : le renommage ne survit pas à la désinstallation, parce qu'on l'efface
//!
//! **Décision, écrite ici parce qu'elle est contre-intuitive.** Les clés MMDevices ne
//! sont pas des `HKR` du périphérique : Windows les garde après le retrait du pilote,
//! sous « Périphériques déconnectés », avec tout ce qu'on y a écrit. Un renommage y
//! survivrait donc **par défaut**, et F-52 (désinstallation propre) demande le
//! contraire.
//!
//! D'où [`Requete::NomDefaut`](crate::protocole::Requete::NomDefaut), exposé partout :
//! `conduit-helper nom-defaut <câble>`, et `CableControl::rename` d'un câble vers son
//! nom d'origine `Conduit N`. Le désinstallateur le fera pour les seize câbles ; en
//! attendant, la commande existe et un utilisateur peut défaire ce qu'il a fait.
//!
//! # Le rafraîchissement : ce qui est établi, et ce qui ne l'est pas
//!
//! **Établi** : le magasin de propriétés est persistant et le nom initial n'est copié
//! de la catégorie de broche KS qu'à la **création** de l'endpoint (« copies its
//! *initial* value », WDK) — un renommage n'est donc pas réécrasé à chaque
//! énumération, et il survit au redémarrage. C'est pour cette raison qu'on peut écrire
//! le registre et s'arrêter là.
//!
//! **Non établi** : combien de temps le moteur audio garde en cache le nom qu'il a déjà
//! lu, et s'il existe une notification qui le lui fasse relire. La documentation
//! n'expose que `IMMNotificationClient::OnPropertyValueChanged`, que le système
//! **émet** ; aucune API documentée ne permet à un tiers de le déclencher. Rien n'est
//! donc tenté ici : le module écrit, et le rafraîchissement reste manuel.
//!
//! Si la mesure en machine virtuelle montre qu'un rafraîchissement de la page Son ne
//! suffit pas, le recours n'exige toujours aucune ligne de pilote : **déconnecter puis
//! reconnecter le câble** (`conduit-helper desactiver N` puis `activer N`) détruit et
//! republie les deux endpoints, et Windows relit alors la clé. Ce n'est pas fait
//! automatiquement — un renommage qui couperait le son d'une application en cours de
//! lecture serait une surprise, et `set_channels` est déjà le seul ordre du service à
//! s'autoriser un court silence.
//!
//! # Ce que ce module ne fait pas
//!
//! Il n'ouvre aucun flux audio, n'émet aucun son, ne charge aucun pilote et n'arme aucun
//! privilège : écrire sous `HKLM` est un droit que `LocalSystem` détient, pas un
//! privilège à armer. C'est aussi pourquoi
//! [`Requete::touche_le_pilote`](crate::protocole::Requete::touche_le_pilote) rend
//! `false` pour les deux ordres de M1b-21.
//!
//! # Ce qui est pur et ce qui ne l'est pas
//!
//! Les chemins, les noms de valeurs, et la correspondance câble ↔ clés
//! ([`cable_designe`]) sont **purs**, compilés partout et vérifiés en table de cas. Seul
//! le module `windows` privé touche au registre, et aucun test ne l'exerce : la
//! contrainte du dépôt interdit d'écrire dans `HKLM` depuis `cargo test`, et
//! `tests/registre.rs` se limite à des lectures.

use conduit_backend::CableId;
use conduit_kmd_core::config::{ConfigGuid, KSPROPSETID_CONDUIT};

use crate::controle::{cable_nomme, nom, Cote};

// ---------------------------------------------------------------------------------
// Les chemins et les noms de valeurs.
// ---------------------------------------------------------------------------------

/// La clé, sous `HKEY_LOCAL_MACHINE`, où le moteur audio range ses endpoints.
///
/// Sans `HKLM\` en tête : c'est une **sous-clé**, et c'est l'appelant qui choisit la
/// ruche — ce qui rend le chemin vérifiable sans registre.
pub const RACINE_MMDEVICES: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio";

/// La sous-clé qui contient les propriétés d'un endpoint.
pub const SOUS_CLE_PROPRIETES: &str = "Properties";

/// Le `fmtid` de `PKEY_Device_DeviceDesc` et de `PKEY_Device_FriendlyName`.
///
/// **Le même pour les deux** : c'est le pid qui les distingue (2 et 14). Le confondre
/// avec `{b3f8fa53-…}` — l'espace privé du moteur audio — ferait écrire dans une valeur
/// qui décrit le *périphérique* et que plusieurs endpoints se partagent.
pub const FMTID_DEVICE: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0}";

/// Le `pid` de `PKEY_Device_DeviceDesc` : le nom de l'endpoint, celui qu'on écrit.
pub const PID_DEVICE_DESC: u32 = 2;

/// Le `pid` de `PKEY_Device_FriendlyName` : le nom **composé**, qu'on n'écrit jamais.
///
/// Présent pour que la constante existe là où elle se lit, et pour que le test qui dit
/// « on n'écrit pas celle-là » puisse la nommer.
pub const PID_DEVICE_FRIENDLY_NAME: u32 = 14;

/// Le `pid` de la marque de Conduit dans le `fmtid` de Conduit.
///
/// 1 et non 0 : un `pid` de 0 est réservé par le système de propriétés Windows.
pub const PID_MARQUE: u32 = 1;

/// Le nom d'une valeur de magasin de propriétés : `{fmtid},pid`.
///
/// C'est la seule forme que le moteur audio range dans le registre, et c'est ce que
/// `regedit` montre.
#[must_use]
pub fn nom_de_valeur(fmtid: &str, pid: u32) -> String {
    format!("{fmtid},{pid}")
}

/// Le `fmtid` de Conduit, en texte, engendré depuis le GUID **gravé** du contrat KS.
///
/// Formaté et non recopié : le GUID de Conduit vit dans `conduit-kmd-core`, partagé avec
/// le pilote, et une seconde écriture en dur finirait par diverger.
#[must_use]
pub fn fmtid_conduit() -> String {
    guid_en_texte(&KSPROPSETID_CONDUIT)
}

/// Formate un [`ConfigGuid`] comme le registre le fait : minuscules, entre accolades.
fn guid_en_texte(guid: &ConfigGuid) -> String {
    let [a, b, c, d, e, f, g, h] = guid.data4;
    format!(
        "{{{:08x}-{:04x}-{:04x}-{a:02x}{b:02x}-{c:02x}{d:02x}{e:02x}{f:02x}{g:02x}{h:02x}}}",
        guid.data1, guid.data2, guid.data3
    )
}

/// Le nom de la valeur qui porte la **description** d'un endpoint — celle qu'on écrit.
#[must_use]
pub fn valeur_description() -> String {
    nom_de_valeur(FMTID_DEVICE, PID_DEVICE_DESC)
}

/// Le nom de la valeur qui porte la **marque** de Conduit. Voir l'en-tête de module.
#[must_use]
pub fn valeur_marque() -> String {
    nom_de_valeur(&fmtid_conduit(), PID_MARQUE)
}

/// La sous-clé du flux, telle que le moteur audio la nomme.
///
/// Les deux mots sont ceux de `EDataFlow` : `eRender` et `eCapture`. Ils ne sont pas
/// traduits — c'est un chemin de registre, pas un message.
#[must_use]
pub const fn sous_cle_flux(cote: Cote) -> &'static str {
    match cote {
        Cote::Rendu => "Render",
        Cote::Capture => "Capture",
    }
}

/// La clé qui contient tous les endpoints d'un côté.
#[must_use]
pub fn chemin_flux(cote: Cote) -> String {
    format!("{RACINE_MMDEVICES}\\{}", sous_cle_flux(cote))
}

/// La clé `Properties` de l'endpoint `endpoint` du côté `cote`.
///
/// `endpoint` est le nom de sous-clé tel que l'énumération l'a rendu — un GUID entre
/// accolades — et n'est **pas** analysé : le dépôt traite les identifiants d'endpoint
/// comme opaques, comme la documentation Microsoft le demande.
#[must_use]
pub fn chemin_proprietes(cote: Cote, endpoint: &str) -> String {
    format!("{}\\{endpoint}\\{SOUS_CLE_PROPRIETES}", chemin_flux(cote))
}

// ---------------------------------------------------------------------------------
// La correspondance câble ↔ clés.
// ---------------------------------------------------------------------------------

/// Quel câble cet endpoint désigne-t-il, d'après ce que sa clé contient ?
///
/// `marque` est notre valeur, `description` est `PKEY_Device_DeviceDesc`. La marque est
/// consultée **d'abord** : elle survit au renommage, la description non. Un endpoint
/// jamais renommé n'a pas de marque et se reconnaît à sa description ; un endpoint
/// renommé n'a plus la bonne description et se reconnaît à sa marque.
///
/// Les deux passent par [`crate::controle::cable_nomme`], donc par une **égalité
/// exacte** avec `Conduit N` : ni préfixe (« Conduit 1 » attraperait « Conduit 10 » à
/// « Conduit 16 »), ni zéro de tête, ni nom composé.
///
/// Un endpoint dont la marque existe mais ne nomme aucun câble ne retombe **pas** sur sa
/// description : une marque illisible est le signe que quelqu'un a écrit dans notre
/// valeur, et deviner à sa place vaudrait moins que ne rien faire.
#[must_use]
pub fn cable_designe(description: Option<&str>, marque: Option<&str>) -> Option<CableId> {
    match marque {
        Some(marque) => cable_nomme(marque),
        None => cable_nomme(description?),
    }
}

/// Le nom d'origine du câble `cable` : `Conduit N`, celui que le pilote publie.
///
/// Il n'a pas besoin d'être retenu quelque part — il se déduit du numéro. La marque ne
/// sert donc qu'à retrouver le **numéro**, pas à mémoriser un texte.
#[must_use]
pub fn nom_d_origine(cable: CableId) -> String {
    nom(cable)
}

// ---------------------------------------------------------------------------------
// L'écriture proprement dite.
// ---------------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::{
        cable_designe, chemin_flux, chemin_proprietes, nom_d_origine, valeur_description,
        valeur_marque, CableId, Cote,
    };
    use ::windows::core::PCWSTR;
    use ::windows::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    };
    use ::windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE,
    };
    use core::fmt;

    /// Longueur maximale, en caractères, d'un nom de sous-clé d'endpoint.
    ///
    /// Le maximum d'un nom de clé du registre est 255 caractères ; les identifiants
    /// d'endpoint sont des GUID de 38. La marge est là pour n'avoir jamais à boucler.
    const MAX_NOM_CLE: usize = 256;

    /// Longueur maximale, en caractères, d'une valeur texte qu'on accepte de lire.
    ///
    /// Une description plus longue que cela ne peut pas être un `Conduit N` : la lecture
    /// rend alors `None`, ce qui est le bon verdict — « ce n'est pas un des nôtres ».
    const MAX_VALEUR: usize = 512;

    /// Ce qui a fait échouer une opération de registre.
    ///
    /// Chaque variante porte **le chemin** et **le code Win32 tel quel** : c'est ce qui
    /// distingue « la clé n'existe pas » de « le service n'a pas le droit d'écrire », et
    /// aucun message générique ne le dirait.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ErreurRegistre {
        /// Une clé n'a pas pu être ouverte.
        Ouverture {
            /// Le chemin, sous `HKLM`.
            chemin: String,
            /// Le code rendu par `RegOpenKeyExW`.
            code: u32,
        },
        /// L'énumération des endpoints d'un flux a échoué.
        Enumeration {
            /// Le chemin, sous `HKLM`.
            chemin: String,
            /// Le code rendu par `RegEnumKeyExW`.
            code: u32,
        },
        /// Une valeur n'a pas pu être écrite ou supprimée.
        Ecriture {
            /// Le nom de la valeur.
            valeur: String,
            /// Le chemin de la clé, sous `HKLM`.
            chemin: String,
            /// Le code rendu par `RegSetValueExW` ou `RegDeleteValueW`.
            code: u32,
        },
        /// Aucun endpoint ne désigne ce câble.
        Aucun {
            /// Le câble cherché.
            cable: CableId,
            /// Combien de côtés ont été trouvés : 0 ou 1, jamais 2.
            trouves: u32,
        },
    }

    impl fmt::Display for ErreurRegistre {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Ouverture { chemin, code } => {
                    write!(
                        f,
                        "HKLM\\{chemin} : ouverture refusée (erreur Win32 {code})"
                    )
                }
                Self::Enumeration { chemin, code } => write!(
                    f,
                    "HKLM\\{chemin} : énumération interrompue (erreur Win32 {code})"
                ),
                Self::Ecriture {
                    valeur,
                    chemin,
                    code,
                } => write!(
                    f,
                    "HKLM\\{chemin} : « {valeur} » non écrite (erreur Win32 {code})"
                ),
                Self::Aucun { cable, trouves } => write!(
                    f,
                    "aucun endpoint ne porte « {} » ({trouves} côté(s) trouvé(s) sur 2) : \
                     Windows ne publie les endpoints d'un câble que lorsqu'il est connecté",
                    nom_d_origine(*cable)
                ),
            }
        }
    }

    impl std::error::Error for ErreurRegistre {}

    impl ErreurRegistre {
        /// Le code Win32 à mettre dans `Reponse::detail`, ou le nombre de côtés trouvés
        /// pour [`Self::Aucun`].
        #[must_use]
        pub const fn detail(&self) -> u32 {
            match self {
                Self::Ouverture { code, .. }
                | Self::Enumeration { code, .. }
                | Self::Ecriture { code, .. } => *code,
                Self::Aucun { trouves, .. } => *trouves,
            }
        }
    }

    /// Une clé de registre ouverte, refermée par son `Drop`.
    ///
    /// Le garde existe pour la même raison que celui de `SeLoadDriverPrivilege` : un
    /// chemin d'erreur ne doit pas pouvoir fuir une poignée dans un service qui tourne
    /// des mois.
    struct Cle(HKEY);

    impl Drop for Cle {
        fn drop(&mut self) {
            // SAFETY: `self.0` vient d'un `RegOpenKeyExW` réussi et n'est fermée qu'ici.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    /// Une chaîne Rust en tampon UTF-16 terminé par un `NUL`, prêt pour une `PCWSTR`.
    fn large(texte: &str) -> Vec<u16> {
        texte.encode_utf16().chain(core::iter::once(0)).collect()
    }

    /// Ouvre `HKLM\<chemin>` avec les droits `acces`.
    fn ouvrir(
        chemin: &str,
        acces: ::windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<Cle, ErreurRegistre> {
        let large = large(chemin);
        let mut cle = HKEY::default();
        // SAFETY: `large` est terminé par un NUL et vit jusqu'à la fin de l'appel ;
        // `cle` est une variable locale valide en écriture.
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(large.as_ptr()),
                None,
                acces,
                &raw mut cle,
            )
        };
        if code == ERROR_SUCCESS {
            Ok(Cle(cle))
        } else {
            Err(ErreurRegistre::Ouverture {
                chemin: chemin.to_owned(),
                code: code.0,
            })
        }
    }

    /// Les noms des sous-clés de `cle`, dans l'ordre du registre.
    fn sous_cles(cle: &Cle, chemin: &str) -> Result<Vec<String>, ErreurRegistre> {
        let mut noms = Vec::new();
        let mut index = 0u32;
        loop {
            let mut tampon = [0u16; MAX_NOM_CLE];
            let mut longueur = MAX_NOM_CLE as u32;
            // SAFETY: `tampon` et `longueur` sont des locales valides ; `longueur` dit
            // au système combien de caractères le tampon peut recevoir.
            let code = unsafe {
                RegEnumKeyExW(
                    cle.0,
                    index,
                    Some(::windows::core::PWSTR(tampon.as_mut_ptr())),
                    &raw mut longueur,
                    None,
                    None,
                    None,
                    None,
                )
            };
            match code {
                ERROR_SUCCESS => {
                    let vus = (longueur as usize).min(MAX_NOM_CLE);
                    noms.push(String::from_utf16_lossy(tampon.get(..vus).unwrap_or(&[])));
                    index = index.saturating_add(1);
                }
                ERROR_NO_MORE_ITEMS => return Ok(noms),
                // Un nom qui ne tient pas dans 256 caractères n'est pas un identifiant
                // d'endpoint : on saute, on n'abandonne pas l'énumération.
                ERROR_MORE_DATA => index = index.saturating_add(1),
                autre => {
                    return Err(ErreurRegistre::Enumeration {
                        chemin: chemin.to_owned(),
                        code: autre.0,
                    })
                }
            }
        }
    }

    /// Lit une valeur `REG_SZ` de `cle`, ou `None`.
    ///
    /// `None` couvre trois cas qui appellent la même conduite — la valeur n'existe pas,
    /// elle n'est pas une chaîne, elle est plus longue que ce qu'un nom d'endpoint peut
    /// être. Aucun n'est une panne : c'est « ce n'est pas un des nôtres ».
    fn lire_texte(cle: &Cle, valeur: &str) -> Option<String> {
        let large_valeur = large(valeur);
        let mut tampon = [0u8; MAX_VALEUR * 2];
        let mut octets = (MAX_VALEUR * 2) as u32;
        let mut genre = REG_VALUE_TYPE::default();
        // SAFETY: `large_valeur` est terminé par un NUL ; `tampon` et `octets` sont des
        // locales, et `octets` borne ce que le système écrit dans `tampon`.
        let code = unsafe {
            RegQueryValueExW(
                cle.0,
                PCWSTR(large_valeur.as_ptr()),
                None,
                Some(&raw mut genre),
                Some(tampon.as_mut_ptr()),
                Some(&raw mut octets),
            )
        };
        if code != ERROR_SUCCESS || genre != REG_SZ {
            return None;
        }
        let vus = (octets as usize).min(tampon.len()) / 2;
        let large: Vec<u16> = tampon
            .chunks_exact(2)
            .take(vus)
            .map(|paire| u16::from_le_bytes([paire[0], paire[1]]))
            .collect();
        // `REG_SZ` est terminé par un `NUL` que la longueur rendue compte : on le retire
        // plutôt que de le laisser dans une chaîne qu'on comparera à `Conduit N`.
        let utile = large.split(|c| *c == 0).next().unwrap_or(&[]);
        Some(String::from_utf16_lossy(utile))
    }

    /// Écrit une valeur `REG_SZ`.
    fn ecrire_texte(
        cle: &Cle,
        chemin: &str,
        valeur: &str,
        texte: &str,
    ) -> Result<(), ErreurRegistre> {
        let large_valeur = large(valeur);
        let large_texte = large(texte);
        let octets: Vec<u8> = large_texte.iter().flat_map(|c| c.to_le_bytes()).collect();
        // SAFETY: les deux tampons vivent jusqu'à la fin de l'appel ; `octets` porte sa
        // propre longueur, `NUL` de fin compris comme `REG_SZ` l'exige.
        let code = unsafe {
            RegSetValueExW(
                cle.0,
                PCWSTR(large_valeur.as_ptr()),
                None,
                REG_SZ,
                Some(&octets),
            )
        };
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ErreurRegistre::Ecriture {
                valeur: valeur.to_owned(),
                chemin: chemin.to_owned(),
                code: code.0,
            })
        }
    }

    /// Supprime une valeur. Une valeur déjà absente est un **succès** : l'état voulu est
    /// atteint, et le contraire ferait échouer un `nom-defaut` rejoué.
    fn supprimer_valeur(cle: &Cle, chemin: &str, valeur: &str) -> Result<(), ErreurRegistre> {
        let large_valeur = large(valeur);
        // SAFETY: `large_valeur` est terminé par un NUL et vit jusqu'à la fin de l'appel.
        let code = unsafe { RegDeleteValueW(cle.0, PCWSTR(large_valeur.as_ptr())) };
        if code == ERROR_SUCCESS || code == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(ErreurRegistre::Ecriture {
                valeur: valeur.to_owned(),
                chemin: chemin.to_owned(),
                code: code.0,
            })
        }
    }

    /// Un endpoint tel que le registre le décrit : ce qui l'identifie, et à quel câble
    /// il se rattache.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Endpoint {
        /// Le côté du câble.
        pub cote: Cote,
        /// Le nom de la sous-clé — un identifiant **opaque**.
        pub endpoint: String,
        /// `PKEY_Device_DeviceDesc`, le nom **affiché** de l'endpoint.
        pub description: Option<String>,
        /// La marque de Conduit, présente seulement sur un endpoint déjà renommé.
        pub marque: Option<String>,
        /// Le câble que cet endpoint désigne, d'après [`cable_designe`].
        pub cable: Option<CableId>,
    }

    /// **Tous** les endpoints audio de la machine, avec ce que leur clé porte.
    ///
    /// En **lecture seule**, et sans filtre : c'est la fonction de diagnostic, celle qui
    /// permet de voir ce que le service voit lorsqu'un renommage ne fait pas ce qu'on
    /// attendait. [`trouver`] n'en est qu'un filtre.
    ///
    /// Un endpoint dont la clé `Properties` refuse de s'ouvrir est **sauté**, pas
    /// rapporté en erreur : sur une machine ordinaire, plusieurs clés d'endpoint
    /// appartiennent à `SYSTEM` seul, et abandonner l'énumération pour l'une d'elles
    /// ferait manquer toutes les suivantes.
    ///
    /// # Erreurs
    ///
    /// [`ErreurRegistre::Ouverture`] ou [`ErreurRegistre::Enumeration`] : l'une des deux
    /// clés de flux n'a pas pu être ouverte ou parcourue. C'est la seule panne qui
    /// empêche de rien dire du tout.
    pub fn inventorier() -> Result<Vec<Endpoint>, ErreurRegistre> {
        let description = valeur_description();
        let marque = valeur_marque();
        let mut vus = Vec::new();
        for cote in Cote::ALL {
            let chemin = chemin_flux(cote);
            let flux = ouvrir(&chemin, KEY_READ)?;
            for endpoint in sous_cles(&flux, &chemin)? {
                let chemin_props = chemin_proprietes(cote, &endpoint);
                let Ok(props) = ouvrir(&chemin_props, KEY_READ) else {
                    continue;
                };
                let lue = lire_texte(&props, &description);
                let marquee = lire_texte(&props, &marque);
                vus.push(Endpoint {
                    cote,
                    endpoint,
                    cable: cable_designe(lue.as_deref(), marquee.as_deref()),
                    description: lue,
                    marque: marquee,
                });
            }
        }
        Ok(vus)
    }

    /// Les endpoints qui désignent le câble `cable`, **au plus un par côté**.
    ///
    /// Le premier de chaque côté est retenu : deux endpoints du même côté portant le même
    /// `Conduit N` seraient un état que Windows ne produit pas, et en renommer deux
    /// vaudrait moins que d'en renommer un.
    ///
    /// # Erreurs
    ///
    /// Celles de [`inventorier`]. Un câble sans aucun endpoint n'est pas une erreur ici —
    /// c'est [`appliquer`] qui décide qu'aucun côté trouvé est un refus.
    pub fn trouver(cable: CableId) -> Result<Vec<Endpoint>, ErreurRegistre> {
        let mut trouves: Vec<Endpoint> = Vec::new();
        for vu in inventorier()? {
            if vu.cable == Some(cable) && !trouves.iter().any(|deja| deja.cote == vu.cote) {
                trouves.push(vu);
            }
        }
        Ok(trouves)
    }

    /// Donne au câble le nom `voulu`, ou lui rend son nom d'origine si `voulu` est
    /// `None`, et rend le nombre de côtés écrits.
    ///
    /// # L'ordre des écritures
    ///
    /// À l'aller, la **marque** est écrite avant la description ; au retour, la
    /// description est écrite avant que la marque ne soit supprimée. Dans les deux sens,
    /// une interruption entre les deux laisse un endpoint encore rattachable à son
    /// câble. Voir l'en-tête de module.
    ///
    /// # Erreurs
    ///
    /// [`ErreurRegistre::Aucun`] si le câble n'a aucun endpoint publié — le cas d'un
    /// câble déconnecté —, sinon l'échec du registre, code Win32 compris.
    pub fn appliquer(cable: CableId, voulu: Option<&str>) -> Result<u32, ErreurRegistre> {
        let trouves = trouver(cable)?;
        if trouves.len() < Cote::ALL.len() {
            // Zéro ou un seul côté : les deux méritent d'être dits, et le nombre part
            // dans la réponse. On n'écrit rien — renommer un seul côté d'un câble
            // laisserait une entrée et une sortie qui ne portent plus le même nom.
            return Err(ErreurRegistre::Aucun {
                cable,
                trouves: trouves.len() as u32,
            });
        }
        let description = valeur_description();
        let marque = valeur_marque();
        let origine = nom_d_origine(cable);
        let mut ecrits = 0u32;
        for vu in &trouves {
            let chemin = chemin_proprietes(vu.cote, &vu.endpoint);
            let props = ouvrir(&chemin, KEY_SET_VALUE)?;
            match voulu {
                Some(nouveau) => {
                    ecrire_texte(&props, &chemin, &marque, &origine)?;
                    ecrire_texte(&props, &chemin, &description, nouveau)?;
                }
                None => {
                    ecrire_texte(&props, &chemin, &description, &origine)?;
                    supprimer_valeur(&props, &chemin, &marque)?;
                }
            }
            ecrits = ecrits.saturating_add(1);
        }
        Ok(ecrits)
    }
}

#[cfg(windows)]
pub use windows::{appliquer, inventorier, trouver, Endpoint, ErreurRegistre};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use conduit_kmd_core::config::CABLE_MAX;

    /// **La valeur qu'on écrit**, en toutes lettres.
    ///
    /// Ce test est le contrat de M1b-21 : si quelqu'un déplace le renommage vers un
    /// autre `PROPERTYKEY`, il tombe, et le rapport de la tâche ne correspond plus au
    /// code.
    #[test]
    fn la_valeur_ecrite_est_pkey_device_devicedesc() {
        assert_eq!(
            valeur_description(),
            "{a45c254e-df1c-4efd-8020-67d146a850e0},2"
        );
        // Le `fmtid` est celui de `Functiondiscoverykeys_devpkey.h`, partagé par
        // `PKEY_Device_DeviceDesc` (pid 2) et `PKEY_Device_FriendlyName` (pid 14).
        assert_eq!(
            nom_de_valeur(FMTID_DEVICE, PID_DEVICE_FRIENDLY_NAME),
            "{a45c254e-df1c-4efd-8020-67d146a850e0},14"
        );
        // Et ce n'est **pas** celle-là qu'on écrit : Windows la compose.
        assert_ne!(
            valeur_description(),
            nom_de_valeur(FMTID_DEVICE, PID_DEVICE_FRIENDLY_NAME)
        );
    }

    /// La marque porte le GUID **gravé** de Conduit, formaté et non recopié.
    #[test]
    fn la_marque_porte_le_guid_de_conduit() {
        assert_eq!(fmtid_conduit(), "{3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b}");
        assert_eq!(valeur_marque(), "{3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b},1");
        // Elle ne peut pas entrer en collision avec celle du système.
        assert_ne!(valeur_marque(), valeur_description());
        // Le `pid` 0 est réservé par le système de propriétés Windows.
        assert_ne!(PID_MARQUE, 0);
    }

    /// Les chemins, à la lettre : c'est ce que Nathan tapera dans `regedit`.
    #[test]
    fn les_chemins_table() {
        assert_eq!(
            RACINE_MMDEVICES,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio"
        );
        let cas = [
            (
                Cote::Rendu,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render",
            ),
            (
                Cote::Capture,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture",
            ),
        ];
        for (cote, attendu) in cas {
            assert_eq!(chemin_flux(cote), attendu, "{cote:?}");
            assert_eq!(
                chemin_proprietes(cote, "{0328cec1-6888-440a-89f5-2f8fbc6a7567}"),
                format!("{attendu}\\{{0328cec1-6888-440a-89f5-2f8fbc6a7567}}\\Properties"),
                "{cote:?}"
            );
        }
        // Les deux côtés sont des sous-clés distinctes de la même racine.
        assert_ne!(chemin_flux(Cote::Rendu), chemin_flux(Cote::Capture));
        for cote in Cote::ALL {
            assert!(chemin_flux(cote).starts_with(RACINE_MMDEVICES), "{cote:?}");
            // Un chemin **relatif** à la ruche : pas de `HKLM\` en tête.
            assert!(!chemin_flux(cote).starts_with("HKLM"), "{cote:?}");
        }
    }

    /// **Le piège du préfixe, sur les seize.**
    ///
    /// « Conduit 1 » ne doit attraper que le câble 1 — jamais « Conduit 10 » à
    /// « Conduit 16 ». Un appariement par préfixe ferait tomber ce test sur sept cas.
    #[test]
    fn l_appariement_est_une_egalite_exacte() {
        for numero in 1..=CABLE_MAX {
            let cible = CableId(numero);
            let description = nom_d_origine(cible);
            assert_eq!(
                cable_designe(Some(&description), None),
                Some(cible),
                "câble {numero}"
            );
            // Aucun autre câble ne se reconnaît dans cette description.
            for autre in 1..=CABLE_MAX {
                if autre == numero {
                    continue;
                }
                assert_ne!(
                    cable_designe(Some(&description), None),
                    Some(CableId(autre)),
                    "« {description} » attrapé par le câble {autre}"
                );
            }
        }
    }

    /// Table de ce qui n'est **pas** un endpoint de Conduit.
    #[test]
    fn ce_qui_ne_designe_aucun_cable() {
        let cas = [
            None,
            Some("Haut-parleurs"),
            Some("Realtek Digital Output"),
            // Le nom **composé**, que Windows affiche : ce n'est pas un identifiant.
            Some("Conduit 1 (Conduit — câbles audio virtuels)"),
            Some("Conduit"),
            Some("Conduit "),
            Some("conduit 1"),
            Some("Conduit 01"),
            Some("Conduit 0"),
            Some("Conduit 17"),
            Some("Conduit 1 "),
            Some(" Conduit 1"),
            // Un câble déjà renommé : sa description ne dit plus rien, et sans marque il
            // n'est plus rattachable — c'est exactement pourquoi la marque existe.
            Some("Musique"),
            Some(""),
        ];
        for description in cas {
            assert_eq!(cable_designe(description, None), None, "{description:?}");
        }
    }

    /// **La marque prime sur la description**, et c'est elle qui fait survivre le lien
    /// au renommage.
    #[test]
    fn la_marque_prime_sur_la_description() {
        // Après un renommage : la description dit « Musique », la marque dit le câble.
        assert_eq!(
            cable_designe(Some("Musique"), Some("Conduit 3")),
            Some(CableId(3))
        );
        // Avant tout renommage : pas de marque, la description suffit.
        assert_eq!(cable_designe(Some("Conduit 3"), None), Some(CableId(3)));
        // Une marque illisible ne retombe pas sur la description : quelqu'un a écrit
        // dans notre valeur, et deviner à sa place vaudrait moins que ne rien faire.
        assert_eq!(
            cable_designe(Some("Conduit 3"), Some("n'importe quoi")),
            None
        );
        assert_eq!(cable_designe(Some("Conduit 3"), Some("")), None);
        // Une marque qui contredit la description : c'est la marque qui gagne, puisque
        // c'est elle qui survit à l'écrasement.
        assert_eq!(
            cable_designe(Some("Conduit 5"), Some("Conduit 2")),
            Some(CableId(2))
        );
    }

    /// Le nom d'origine se **déduit** du numéro : rien n'est mémorisé.
    #[test]
    fn le_nom_d_origine_se_deduit_du_numero() {
        for numero in 1..=CABLE_MAX {
            let cable = CableId(numero);
            assert_eq!(nom_d_origine(cable), format!("Conduit {numero}"));
            // Et il se relit : c'est ce qui permet de revenir en arrière sans état.
            assert_eq!(
                cable_designe(Some(&nom_d_origine(cable)), None),
                Some(cable)
            );
        }
    }

    /// Le format d'un nom de valeur est celui du registre, pas une invention.
    #[test]
    fn le_format_d_un_nom_de_valeur() {
        assert_eq!(nom_de_valeur("{abc}", 7), "{abc},7");
        assert_eq!(nom_de_valeur("{abc}", 0), "{abc},0");
        // Un GUID est rendu en minuscules et entre accolades, comme `regedit` l'affiche.
        let texte = fmtid_conduit();
        assert!(texte.starts_with('{') && texte.ends_with('}'), "{texte}");
        assert_eq!(texte.len(), 38, "{texte}");
        assert!(
            texte.chars().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || c == '-'
                || c == '{'
                || c == '}'),
            "{texte}"
        );
    }

    /// Les deux côtés existent et se nomment comme `EDataFlow`.
    #[test]
    fn les_deux_cotes() {
        assert_eq!(sous_cle_flux(Cote::Rendu), "Render");
        assert_eq!(sous_cle_flux(Cote::Capture), "Capture");
        assert_eq!(Cote::ALL.len(), 2);
        assert_eq!(SOUS_CLE_PROPRIETES, "Properties");
    }
}
