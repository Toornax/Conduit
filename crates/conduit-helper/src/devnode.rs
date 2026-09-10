//! Le format d'un câble dans la clé **matérielle** du devnode, et le redémarrage qui le
//! rend effectif (M1b-05, lot A1).
//!
//! # Le principe qui commande ce module
//!
//! Le même que [`crate::registre`] : le pilote noyau reste le plus léger possible, et tout
//! ce qui peut être fait en espace utilisateur y est fait. Changer le format d'un câble
//! est l'exemple pur — le pilote **ne sait pas** en changer à chaud, ses tables KS sont
//! immuables et PortCls en retient les pointeurs pour toute la vie du filtre. Il ne sait
//! que **lire son format au démarrage du devnode**. Le service écrit donc la valeur et
//! redémarre le devnode ; rien de ce fichier ne descend dans `drivers/windows`.
//!
//! # La valeur qu'on écrit
//!
//! Un `REG_DWORD` nommé `CableFormat<index>` dans la clé matérielle (`HKR`) du
//! périphérique — la même clé que `ActiveCables`, et pour la même raison : PnP la supprime
//! avec le périphérique (F-52). Les noms viennent de
//! [`conduit_kmd_core::config::CABLE_FORMAT_VALUE_NAMES`], l'encodage de
//! [`conduit_kmd_core::config::CableFormat::encode`] : quatre lecteurs, une seule liste,
//! un seul codec.
//!
//! L'index qui suffixe le nom est l'index **pilote** (`CableFormat0` = « Conduit 1 »), et
//! le décalage de un ne se fait pas ici : il appartient à
//! `conduit_backend_wasapi::cable::driver_index`, seul endroit du dépôt qui le connaisse.
//! [`nom_de_valeur`] prend donc un index, jamais un [`conduit_backend::CableId`].
//!
//! # Les quatre appels, et pourquoi ceux-là
//!
//! 1. `CM_Locate_DevNodeW` — de l'identifiant d'instance au `DEVINST`.
//! 2. `CM_Open_DevNode_Key(…, CM_REGISTRY_HARDWARE)` — la clé matérielle.
//!
//!    **Pas `SetupDiOpenDevRegKey`.** On part d'un identifiant d'instance que
//!    `conduit_backend_wasapi::cable::devnode_instance` a obtenu de cfgmgr32 ; y revenir
//!    par SetupAPI demanderait de construire un `HDEVINFO` et un `SP_DEVINFO_DATA` pour
//!    retrouver ce qu'on a déjà, avec une seconde famille d'erreurs à traduire. Une seule
//!    surface, un seul style de code d'erreur : le `CONFIGRET`.
//!
//! 3. `CM_Query_And_Remove_SubTreeW` puis
//! 4. `CM_Setup_DevNode(CM_SETUP_DEVNODE_READY)` — le redémarrage.
//!
//!    **Pas `CM_Disable_DevNode` / `CM_Enable_DevNode`.** Le couple laisse un **état** :
//!    un service qui meurt entre les deux laisse le périphérique désactivé, donc les seize
//!    câbles disparus, et rien dans le journal ne dit pourquoi. Le retrait-rétablissement
//!    n'a pas d'état intermédiaire persistant — un devnode retiré et non rétabli revient
//!    à la prochaine énumération PnP —, et il **diagnostique** : un veto nomme
//!    l'application qui tient un flux ouvert, là où `Disable` rend un code muet.
//!
//!    **Pas `pnputil`.** Lancer un processus depuis `LocalSystem` pour analyser sa sortie
//!    — traduite dans la langue du poste — serait une surface et une fragilité pour un
//!    travail que trois appels font.
//!
//! # Les deux moitiés d'un échec, et pourquoi elles ont deux statuts
//!
//! Les étapes 1 et 2 échouent **avant** toute écriture : la clé est intacte, le câble sert
//! toujours son ancien format, réessayer est sans danger. C'est
//! [`crate::protocole::Statut::FormatNonEcrit`].
//!
//! Les étapes 3 et 4 échouent **après** : la valeur est posée et prendra effet au prochain
//! démarrage du périphérique, qu'on le veuille ou non. C'est
//! [`crate::protocole::Statut::RedemarrageEchoue`], et sa conduite est l'opposée —
//! provoquer ce redémarrage, ou réécrire l'ancienne valeur. [`ErreurDevnode::ecrite`]
//! porte cette distinction, pour que `crate::cables` n'ait pas à la redécouvrir.
//!
//! # Ce que ce module ne fait pas
//!
//! Il n'ouvre aucun flux audio et n'émet aucun son. Il n'arme **pas** le privilège non
//! plus : `CM_Query_And_Remove_SubTreeW` exige `SeLoadDriverPrivilege` armé, mais c'est
//! `crate::cables` qui l'arme, au dernier moment et pour la durée de l'écriture, comme
//! pour les ordres qui parlent au pilote.
//!
//! # Ce qui est pur et ce qui ne l'est pas
//!
//! Les noms de valeurs et les messages d'erreur sont **purs**, compilés partout et
//! vérifiés en table de cas. Seul le module `windows` privé touche au registre et au
//! gestionnaire de configuration, et aucun test ne l'exerce : la contrainte du dépôt
//! interdit d'écrire dans la clé matérielle d'un périphérique depuis `cargo test`, et
//! retirer un devnode ferait disparaître les cartes son du poste le temps d'un battement.

use core::fmt;

use conduit_kmd_core::config::{cable_format_value_name, CABLE_MAX};

// ---------------------------------------------------------------------------------
// La partie pure : les noms de valeurs et les causes de refus.
// ---------------------------------------------------------------------------------

/// Le nom de la valeur `REG_DWORD` qui porte le format du câble d'**index pilote**
/// `index`, ou `None` au-delà du dernier câble.
///
/// Ré-exporté du contrat partagé plutôt que composé ici : `conduit-kmd-core` est `no_std`
/// et compose ces seize chaînes à la compilation, le pilote les lit, l'INF les écrit, et
/// une dix-septième écriture en dur finirait par diverger.
#[must_use]
pub fn nom_de_valeur(index: u32) -> Option<&'static str> {
    cable_format_value_name(index)
}

/// Ce qui a fait échouer l'application d'un format.
///
/// Chaque variante porte **le code du système tel quel** — `CONFIGRET` de cfgmgr32 ou code
/// Win32 du registre — et son message dit **quoi faire**, pas seulement qu'on a échoué.
/// Le partage entre les variantes d'écriture et celles de redémarrage est la distinction
/// qui compte : voir [`Self::ecrite`] et l'en-tête de module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurDevnode {
    /// Aucun nom de valeur pour cet index : le câble est hors des seize.
    ///
    /// Ne peut pas arriver — le parseur du protocole borne le numéro — et le chemin existe
    /// pour que ce module n'ait pas à faire confiance à un autre.
    Cable {
        /// L'index pilote reçu.
        index: u32,
    },
    /// `CM_Locate_DevNodeW` n'a pas trouvé le périphérique.
    Localisation {
        /// L'identifiant d'instance cherché.
        instance: String,
        /// Le `CONFIGRET` rendu.
        configret: u32,
    },
    /// `CM_Open_DevNode_Key` a refusé la clé matérielle.
    CleMaterielle {
        /// L'identifiant d'instance du périphérique.
        instance: String,
        /// Le `CONFIGRET` rendu.
        configret: u32,
    },
    /// La valeur n'a pas pu être écrite.
    Ecriture {
        /// Le nom de la valeur, `CableFormat<n>`.
        valeur: &'static str,
        /// Le code Win32 rendu par `RegSetValueExW`.
        code: u32,
    },
    /// Le retrait du devnode a été **refusé par un veto** : quelque chose tient le
    /// périphérique.
    Veto {
        /// Le type de veto, tel que `PNP_VETO_TYPE` le code.
        genre: i32,
        /// Le nom que PnP a rendu : l'application, le service ou le pilote qui bloque.
        vetoteur: String,
    },
    /// Le retrait du devnode a échoué pour une autre raison.
    Retrait {
        /// Le `CONFIGRET` rendu par `CM_Query_And_Remove_SubTreeW`.
        configret: u32,
    },
    /// Le devnode a été retiré mais n'a pas pu être rétabli.
    ///
    /// Le cas le plus désagréable, et c'est pourquoi il est nommé : les seize câbles ont
    /// disparu et ne reviendront qu'à la prochaine énumération PnP.
    Retablissement {
        /// Le `CONFIGRET` rendu par `CM_Setup_DevNode`.
        configret: u32,
    },
}

impl ErreurDevnode {
    /// La valeur est-elle **écrite** dans la clé matérielle malgré cet échec ?
    ///
    /// C'est toute la différence entre `FormatNonEcrit` et `RedemarrageEchoue` : `false`
    /// veut dire « rien n'a changé, réessayez », `true` veut dire « la valeur est posée et
    /// prendra effet au prochain démarrage du périphérique ». Le prédicat vit ici et non
    /// dans `crate::cables` pour qu'une variante ajoutée demain doive répondre à la
    /// question au moment où on l'écrit.
    #[must_use]
    pub const fn ecrite(&self) -> bool {
        match self {
            Self::Cable { .. }
            | Self::Localisation { .. }
            | Self::CleMaterielle { .. }
            | Self::Ecriture { .. } => false,
            Self::Veto { .. } | Self::Retrait { .. } | Self::Retablissement { .. } => true,
        }
    }

    /// Le code chiffré à mettre dans `crate::protocole::Reponse::detail`, **tel quel**.
    ///
    /// `CONFIGRET` pour les appels de cfgmgr32, code Win32 pour le registre, type de veto
    /// pour un refus de retrait — et l'index du câble pour le cas injoignable. Chacun est
    /// le seul renseignement qui distingue deux échecs de la même variante.
    #[must_use]
    pub const fn detail(&self) -> u32 {
        match self {
            Self::Cable { index } => *index,
            Self::Localisation { configret, .. }
            | Self::CleMaterielle { configret, .. }
            | Self::Retrait { configret }
            | Self::Retablissement { configret } => *configret,
            Self::Ecriture { code, .. } => *code,
            // Le type de veto est un `i32` des en-têtes PnP, positif par construction ;
            // `unsigned_abs` interdit la panique sans mentir sur la valeur.
            Self::Veto { genre, .. } => genre.unsigned_abs(),
        }
    }
}

impl fmt::Display for ErreurDevnode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cable { index } => write!(
                f,
                "câble d'index {index} hors des {CABLE_MAX} câbles adressables : aucun nom \
                 de valeur ne lui correspond"
            ),
            Self::Localisation {
                instance,
                configret,
            } => write!(
                f,
                "périphérique « {instance} » introuvable (CONFIGRET {configret}) : le \
                 pilote Conduit n'est pas installé, ou il vient d'être retiré — rien n'a \
                 été écrit"
            ),
            Self::CleMaterielle {
                instance,
                configret,
            } => write!(
                f,
                "clé matérielle de « {instance} » inaccessible (CONFIGRET {configret}) : \
                 le service doit tourner en LocalSystem — rien n'a été écrit"
            ),
            Self::Ecriture { valeur, code } => write!(
                f,
                "« {valeur} » non écrite dans la clé matérielle (erreur Win32 {code}) : \
                 rien n'a changé, le câble sert toujours son ancien format"
            ),
            Self::Veto { genre, vetoteur } => write!(
                f,
                "redémarrage du périphérique refusé par « {vetoteur} » (veto de type \
                 {genre}) : le format est écrit mais ne s'appliquera qu'au prochain \
                 démarrage du périphérique — fermez ce qui tient un flux audio, puis \
                 réessayez"
            ),
            Self::Retrait { configret } => write!(
                f,
                "retrait du périphérique refusé (CONFIGRET {configret}) : le format est \
                 écrit mais ne s'appliquera qu'au prochain démarrage du périphérique, au \
                 redémarrage de la machine au pire"
            ),
            Self::Retablissement { configret } => write!(
                f,
                "périphérique retiré mais non rétabli (CONFIGRET {configret}) : les câbles \
                 ont disparu et reviendront à la prochaine énumération PnP — « Rechercher \
                 les modifications sur le matériel » dans le Gestionnaire de \
                 périphériques, ou un redémarrage. Le format, lui, est écrit"
            ),
        }
    }
}

impl std::error::Error for ErreurDevnode {}

/// Ce qu'une application de format a réellement fait, pour le journal.
///
/// Les deux durées sont mesurées et non estimées : le retrait et le rétablissement d'un
/// devnode sont les seuls gestes du service qui coupent le son des seize câbles, et
/// combien de temps ils coûtent est exactement ce qu'on veut lire quand quelqu'un se
/// plaint d'un blanc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applique {
    /// L'encodage trouvé dans la clé avant l'écriture, 0 si la valeur n'existait pas.
    pub ancien: u32,
    /// L'encodage écrit.
    pub nouveau: u32,
    /// Durée du retrait du devnode.
    pub retrait: core::time::Duration,
    /// Durée du rétablissement.
    pub retablissement: core::time::Duration,
}

// ---------------------------------------------------------------------------------
// L'écriture et le redémarrage proprement dits.
// ---------------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::{Applique, ErreurDevnode, CABLE_MAX};
    use ::windows::core::PCWSTR;
    use ::windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Locate_DevNodeW, CM_Open_DevNode_Key, CM_Query_And_Remove_SubTreeW, CM_Setup_DevNode,
        CM_LOCATE_DEVNODE_NORMAL, CM_LOCATE_DEVNODE_PHANTOM, CM_REGISTRY_HARDWARE,
        CM_REMOVE_UI_NOT_OK, CM_SETUP_DEVNODE_READY, CONFIGRET, CR_NO_SUCH_DEVNODE,
        CR_REMOVE_VETOED, CR_SUCCESS, PNP_VETO_TYPE,
    };
    use ::windows::Win32::Foundation::ERROR_SUCCESS;
    use ::windows::Win32::System::Registry::{
        RegCloseKey, RegQueryValueExW, RegSetValueExW, HKEY, KEY_QUERY_VALUE, KEY_SET_VALUE,
        REG_DWORD, REG_VALUE_TYPE,
    };
    use conduit_backend::CableId;
    use conduit_backend_wasapi::cable::driver_index;
    use conduit_kmd_core::config::CableFormat;
    use std::time::Instant;

    /// Longueur, en caractères, du tampon où PnP écrit le nom du vétoteur.
    ///
    /// `MAX_PATH`, la borne que la documentation de `CM_Query_And_Remove_SubTreeW`
    /// recommande : le nom est un chemin d'exécutable, un nom de service ou un nom de
    /// périphérique.
    const MAX_VETO: usize = 260;

    /// Une clé de devnode ouverte, refermée par son `Drop`.
    ///
    /// Le garde existe pour la raison de celui de `crate::registre` : un chemin d'erreur
    /// ne doit pas pouvoir fuir une poignée dans un service qui tourne des mois. Il
    /// compte double ici — la clé est ouverte **avant** un retrait de devnode, et une
    /// poignée oubliée sur une clé matérielle est de celles qui font vétoter le retrait.
    struct Cle(HKEY);

    impl Drop for Cle {
        fn drop(&mut self) {
            // SAFETY: `self.0` vient d'un `CM_Open_DevNode_Key` réussi et n'est fermée
            // qu'ici.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    /// Une chaîne Rust en tampon UTF-16 terminé par un `NUL`, prêt pour une `PCWSTR`.
    fn large(texte: &str) -> Vec<u16> {
        texte.encode_utf16().chain(core::iter::once(0)).collect()
    }

    /// Le `DEVINST` de l'identifiant d'instance `instance`.
    ///
    /// `drapeaux` est `CM_LOCATE_DEVNODE_NORMAL` ou `CM_LOCATE_DEVNODE_PHANTOM` : le
    /// second retrouve un devnode que le retrait vient de faire disparaître de
    /// l'énumération vivante.
    fn localiser(
        instance: &str,
        drapeaux: ::windows::Win32::Devices::DeviceAndDriverInstallation::CM_LOCATE_DEVNODE_FLAGS,
    ) -> Result<u32, ErreurDevnode> {
        let large = large(instance);
        let mut devinst: u32 = 0;
        // SAFETY: `large` est terminé par un NUL et vit jusqu'à la fin de l'appel ;
        // `devinst` est une locale valide en écriture.
        let ret = unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(large.as_ptr()), drapeaux) };
        if ret == CR_SUCCESS {
            Ok(devinst)
        } else {
            Err(ErreurDevnode::Localisation {
                instance: instance.to_owned(),
                configret: ret.0,
            })
        }
    }

    /// Ouvre la clé **matérielle** du devnode, en lecture et en écriture.
    ///
    /// `RegDisposition_OpenAlways` la crée si elle n'existe pas : un périphérique dont
    /// l'INF n'aurait pas écrit les seize `CableFormat<n>` doit pouvoir en recevoir un.
    fn ouvrir_cle_materielle(devinst: u32, instance: &str) -> Result<Cle, ErreurDevnode> {
        use ::windows::Win32::Devices::DeviceAndDriverInstallation::RegDisposition_OpenAlways;
        let mut hkey = HKEY::default();
        // SAFETY: `devinst` vient d'un `CM_Locate_DevNodeW` réussi ; `hkey` est une locale
        // valide en écriture, que le garde `Cle` refermera.
        let ret = unsafe {
            CM_Open_DevNode_Key(
                devinst,
                (KEY_QUERY_VALUE | KEY_SET_VALUE).0,
                0,
                RegDisposition_OpenAlways,
                &mut hkey,
                CM_REGISTRY_HARDWARE,
            )
        };
        if ret == CR_SUCCESS {
            Ok(Cle(hkey))
        } else {
            Err(ErreurDevnode::CleMaterielle {
                instance: instance.to_owned(),
                configret: ret.0,
            })
        }
    }

    /// Lit un `REG_DWORD` de `cle`, ou 0 s'il est absent ou d'un autre type.
    ///
    /// Une valeur absente **n'est pas une panne** : c'est « ce câble n'a pas de format
    /// écrit », ce que le pilote traite en se repliant sur le défaut. Rendre 0 laisse le
    /// protocole dire « inconnu » plutôt que d'inventer une valeur.
    fn lire_dword(cle: &Cle, valeur: &str) -> u32 {
        let large_valeur = large(valeur);
        let mut tampon = [0u8; 4];
        let mut octets = 4u32;
        let mut genre = REG_VALUE_TYPE::default();
        // SAFETY: `large_valeur` est terminé par un NUL ; `tampon`, `octets` et `genre`
        // sont des locales, et `octets` borne ce que le système écrit dans `tampon`.
        let code = unsafe {
            RegQueryValueExW(
                cle.0,
                PCWSTR(large_valeur.as_ptr()),
                None,
                Some(&mut genre),
                Some(tampon.as_mut_ptr()),
                Some(&mut octets),
            )
        };
        if code != ERROR_SUCCESS || genre != REG_DWORD || octets != 4 {
            return 0;
        }
        u32::from_ne_bytes(tampon)
    }

    /// Écrit un `REG_DWORD`.
    ///
    /// Boutisme **natif** : `REG_DWORD` est `REG_DWORD_LITTLE_ENDIAN` sur les plateformes
    /// que Windows sert, et c'est ainsi que le pilote le relit et que `regedit` l'affiche.
    fn ecrire_dword(cle: &Cle, valeur: &'static str, brut: u32) -> Result<(), ErreurDevnode> {
        let large_valeur = large(valeur);
        let octets = brut.to_ne_bytes();
        // SAFETY: les deux tampons vivent jusqu'à la fin de l'appel ; `octets` fait les
        // quatre octets qu'un `REG_DWORD` exige, et porte sa propre longueur.
        let code = unsafe {
            RegSetValueExW(
                cle.0,
                PCWSTR(large_valeur.as_ptr()),
                None,
                REG_DWORD,
                Some(&octets),
            )
        };
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ErreurDevnode::Ecriture {
                valeur,
                code: code.0,
            })
        }
    }

    /// Retire le devnode, puis le rétablit : c'est le **redémarrage**.
    ///
    /// Rend les deux durées mesurées. Voir l'en-tête de module pour le choix de
    /// `CM_Query_And_Remove_SubTreeW` contre `CM_Disable_DevNode`.
    fn redemarrer(
        devinst: u32,
        instance: &str,
    ) -> Result<(core::time::Duration, core::time::Duration), ErreurDevnode> {
        let mut genre = PNP_VETO_TYPE::default();
        let mut vetoteur = [0u16; MAX_VETO];
        let debut = Instant::now();
        // SAFETY: `devinst` vient d'un `CM_Locate_DevNodeW` réussi ; `genre` et `vetoteur`
        // sont des locales, et l'enveloppe de la caisse `windows` transmet la longueur du
        // tampon. `CM_REMOVE_UI_NOT_OK` interdit toute boîte de dialogue : un service
        // n'a pas de bureau où l'afficher.
        let ret = unsafe {
            CM_Query_And_Remove_SubTreeW(
                devinst,
                Some(&mut genre),
                Some(&mut vetoteur),
                CM_REMOVE_UI_NOT_OK,
            )
        };
        let retrait = debut.elapsed();
        if ret == CR_REMOVE_VETOED {
            return Err(ErreurDevnode::Veto {
                genre: genre.0,
                vetoteur: texte_veto(&vetoteur),
            });
        }
        if ret != CR_SUCCESS {
            return Err(ErreurDevnode::Retrait { configret: ret.0 });
        }

        let debut = Instant::now();
        let issue = retablir(devinst, instance);
        let retablissement = debut.elapsed();
        issue?;
        Ok((retrait, retablissement))
    }

    /// Rétablit le devnode, en le relocalisant une **seule** fois s'il a disparu.
    ///
    /// `CM_Query_And_Remove_SubTreeW` retire le devnode de l'énumération vivante :
    /// `CM_Setup_DevNode` peut donc rendre `CR_NO_SUCH_DEVNODE` sur un `DEVINST` qui était
    /// valide une milliseconde plus tôt. `CM_LOCATE_DEVNODE_PHANTOM` retrouve les devnodes
    /// dans cet état-là, et on réessaie **une** fois — une boucle transformerait un
    /// périphérique réellement parti en attente sans fin dans un service.
    fn retablir(devinst: u32, instance: &str) -> Result<(), ErreurDevnode> {
        // SAFETY: `devinst` vient d'un `CM_Locate_DevNodeW` réussi ; l'appel ne lit ni
        // n'écrit aucun tampon de cette pile.
        let ret = unsafe { CM_Setup_DevNode(devinst, CM_SETUP_DEVNODE_READY) };
        if ret == CR_SUCCESS {
            return Ok(());
        }
        if ret != CR_NO_SUCH_DEVNODE {
            return Err(ErreurDevnode::Retablissement { configret: ret.0 });
        }
        let fantome = localiser(instance, CM_LOCATE_DEVNODE_PHANTOM)
            .map_err(|_| ErreurDevnode::Retablissement { configret: ret.0 })?;
        // SAFETY: `fantome` vient du `CM_Locate_DevNodeW` ci-dessus, réussi.
        let ret: CONFIGRET = unsafe { CM_Setup_DevNode(fantome, CM_SETUP_DEVNODE_READY) };
        if ret == CR_SUCCESS {
            Ok(())
        } else {
            Err(ErreurDevnode::Retablissement { configret: ret.0 })
        }
    }

    /// Le nom du vétoteur, tel que PnP l'a écrit : jusqu'au premier `NUL`.
    ///
    /// Un tampon que PnP n'a pas rempli donne « (non nommé) » plutôt qu'une chaîne vide :
    /// le message de refus doit rester lisible même sans nom.
    fn texte_veto(tampon: &[u16; MAX_VETO]) -> String {
        let utile = tampon.split(|unite| *unite == 0).next().unwrap_or(&[]);
        if utile.is_empty() {
            "(non nommé)".to_owned()
        } else {
            String::from_utf16_lossy(utile)
        }
    }

    /// Écrit le format du câble `cable` dans la clé matérielle de `instance`, puis
    /// redémarre le devnode.
    ///
    /// L'ordre est celui du module : localiser, ouvrir, **relire** (pour le journal),
    /// écrire, puis redémarrer. La clé est refermée **avant** le retrait — une poignée
    /// ouverte sur une clé matérielle est de celles qui font vétoter un retrait.
    ///
    /// # Erreurs
    ///
    /// [`ErreurDevnode`], dont [`ErreurDevnode::ecrite`] dit si la valeur est posée : les
    /// deux moitiés appellent des conduites opposées, voir l'en-tête de module.
    pub fn appliquer(
        cable: CableId,
        format: CableFormat,
        instance: &str,
    ) -> Result<Applique, ErreurDevnode> {
        let index = driver_index(cable).unwrap_or(CABLE_MAX);
        let Some(valeur) = super::nom_de_valeur(index) else {
            return Err(ErreurDevnode::Cable { index });
        };
        let devinst = localiser(instance, CM_LOCATE_DEVNODE_NORMAL)?;
        let nouveau = format.encode();
        let ancien = {
            let cle = ouvrir_cle_materielle(devinst, instance)?;
            let ancien = lire_dword(&cle, valeur);
            ecrire_dword(&cle, valeur, nouveau)?;
            ancien
        };
        let (retrait, retablissement) = redemarrer(devinst, instance)?;
        Ok(Applique {
            ancien,
            nouveau,
            retrait,
            retablissement,
        })
    }

    /// Le format des **seize** câbles, lu en une seule ouverture de la clé matérielle.
    ///
    /// Une ouverture et seize lectures, et non seize ouvertures : c'est ce qui rend la
    /// table de [`crate::protocole::Reponse::formats`] à peu près gratuite dans un
    /// `lister`. Une valeur absente donne 0, « inconnu », que le protocole sait dire.
    ///
    /// **La source est le registre et non `CableState`.** Le contrat KS ne porte que les
    /// canaux ; la fréquence et la profondeur préférée n'existent que dans
    /// `CableFormat<n>`. Un écart entre ce que le registre porte et ce que le pilote sert
    /// est d'ailleurs un renseignement — le câble n'a pas redémarré depuis la dernière
    /// écriture — et il se journalise, il ne se corrige pas.
    ///
    /// # Erreurs
    ///
    /// [`ErreurDevnode::Localisation`] ou [`ErreurDevnode::CleMaterielle`] : le
    /// périphérique ou sa clé sont hors d'atteinte. Aucune lecture de valeur n'échoue.
    pub fn lire_formats(instance: &str) -> Result<[u32; CABLE_MAX as usize], ErreurDevnode> {
        let devinst = localiser(instance, CM_LOCATE_DEVNODE_NORMAL)?;
        let cle = ouvrir_cle_materielle(devinst, instance)?;
        let mut formats = [0u32; CABLE_MAX as usize];
        for (index, place) in formats.iter_mut().enumerate() {
            let index = u32::try_from(index).unwrap_or(CABLE_MAX);
            if let Some(valeur) = super::nom_de_valeur(index) {
                *place = lire_dword(&cle, valeur);
            }
        }
        Ok(formats)
    }
}

#[cfg(windows)]
pub use windows::{appliquer, lire_formats};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use conduit_kmd_core::config::CABLE_FORMAT_VALUE_NAMES;

    /// Les seize noms de valeurs, et rien au-delà.
    ///
    /// L'index est celui du **pilote** : `CableFormat0` est le format de « Conduit 1 ».
    /// Le décalage de un ne se fait pas ici, et ce test le dit en n'employant que des
    /// index.
    #[test]
    fn les_noms_de_valeurs_couvrent_les_seize_cables() {
        for index in 0..CABLE_MAX {
            let nom = nom_de_valeur(index).unwrap_or_else(|| panic!("index {index}"));
            assert_eq!(nom, format!("CableFormat{index}"), "index {index}");
            // Le nom vient du contrat partagé, pas d'une composition locale.
            assert_eq!(
                Some(nom),
                CABLE_FORMAT_VALUE_NAMES.get(index as usize).copied(),
                "index {index}"
            );
        }
        // `CableFormat1` n'attrape pas `CableFormat10` à `CableFormat15` : les noms sont
        // deux à deux distincts, ce que le nombre de doublons vérifie d'un coup.
        let mut vus: Vec<&str> = (0..CABLE_MAX).filter_map(nom_de_valeur).collect();
        vus.sort_unstable();
        vus.dedup();
        assert_eq!(vus.len(), CABLE_MAX as usize);

        for au_dela in [CABLE_MAX, CABLE_MAX + 1, u32::MAX] {
            assert_eq!(nom_de_valeur(au_dela), None, "index {au_dela}");
        }
    }

    /// Toutes les causes de refus, en table : le détail est **fidèle** au code du système,
    /// le message n'est jamais vide, et le partage écriture / redémarrage est celui qui
    /// décide du statut.
    #[test]
    fn chaque_cause_porte_son_code_et_dit_quoi_faire() {
        let cas: [(ErreurDevnode, u32, bool); 7] = [
            (ErreurDevnode::Cable { index: 99 }, 99, false),
            (
                ErreurDevnode::Localisation {
                    instance: "ROOT\\MEDIA\\0000".to_owned(),
                    configret: 13,
                },
                13,
                false,
            ),
            (
                ErreurDevnode::CleMaterielle {
                    instance: "ROOT\\MEDIA\\0000".to_owned(),
                    configret: 29,
                },
                29,
                false,
            ),
            (
                ErreurDevnode::Ecriture {
                    valeur: "CableFormat2",
                    code: 5,
                },
                5,
                false,
            ),
            (
                ErreurDevnode::Veto {
                    genre: 5,
                    vetoteur: "audiodg.exe".to_owned(),
                },
                5,
                true,
            ),
            (ErreurDevnode::Retrait { configret: 23 }, 23, true),
            (ErreurDevnode::Retablissement { configret: 13 }, 13, true),
        ];
        for (erreur, detail, ecrite) in &cas {
            assert_eq!(erreur.detail(), *detail, "{erreur:?}");
            assert_eq!(erreur.ecrite(), *ecrite, "{erreur:?}");
            let texte = erreur.to_string();
            assert!(!texte.is_empty(), "{erreur:?}");
            // Le code du système se lit tel quel dans le message : c'est le seul
            // renseignement qui distingue deux échecs de la même variante.
            assert!(texte.contains(&detail.to_string()), "{erreur:?} : {texte}");
        }
    }

    /// **Le partage qui décide du statut**, dit à l'endroit où il se lit.
    ///
    /// Un échec d'écriture dit « rien n'a changé » ; un échec de redémarrage dit
    /// « écrit », et renvoie au prochain démarrage du périphérique. Ce sont les deux
    /// conduites opposées de l'en-tête de module, et elles doivent être lisibles dans le
    /// message même sans le statut.
    #[test]
    fn les_messages_disent_de_quel_cote_de_l_ecriture_on_est() {
        for erreur in [
            ErreurDevnode::Localisation {
                instance: "ROOT\\MEDIA\\0000".to_owned(),
                configret: 13,
            },
            ErreurDevnode::CleMaterielle {
                instance: "ROOT\\MEDIA\\0000".to_owned(),
                configret: 29,
            },
            ErreurDevnode::Ecriture {
                valeur: "CableFormat0",
                code: 5,
            },
        ] {
            let texte = erreur.to_string();
            assert!(!erreur.ecrite(), "{erreur:?}");
            assert!(
                texte.contains("rien n'a") || texte.contains("Rien n'a"),
                "{texte}"
            );
        }

        for erreur in [
            ErreurDevnode::Veto {
                genre: 5,
                vetoteur: "audiodg.exe".to_owned(),
            },
            ErreurDevnode::Retrait { configret: 23 },
            ErreurDevnode::Retablissement { configret: 13 },
        ] {
            let texte = erreur.to_string();
            assert!(erreur.ecrite(), "{erreur:?}");
            assert!(texte.contains("écrit"), "{texte}");
        }

        // Un veto nomme **qui** bloque : c'est tout l'intérêt de
        // `CM_Query_And_Remove_SubTreeW` sur `CM_Disable_DevNode`.
        let veto = ErreurDevnode::Veto {
            genre: 5,
            vetoteur: "audiodg.exe".to_owned(),
        };
        assert!(veto.to_string().contains("audiodg.exe"), "{veto}");
    }
}
