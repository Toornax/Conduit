//! Le protocole **interne** du canal nommé, entre le démon et le service (M1b-20).
//!
//! Fixe, versionné, à trames bornées et longueur préfixée. Tout ce fichier est **pur** :
//! aucun appel au système, aucune allocation hors des `Vec` de sortie, aucune panique.
//! C'est la partie qui se vérifie sans machine virtuelle, et celle que M1b-08 voudra
//! fuzzer.
//!
//! # Ce n'est pas le protocole de `conduit-protocol`
//!
//! `conduit-protocol` est le protocole **utilisateur**, entre le démon et `conduitctl` :
//! MessagePack, extensible, avec négociation de capacités. Celui-ci est autre chose —
//! cinq ordres, deux structures de taille fixe, aucune extensibilité, et un serveur
//! qui tourne en `LocalSystem`. Les mélanger ferait entrer un décodeur générique dans le
//! processus le plus privilégié du produit ; on écrit donc le strict nécessaire, et on
//! le refuse dès qu'il n'a pas exactement la forme attendue.
//!
//! # Boutisme : petit-boutiste, et c'est un choix
//!
//! [`conduit_kmd_core::config::CableState`] est en boutisme **natif** parce qu'elle ne
//! traverse qu'une frontière mémoire (KS recopie un tampon d'un processus à l'autre sur
//! la même machine). Les trames de ce module traversent une frontière de **format** —
//! un canal, un flux d'octets, quelque chose qu'on peut relire dans un journal ou
//! rejouer — donc petit-boutiste explicite, comme le `u32` d'en-tête de
//! `conduit_protocol::framing`.
//!
//! # La règle du parseur : la longueur est exacte, les champs inutilisés sont nuls
//!
//! Une requête fait [`TAILLE_REQUETE`] octets, une réponse [`TAILLE_REPONSE`], et **rien
//! d'autre n'est accepté** : ni plus court, ni plus long, ni un préfixe valide suivi
//! d'octets en trop. C'est la règle de [`conduit_kmd_core::config::CableState::from_bytes`],
//! pour les mêmes raisons — accepter un préfixe rend le format non extensible et c'est le
//! premier écart qu'un fuzzer trouve.
//!
//! Elle va plus loin ici : les champs qu'un ordre **n'utilise pas** sont exigés nuls
//! ([`ErreurRequete::ChampParasite`]). Un `Lister` qui porterait un numéro de câble est
//! un client qui s'est trompé d'ordre ou un octet retourné en transit ; le laisser passer
//! reviendrait à servir une requête qu'on n'a pas comprise.

use conduit_backend::CableId;
use conduit_kmd_core::config::CABLE_MAX;
use conduit_kmd_core::params::{DEFAULT_CHANNELS, MAX_CHANNELS, MIN_CHANNELS};
use core::fmt;

// ---------------------------------------------------------------------------------
// Les constantes du format.
// ---------------------------------------------------------------------------------

/// Version de **ce** protocole (démon ↔ service), à ne pas confondre avec
/// [`conduit_kmd_core::config::CONFIG_VERSION`], celle du contrat KS (service ↔ pilote).
///
/// Les deux se déplacent indépendamment : on peut changer la forme d'une trame sans
/// toucher au pilote, et l'inverse. Un client d'une autre version reçoit
/// [`Statut::VersionInconnue`] et la version servie dans [`Reponse::detail`] — un refus
/// qui **dit quoi faire**, là où une trame mal comprise donnerait un octet aberrant.
pub const PROTOCOLE_VERSION: u8 = 1;

/// Taille de l'en-tête de trame : un `u32` petit-boutiste, la longueur de la charge.
pub const EN_TETE_OCTETS: usize = 4;

/// Taille d'une requête : **8 octets**, toujours.
pub const TAILLE_REQUETE: usize = 8;

/// Taille d'une réponse : **28 octets**, toujours.
///
/// Une réponse de taille fixe est la propriété de sécurité la moins chère qui soit : le
/// serveur n'a jamais à décider d'une longueur à partir de ce qu'il a reçu.
pub const TAILLE_REPONSE: usize = 28;

/// Charge utile maximale acceptée avant toute allocation : **64 octets**.
///
/// Généreuse par rapport aux 8 et 28 octets réellement utilisés — de quoi absorber une
/// v2 un peu plus large sans rouvrir ce fichier — et assez petite pour qu'un client
/// hostile ne puisse rien faire réserver au service. La longueur annoncée est confrontée
/// à cette borne **avant** que le moindre tampon ne soit alloué
/// ([`longueur_annoncee`]).
pub const MAX_TRAME_OCTETS: usize = 64;

// Les deux structures tiennent dans la borne, avec de la marge.
const _: () = assert!(TAILLE_REQUETE <= MAX_TRAME_OCTETS);
const _: () = assert!(TAILLE_REPONSE <= MAX_TRAME_OCTETS);

/// Nom du canal nommé du service.
///
/// Fixe et sans numéro de version : c'est le protocole qui porte la sienne, et un client
/// d'une autre version doit pouvoir obtenir un refus explicite plutôt qu'un
/// `ERROR_FILE_NOT_FOUND` qui ressemblerait à « le service n'est pas installé ».
pub const NOM_TUBE: &str = r"\\.\pipe\conduit-helper";

// ---------------------------------------------------------------------------------
// Les ordres.
// ---------------------------------------------------------------------------------

/// Code de l'ordre `version`, dans la trame.
pub const ORDRE_VERSION: u8 = 0;
/// Code de l'ordre `lister`.
pub const ORDRE_LISTER: u8 = 1;
/// Code de l'ordre `activer`.
pub const ORDRE_ACTIVER: u8 = 2;
/// Code de l'ordre `désactiver`.
pub const ORDRE_DESACTIVER: u8 = 3;
/// Code de l'ordre `canaux`.
pub const ORDRE_CANAUX: u8 = 4;

/// Une requête du démon au service : l'interface **fixe** du canal nommé.
///
/// Cinq ordres, et jamais un sixième sans changer [`PROTOCOLE_VERSION`]. Deux ne
/// modifient rien ([`Self::Version`], [`Self::Lister`]) ; les trois autres écrivent dans
/// le pilote et sont donc journalisés avec l'identité de l'appelant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Requete {
    /// La version de ce protocole et celle du contrat KS servi par le pilote.
    ///
    /// C'est l'ordre qu'un client émet en premier : il ne touche à rien, ne demande
    /// aucun câble, et dit si la suite a un sens.
    Version,
    /// L'état de tous les câbles : lesquels existent, lesquels sont connectés.
    Lister,
    /// Connecte le câble : ses deux endpoints apparaissent.
    Activer(CableId),
    /// Déconnecte le câble : ses deux endpoints se rangent sous « Périphériques
    /// déconnectés ».
    Desactiver(CableId),
    /// Règle le nombre de canaux du câble.
    ///
    /// **Validé ici, pas appliqué par le pilote** : voir [`Statut::CanauxNonApplicables`].
    Canaux {
        /// Le câble visé.
        cable: CableId,
        /// Le nombre de canaux voulu, dans `MIN_CHANNELS..=MAX_CHANNELS`.
        canaux: u32,
    },
}

impl Requete {
    /// Le code d'ordre de cette requête, tel qu'il voyage dans la trame.
    #[must_use]
    pub const fn code(&self) -> u8 {
        match self {
            Self::Version => ORDRE_VERSION,
            Self::Lister => ORDRE_LISTER,
            Self::Activer(_) => ORDRE_ACTIVER,
            Self::Desactiver(_) => ORDRE_DESACTIVER,
            Self::Canaux { .. } => ORDRE_CANAUX,
        }
    }

    /// Le nom de l'ordre, en français, pour le journal.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Lister => "lister",
            Self::Activer(_) => "activer",
            Self::Desactiver(_) => "désactiver",
            Self::Canaux { .. } => "canaux",
        }
    }

    /// Cette requête **modifie-t-elle** l'état de la machine ?
    ///
    /// C'est ce prédicat qui décide de deux choses : armer ou non
    /// `SeLoadDriverPrivilege` (un privilège armé pour une lecture est une surface
    /// offerte pour rien), et journaliser ou non l'identité de l'appelant.
    #[must_use]
    pub const fn modifie(&self) -> bool {
        match self {
            Self::Version | Self::Lister => false,
            Self::Activer(_) | Self::Desactiver(_) | Self::Canaux { .. } => true,
        }
    }

    /// Le câble visé, s'il y en a un.
    #[must_use]
    pub const fn cable(&self) -> Option<CableId> {
        match self {
            Self::Version | Self::Lister => None,
            Self::Activer(cable) | Self::Desactiver(cable) => Some(*cable),
            Self::Canaux { cable, .. } => Some(*cable),
        }
    }

    /// Sérialise la requête en [`TAILLE_REQUETE`] octets.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; TAILLE_REQUETE] {
        let (cable, canaux) = match self {
            Self::Version | Self::Lister => (0_u16, 0_u16),
            Self::Activer(c) | Self::Desactiver(c) => (c.0 as u16, 0),
            Self::Canaux { cable, canaux } => (cable.0 as u16, *canaux as u16),
        };
        let cable = cable.to_le_bytes();
        let canaux = canaux.to_le_bytes();
        [
            PROTOCOLE_VERSION,
            self.code(),
            cable[0],
            cable[1],
            canaux[0],
            canaux[1],
            0,
            0,
        ]
    }

    /// Sérialise la requête, en-tête de trame compris.
    #[must_use]
    pub fn encadrer(&self) -> Vec<u8> {
        encadrer(&self.to_bytes())
    }

    /// **Le** parseur des requêtes : des octets hostiles vers un ordre valide, ou une
    /// cause de refus.
    ///
    /// L'ordre des contrôles est celui des champs — longueur, version, ordre, puis les
    /// champs de cet ordre — pour que la cause nommée soit la **première** anomalie et
    /// non une arbitraire.
    ///
    /// # Erreurs
    ///
    /// [`ErreurRequete`], dont chaque variante porte ce qui a été trouvé.
    pub fn from_bytes(octets: &[u8]) -> Result<Self, ErreurRequete> {
        let brut: [u8; TAILLE_REQUETE] =
            octets.try_into().map_err(|_| ErreurRequete::Longueur {
                recus: octets.len(),
            })?;
        let version = brut[0];
        if version != PROTOCOLE_VERSION {
            return Err(ErreurRequete::Version { trouvee: version });
        }
        let ordre = brut[1];
        let cable = u16::from_le_bytes([brut[2], brut[3]]);
        let canaux = u16::from_le_bytes([brut[4], brut[5]]);
        let reserve = u16::from_le_bytes([brut[6], brut[7]]);
        if reserve != 0 {
            return Err(ErreurRequete::Reserve { brut: reserve });
        }
        match ordre {
            ORDRE_VERSION | ORDRE_LISTER => {
                // Aucun champ n'est utilisé : les deux sont exigés nuls.
                exiger_nul("câble", cable)?;
                exiger_nul("canaux", canaux)?;
                Ok(if ordre == ORDRE_VERSION {
                    Self::Version
                } else {
                    Self::Lister
                })
            }
            ORDRE_ACTIVER | ORDRE_DESACTIVER => {
                let cable = valider_cable(cable)?;
                exiger_nul("canaux", canaux)?;
                Ok(if ordre == ORDRE_ACTIVER {
                    Self::Activer(cable)
                } else {
                    Self::Desactiver(cable)
                })
            }
            ORDRE_CANAUX => {
                let cable = valider_cable(cable)?;
                let canaux = valider_canaux(canaux)?;
                Ok(Self::Canaux { cable, canaux })
            }
            code => Err(ErreurRequete::Ordre { code }),
        }
    }
}

/// Refuse un champ que l'ordre n'utilise pas et qui n'est pas nul.
const fn exiger_nul(champ: &'static str, brut: u16) -> Result<(), ErreurRequete> {
    if brut == 0 {
        Ok(())
    } else {
        Err(ErreurRequete::ChampParasite { champ, brut })
    }
}

/// Valide un numéro de câble **affiché** : 1 à [`CABLE_MAX`].
///
/// Le domaine vient du contrat partagé avec le pilote, pas d'un 16 recopié. Le décalage
/// de un (« Conduit 1 » = index pilote 0) ne se fait **pas** ici : il appartient à
/// `conduit_backend_wasapi::cable::driver_index`, seul endroit du dépôt qui le connaisse.
const fn valider_cable(brut: u16) -> Result<CableId, ErreurRequete> {
    let numero = brut as u32;
    if numero == 0 || numero > CABLE_MAX {
        return Err(ErreurRequete::Cable { brut });
    }
    Ok(CableId(numero))
}

/// Valide un nombre de canaux contre les bornes de [`conduit_kmd_core::params`].
const fn valider_canaux(brut: u16) -> Result<u32, ErreurRequete> {
    let canaux = brut as u32;
    if canaux < MIN_CHANNELS || canaux > MAX_CHANNELS {
        return Err(ErreurRequete::Canaux { brut });
    }
    Ok(canaux)
}

/// Ce qui a fait refuser une requête : un cas, une cause, une ligne de journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurRequete {
    /// Longueur différente de [`TAILLE_REQUETE`] — plus courte **ou** plus longue.
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// Version de protocole que ce service ne sert pas.
    Version {
        /// La version annoncée par le client.
        trouvee: u8,
    },
    /// Code d'ordre inconnu.
    Ordre {
        /// Le code reçu.
        code: u8,
    },
    /// Numéro de câble hors de 1 à [`CABLE_MAX`].
    Cable {
        /// Le numéro reçu.
        brut: u16,
    },
    /// Nombre de canaux hors des bornes du pilote.
    Canaux {
        /// Le nombre reçu.
        brut: u16,
    },
    /// Un champ que cet ordre n'utilise pas, et qui n'est pas nul.
    ChampParasite {
        /// Le nom du champ, en français.
        champ: &'static str,
        /// La valeur trouvée.
        brut: u16,
    },
    /// Le champ réservé de la requête n'est pas nul.
    Reserve {
        /// La valeur trouvée.
        brut: u16,
    },
}

impl ErreurRequete {
    /// Le statut à renvoyer au client pour cette cause.
    #[must_use]
    pub const fn statut(&self) -> Statut {
        match self {
            Self::Version { .. } => Statut::VersionInconnue,
            Self::Ordre { .. } => Statut::OrdreInconnu,
            Self::Cable { .. } => Statut::CableInconnu,
            Self::Canaux { .. } => Statut::CanauxInvalides,
            Self::Longueur { .. } | Self::ChampParasite { .. } | Self::Reserve { .. } => {
                Statut::TrameInvalide
            }
        }
    }
}

impl fmt::Display for ErreurRequete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "requête de {recus} octets, {TAILLE_REQUETE} attendus exactement"
            ),
            Self::Version { trouvee } => write!(
                f,
                "protocole version {trouvee} : ce service sert la version \
                 {PROTOCOLE_VERSION}"
            ),
            Self::Ordre { code } => write!(f, "ordre {code} inconnu (0 à {ORDRE_CANAUX})"),
            Self::Cable { brut } => {
                write!(
                    f,
                    "câble {brut} inconnu : les câbles vont de 1 à {CABLE_MAX}"
                )
            }
            Self::Canaux { brut } => {
                write!(f, "{brut} canaux hors de {MIN_CHANNELS} à {MAX_CHANNELS}")
            }
            Self::ChampParasite { champ, brut } => write!(
                f,
                "champ « {champ} » = {brut} alors que cet ordre ne l'utilise pas"
            ),
            Self::Reserve { brut } => write!(f, "champ réservé non nul ({brut:#06x})"),
        }
    }
}

impl std::error::Error for ErreurRequete {}

// ---------------------------------------------------------------------------------
// Les statuts.
// ---------------------------------------------------------------------------------

/// L'issue d'un ordre, telle qu'elle voyage dans [`Reponse::statut`].
///
/// Chaque variante correspond à **une conduite différente** pour le client : changer de
/// version, corriger sa trame, installer le pilote, changer de compte. « Erreur » sans
/// autre précision ne servirait à rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Statut {
    /// L'ordre a été exécuté.
    Succes,
    /// Le client parle une autre version du protocole ; [`Reponse::detail`] porte celle
    /// que ce service sert.
    VersionInconnue,
    /// La trame n'a pas la forme attendue : longueur, champ réservé, champ parasite.
    TrameInvalide,
    /// Code d'ordre inconnu.
    OrdreInconnu,
    /// Numéro de câble hors des câbles adressables.
    CableInconnu,
    /// Nombre de canaux hors des bornes du pilote.
    CanauxInvalides,
    /// Nombre de canaux valide, mais que le pilote ne sait pas encore appliquer.
    ///
    /// **C'est l'état réel de M1b-20, pas un contournement.** Le nombre de canaux est
    /// scellé dans les tables KS du pilote (`descriptors::CHANNELS`) et le gestionnaire
    /// de propriété refuse toute valeur différente de
    /// [`conduit_kmd_core::params::DEFAULT_CHANNELS`] tant que **M1b-05** n'a pas rendu
    /// la valeur dynamique. Le service le dit ici, avec le mot « M1b-05 » dans le
    /// message, plutôt que de rendre un succès pour un réglage qui n'agirait sur rien —
    /// ce qui serait pire qu'un refus. Ce statut **disparaîtra avec M1b-05** : c'est
    /// alors le pilote qui tranchera.
    CanauxNonApplicables,
    /// Aucune interface `KSCATEGORY_TOPOLOGY` de ce câble : le pilote Conduit n'est pas
    /// chargé, ou ce câble n'est pas enregistré.
    PiloteAbsent,
    /// `SeLoadDriverPrivilege` n'a pas pu être armé dans le jeton du service.
    ///
    /// Ne devrait pas arriver en `LocalSystem`, qui le détient : c'est le signe que le
    /// service tourne sous un autre compte.
    PrivilegeAbsent,
    /// Le système a refusé ; [`Reponse::detail`] porte le code Win32 **tel quel**.
    ErreurSysteme,
}

impl Statut {
    /// Tous les statuts, dans l'ordre de leur code.
    pub const ALL: [Self; 10] = [
        Self::Succes,
        Self::VersionInconnue,
        Self::TrameInvalide,
        Self::OrdreInconnu,
        Self::CableInconnu,
        Self::CanauxInvalides,
        Self::CanauxNonApplicables,
        Self::PiloteAbsent,
        Self::PrivilegeAbsent,
        Self::ErreurSysteme,
    ];

    /// Le code de ce statut, tel qu'il voyage dans la trame.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Succes => 0,
            Self::VersionInconnue => 1,
            Self::TrameInvalide => 2,
            Self::OrdreInconnu => 3,
            Self::CableInconnu => 4,
            Self::CanauxInvalides => 5,
            Self::CanauxNonApplicables => 6,
            Self::PiloteAbsent => 7,
            Self::PrivilegeAbsent => 8,
            Self::ErreurSysteme => 9,
        }
    }

    /// Le statut de code `code`, ou `None` : un code inconnu n'est **pas** replié sur
    /// « erreur », sinon un service d'une version future ferait croire à un échec
    /// générique là où il y a une inadéquation de version.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Succes,
            1 => Self::VersionInconnue,
            2 => Self::TrameInvalide,
            3 => Self::OrdreInconnu,
            4 => Self::CableInconnu,
            5 => Self::CanauxInvalides,
            6 => Self::CanauxNonApplicables,
            7 => Self::PiloteAbsent,
            8 => Self::PrivilegeAbsent,
            9 => Self::ErreurSysteme,
            _ => return None,
        })
    }

    /// L'ordre a-t-il abouti ?
    #[must_use]
    pub const fn succes(self) -> bool {
        matches!(self, Self::Succes)
    }
}

impl fmt::Display for Statut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let texte = match self {
            Self::Succes => "succès",
            Self::VersionInconnue => "version de protocole non servie par ce service",
            Self::TrameInvalide => "trame invalide",
            Self::OrdreInconnu => "ordre inconnu",
            Self::CableInconnu => "câble inconnu",
            Self::CanauxInvalides => "nombre de canaux hors bornes",
            Self::CanauxNonApplicables => {
                "nombre de canaux valide mais que le pilote ne sait pas encore appliquer \
                 (M1b-05) : il n'accepte aujourd'hui que la valeur par défaut"
            }
            Self::PiloteAbsent => "pilote Conduit absent ou câble non enregistré",
            Self::PrivilegeAbsent => {
                "SeLoadDriverPrivilege impossible à armer : le service ne tourne pas en \
                 LocalSystem"
            }
            Self::ErreurSysteme => "refus du système",
        };
        f.write_str(texte)
    }
}

// ---------------------------------------------------------------------------------
// Les réponses.
// ---------------------------------------------------------------------------------

/// La réponse du service : **toujours** [`TAILLE_REPONSE`] octets, quel que soit l'ordre
/// et quelle que soit l'issue.
///
/// Elle porte l'état complet des câbles même sur un refus, ce qui évite au client de
/// devoir enchaîner un `lister` derrière chaque `activer` — et rend le journal du client
/// lisible sans corrélation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reponse {
    /// Écho du code d'ordre **reçu**, tel quel — y compris quand il est inconnu.
    pub ordre: u8,
    /// L'issue.
    pub statut: Statut,
    /// Le renseignement chiffré qui accompagne le statut : code Win32 pour
    /// [`Statut::ErreurSysteme`], version servie pour [`Statut::VersionInconnue`], 0
    /// sinon.
    pub detail: u32,
    /// Masque des câbles dont le filtre de topologie **existe** sur la machine (bit `n`
    /// = « Conduit *n+1* »), même disposition que
    /// [`conduit_kmd_core::config::ACTIVE_CABLES_MASK`].
    pub presents: u32,
    /// Masque des câbles **connectés**.
    pub actifs: u32,
    /// Version du contrat KS servie par le pilote
    /// ([`conduit_kmd_core::config::CONFIG_VERSION`] côté service), 0 si elle n'a pas pu
    /// être lue.
    pub version_ks: u32,
    /// Nombre de canaux du câble visé, 0 quand l'ordre ne vise aucun câble.
    pub canaux: u32,
}

impl Reponse {
    /// Une réponse vide, de statut `statut`, en écho à l'ordre `ordre`.
    #[must_use]
    pub const fn refus(ordre: u8, statut: Statut) -> Self {
        Self {
            ordre,
            statut,
            detail: 0,
            presents: 0,
            actifs: 0,
            version_ks: 0,
            canaux: 0,
        }
    }

    /// La même, avec un détail chiffré.
    #[must_use]
    pub const fn refus_detaille(ordre: u8, statut: Statut, detail: u32) -> Self {
        Self {
            detail,
            ..Self::refus(ordre, statut)
        }
    }

    /// Le câble `cable` (numéro **affiché**) est-il connecté d'après [`Self::actifs`] ?
    ///
    /// Rend `false` hors des câbles adressables : un numéro qui n'existe pas n'est
    /// jamais connecté. Le décalage de un est fait ici pour que le client n'ait pas à
    /// le refaire.
    #[must_use]
    pub const fn est_actif(&self, cable: CableId) -> bool {
        conduit_kmd_core::config::is_active(self.actifs, cable.0.wrapping_sub(1))
    }

    /// Le câble `cable` est-il enregistré par le pilote sur cette machine ?
    #[must_use]
    pub const fn est_present(&self, cable: CableId) -> bool {
        conduit_kmd_core::config::is_active(self.presents, cable.0.wrapping_sub(1))
    }

    /// Sérialise la réponse en [`TAILLE_REPONSE`] octets.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; TAILLE_REPONSE] {
        let detail = self.detail.to_le_bytes();
        let presents = self.presents.to_le_bytes();
        let actifs = self.actifs.to_le_bytes();
        let version_ks = self.version_ks.to_le_bytes();
        let canaux = self.canaux.to_le_bytes();
        [
            PROTOCOLE_VERSION,
            self.ordre,
            self.statut.code(),
            0,
            detail[0],
            detail[1],
            detail[2],
            detail[3],
            presents[0],
            presents[1],
            presents[2],
            presents[3],
            actifs[0],
            actifs[1],
            actifs[2],
            actifs[3],
            version_ks[0],
            version_ks[1],
            version_ks[2],
            version_ks[3],
            canaux[0],
            canaux[1],
            canaux[2],
            canaux[3],
            0,
            0,
            0,
            0,
        ]
    }

    /// Sérialise la réponse, en-tête de trame compris.
    #[must_use]
    pub fn encadrer(&self) -> Vec<u8> {
        encadrer(&self.to_bytes())
    }

    /// **Le** parseur des réponses, côté client : mêmes règles qu'à l'aller.
    ///
    /// Le client d'un service `LocalSystem` n'a pas de raison de se méfier autant que le
    /// serveur — mais rien ne garantit que le canal soit tenu par notre service (un
    /// canal nommé se squatte, c'est même la raison du descripteur de sécurité du module
    /// `securite`). La réponse est donc analysée avec la même sévérité.
    ///
    /// # Erreurs
    ///
    /// [`ErreurReponse`], dont chaque variante porte ce qui a été trouvé.
    pub fn from_bytes(octets: &[u8]) -> Result<Self, ErreurReponse> {
        let brut: [u8; TAILLE_REPONSE] =
            octets.try_into().map_err(|_| ErreurReponse::Longueur {
                recus: octets.len(),
            })?;
        let version = brut[0];
        if version != PROTOCOLE_VERSION {
            return Err(ErreurReponse::Version { trouvee: version });
        }
        if brut[3] != 0 {
            return Err(ErreurReponse::Reserve { brut: brut[3] });
        }
        let statut = Statut::from_code(brut[2]).ok_or(ErreurReponse::Statut { code: brut[2] })?;
        let reserve2 = mot(&brut, 24);
        if reserve2 != 0 {
            return Err(ErreurReponse::Reserve2 { brut: reserve2 });
        }
        Ok(Self {
            ordre: brut[1],
            statut,
            detail: mot(&brut, 4),
            presents: mot(&brut, 8),
            actifs: mot(&brut, 12),
            version_ks: mot(&brut, 16),
            canaux: mot(&brut, 20),
        })
    }
}

/// Lit un `u32` petit-boutiste au décalage `offset` d'une réponse.
///
/// Les quatre octets sont **recopiés** puis interprétés, jamais transtypés ; `offset`
/// est toujours un décalage nommé de ce fichier, donc dans les bornes, et le repli à
/// zéro n'est là que pour interdire la panique.
const fn mot(brut: &[u8; TAILLE_REPONSE], offset: usize) -> u32 {
    match offset.checked_add(4) {
        Some(fin) if fin <= TAILLE_REPONSE => u32::from_le_bytes([
            brut[offset],
            brut[offset.wrapping_add(1)],
            brut[offset.wrapping_add(2)],
            brut[offset.wrapping_add(3)],
        ]),
        _ => 0,
    }
}

/// Ce qui a fait refuser une réponse, côté client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurReponse {
    /// Longueur différente de [`TAILLE_REPONSE`].
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// Version de protocole inattendue.
    Version {
        /// La version annoncée.
        trouvee: u8,
    },
    /// Code de statut inconnu.
    Statut {
        /// Le code reçu.
        code: u8,
    },
    /// Le premier champ réservé n'est pas nul.
    Reserve {
        /// La valeur trouvée.
        brut: u8,
    },
    /// Le second champ réservé n'est pas nul.
    Reserve2 {
        /// La valeur trouvée.
        brut: u32,
    },
}

impl fmt::Display for ErreurReponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "réponse de {recus} octets, {TAILLE_REPONSE} attendus exactement"
            ),
            Self::Version { trouvee } => write!(
                f,
                "réponse en protocole version {trouvee}, {PROTOCOLE_VERSION} attendue"
            ),
            Self::Statut { code } => write!(f, "statut {code} inconnu"),
            Self::Reserve { brut } => write!(f, "champ réservé non nul ({brut:#04x})"),
            Self::Reserve2 { brut } => write!(f, "second champ réservé non nul ({brut:#010x})"),
        }
    }
}

impl std::error::Error for ErreurReponse {}

// ---------------------------------------------------------------------------------
// Le cadrage.
// ---------------------------------------------------------------------------------

/// Ce qui a fait refuser un en-tête de trame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurTrame {
    /// Longueur annoncée au-delà de [`MAX_TRAME_OCTETS`] : refusée **avant** toute
    /// allocation.
    TropGrande {
        /// La longueur annoncée.
        annoncee: usize,
        /// La borne.
        maximum: usize,
    },
    /// Longueur annoncée nulle : une trame vide n'existe pas dans ce protocole.
    Vide,
}

impl fmt::Display for ErreurTrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TropGrande { annoncee, maximum } => {
                write!(f, "trame de {annoncee} octets annoncée : maximum {maximum}")
            }
            Self::Vide => f.write_str("trame de longueur nulle"),
        }
    }
}

impl std::error::Error for ErreurTrame {}

/// Préfixe `charge` de sa longueur (`u32` petit-boutiste).
///
/// La charge vient toujours d'un `to_bytes` de ce module, donc sa longueur est connue à
/// la compilation et tient dans un `u32` : la conversion ne peut pas échouer, et le
/// repli sur `u32::MAX` n'est là que pour interdire la panique.
#[must_use]
pub fn encadrer(charge: &[u8]) -> Vec<u8> {
    let longueur = u32::try_from(charge.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(EN_TETE_OCTETS.saturating_add(charge.len()));
    out.extend_from_slice(&longueur.to_le_bytes());
    out.extend_from_slice(charge);
    out
}

/// Décode un en-tête de trame et rend la longueur de la charge qui suit, **bornée**.
///
/// C'est le seul endroit où un nombre venu du réseau devient une taille de tampon. La
/// borne est vérifiée ici, avant l'allocation, et non après la lecture : un serveur qui
/// réserverait d'abord et vérifierait ensuite offrirait à tout membre du groupe
/// `INTERACTIVE` de faire réserver 4 Gio au processus `LocalSystem` avec quatre octets.
///
/// # Erreurs
///
/// [`ErreurTrame::TropGrande`] ou [`ErreurTrame::Vide`].
pub const fn longueur_annoncee(en_tete: [u8; EN_TETE_OCTETS]) -> Result<usize, ErreurTrame> {
    let annoncee = u32::from_le_bytes(en_tete) as usize;
    if annoncee == 0 {
        return Err(ErreurTrame::Vide);
    }
    if annoncee > MAX_TRAME_OCTETS {
        return Err(ErreurTrame::TropGrande {
            annoncee,
            maximum: MAX_TRAME_OCTETS,
        });
    }
    Ok(annoncee)
}

/// Découpe la **première** trame complète de `tampon` : rend la charge et le nombre
/// d'octets consommés, ou `None` s'il en manque encore.
///
/// Fonction pure et sans allocation, séparée de toute entrée-sortie : c'est elle que les
/// tables de cas éprouvent, et c'est elle que M1b-08 fuzzera. Le serveur, lui, connaît la
/// taille de ce qu'il attend et lit en deux temps ([`longueur_annoncee`]).
///
/// # Erreurs
///
/// [`ErreurTrame`] si l'en-tête annonce une longueur nulle ou hors borne.
pub fn decouper(tampon: &[u8]) -> Result<Option<(&[u8], usize)>, ErreurTrame> {
    let Some(en_tete) = tampon.get(..EN_TETE_OCTETS) else {
        return Ok(None);
    };
    let en_tete: [u8; EN_TETE_OCTETS] = en_tete.try_into().unwrap_or([0; EN_TETE_OCTETS]);
    let longueur = longueur_annoncee(en_tete)?;
    let fin = EN_TETE_OCTETS.saturating_add(longueur);
    match tampon.get(EN_TETE_OCTETS..fin) {
        Some(charge) => Ok(Some((charge, fin))),
        None => Ok(None),
    }
}

/// Le nombre de canaux que le pilote sait **servir** aujourd'hui.
///
/// Ré-exporté depuis le contrat partagé plutôt que recopié : c'est la valeur contre
/// laquelle [`Statut::CanauxNonApplicables`] se décide, et elle doit bouger avec M1b-05
/// sans que ce fichier soit rouvert.
pub const CANAUX_APPLICABLES: u32 = DEFAULT_CHANNELS;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Une requête brute, champ par champ, sans passer par le sérialiseur : c'est ce qui
    /// permet de fabriquer des trames que `Requete::to_bytes` ne produirait jamais.
    fn brut(version: u8, ordre: u8, cable: u16, canaux: u16, reserve: u16) -> [u8; 8] {
        let c = cable.to_le_bytes();
        let n = canaux.to_le_bytes();
        let r = reserve.to_le_bytes();
        [version, ordre, c[0], c[1], n[0], n[1], r[0], r[1]]
    }

    /// Les cinq ordres font l'aller-retour, et le code d'ordre est celui qu'on a gravé.
    #[test]
    fn les_cinq_ordres_font_l_aller_retour() {
        let cas = [
            (Requete::Version, ORDRE_VERSION),
            (Requete::Lister, ORDRE_LISTER),
            (Requete::Activer(CableId(1)), ORDRE_ACTIVER),
            (Requete::Desactiver(CableId(16)), ORDRE_DESACTIVER),
            (
                Requete::Canaux {
                    cable: CableId(3),
                    canaux: 2,
                },
                ORDRE_CANAUX,
            ),
        ];
        for (requete, code) in cas {
            let octets = requete.to_bytes();
            assert_eq!(octets.len(), TAILLE_REQUETE);
            assert_eq!(octets[0], PROTOCOLE_VERSION, "{requete:?}");
            assert_eq!(octets[1], code, "{requete:?}");
            assert_eq!(requete.code(), code);
            assert_eq!(Requete::from_bytes(&octets), Ok(requete), "{requete:?}");
        }
        // Les codes sont deux à deux distincts et contigus depuis 0.
        let codes: Vec<u8> = cas.iter().map(|(_, c)| *c).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4]);
    }

    /// Table des longueurs : exacte, tronquée, allongée, vide.
    ///
    /// L'allongement est le cas qui compte — un préfixe parfaitement valide suivi d'un
    /// octet en trop doit être refusé, sinon le format n'est plus extensible.
    #[test]
    fn longueur_table() {
        let complet = Requete::Lister.to_bytes();
        assert!(Requete::from_bytes(&complet).is_ok());

        assert_eq!(
            Requete::from_bytes(&complet[..7]),
            Err(ErreurRequete::Longueur { recus: 7 })
        );
        let mut trop_long = complet.to_vec();
        trop_long.push(0);
        assert_eq!(
            Requete::from_bytes(&trop_long),
            Err(ErreurRequete::Longueur { recus: 9 })
        );
        // L'octet en trop n'a même pas besoin d'être nul.
        let mut trop_long = complet.to_vec();
        trop_long.push(0xFF);
        assert_eq!(
            Requete::from_bytes(&trop_long),
            Err(ErreurRequete::Longueur { recus: 9 })
        );

        // Toutes les longueurs de 0 à 24 sauf la bonne sont refusées.
        for taille in 0..=24usize {
            let tampon = vec![0u8; taille];
            let resultat = Requete::from_bytes(&tampon);
            if taille == TAILLE_REQUETE {
                // Huit octets nuls : la version 0 n'est pas la nôtre, c'est elle qui
                // refuse — pas la longueur. C'est ce qui distingue les deux contrôles.
                assert_eq!(resultat, Err(ErreurRequete::Version { trouvee: 0 }));
            } else {
                assert_eq!(
                    resultat,
                    Err(ErreurRequete::Longueur { recus: taille }),
                    "taille {taille}"
                );
            }
        }
    }

    /// Table de la version : la nôtre passe, toutes les autres sont refusées, et le
    /// refus est le **premier** contrôle après la longueur.
    #[test]
    fn version_table() {
        for version in 0..=u8::MAX {
            let trame = brut(version, ORDRE_LISTER, 0, 0, 0);
            let resultat = Requete::from_bytes(&trame);
            if version == PROTOCOLE_VERSION {
                assert_eq!(resultat, Ok(Requete::Lister));
            } else {
                assert_eq!(resultat, Err(ErreurRequete::Version { trouvee: version }));
            }
        }
        // Une version étrangère est signalée même quand tout le reste est aberrant :
        // c'est le renseignement le plus utile au client.
        let trame = brut(9, 200, 999, 999, 7);
        assert_eq!(
            Requete::from_bytes(&trame),
            Err(ErreurRequete::Version { trouvee: 9 })
        );
    }

    /// Table des codes d'ordre : les cinq connus, tous les autres refusés.
    #[test]
    fn ordre_table() {
        for code in 0..=u8::MAX {
            let trame = brut(PROTOCOLE_VERSION, code, 0, 0, 0);
            let resultat = Requete::from_bytes(&trame);
            match code {
                ORDRE_VERSION => assert_eq!(resultat, Ok(Requete::Version)),
                ORDRE_LISTER => assert_eq!(resultat, Ok(Requete::Lister)),
                // Les ordres qui visent un câble refusent le câble 0, pas l'ordre.
                ORDRE_ACTIVER | ORDRE_DESACTIVER | ORDRE_CANAUX => {
                    assert_eq!(
                        resultat,
                        Err(ErreurRequete::Cable { brut: 0 }),
                        "code {code}"
                    );
                }
                _ => assert_eq!(resultat, Err(ErreurRequete::Ordre { code }), "code {code}"),
            }
        }
    }

    /// Table du numéro de câble : 1 à `CABLE_MAX`, et rien d'autre. Le domaine vient du
    /// contrat, pas d'un 16 recopié.
    #[test]
    fn cable_table() {
        for ordre in [ORDRE_ACTIVER, ORDRE_DESACTIVER] {
            for numero in 0..=(CABLE_MAX as u16).saturating_add(2) {
                let trame = brut(PROTOCOLE_VERSION, ordre, numero, 0, 0);
                let resultat = Requete::from_bytes(&trame);
                if numero >= 1 && u32::from(numero) <= CABLE_MAX {
                    assert_eq!(
                        resultat.map(|r| r.cable()),
                        Ok(Some(CableId(u32::from(numero)))),
                        "câble {numero}"
                    );
                } else {
                    assert_eq!(
                        resultat,
                        Err(ErreurRequete::Cable { brut: numero }),
                        "câble {numero}"
                    );
                }
            }
        }
        // Les extrêmes du champ, qui ne peuvent pas venir d'un client honnête.
        for numero in [17, 100, u16::MAX] {
            let trame = brut(PROTOCOLE_VERSION, ORDRE_ACTIVER, numero, 0, 0);
            assert_eq!(
                Requete::from_bytes(&trame),
                Err(ErreurRequete::Cable { brut: numero })
            );
        }
    }

    /// Table des canaux : les bornes de `params`, ni plus ni moins.
    #[test]
    fn canaux_table() {
        for canaux in 0..=(MAX_CHANNELS as u16).saturating_add(2) {
            let trame = brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, canaux, 0);
            let resultat = Requete::from_bytes(&trame);
            let attendu = u32::from(canaux) >= MIN_CHANNELS && u32::from(canaux) <= MAX_CHANNELS;
            if attendu {
                assert_eq!(
                    resultat,
                    Ok(Requete::Canaux {
                        cable: CableId(1),
                        canaux: u32::from(canaux)
                    }),
                    "canaux {canaux}"
                );
            } else {
                assert_eq!(
                    resultat,
                    Err(ErreurRequete::Canaux { brut: canaux }),
                    "canaux {canaux}"
                );
            }
        }
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, u16::MAX, 0)),
            Err(ErreurRequete::Canaux { brut: u16::MAX })
        );
    }

    /// Les champs qu'un ordre n'utilise pas sont exigés **nuls**.
    ///
    /// C'est la règle qui distingue « j'ai compris la requête » de « j'ai compris un
    /// préfixe de la requête ».
    #[test]
    fn les_champs_inutilises_sont_exiges_nuls() {
        // `version` et `lister` n'utilisent ni le câble ni les canaux.
        for ordre in [ORDRE_VERSION, ORDRE_LISTER] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 0, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "câble",
                    brut: 1
                }),
                "ordre {ordre}"
            );
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 0, 2, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "canaux",
                    brut: 2
                }),
                "ordre {ordre}"
            );
        }
        // `activer` et `désactiver` n'utilisent pas les canaux.
        for ordre in [ORDRE_ACTIVER, ORDRE_DESACTIVER] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 2, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "canaux",
                    brut: 2
                }),
                "ordre {ordre}"
            );
        }
        // Le champ réservé est refusé pour tous les ordres, y compris valides.
        for ordre in [
            ORDRE_VERSION,
            ORDRE_LISTER,
            ORDRE_ACTIVER,
            ORDRE_DESACTIVER,
            ORDRE_CANAUX,
        ] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 2, 0xBEEF)),
                Err(ErreurRequete::Reserve { brut: 0xBEEF }),
                "ordre {ordre}"
            );
        }
    }

    /// L'ordre des contrôles suit celui des champs : la cause nommée est la **première**
    /// anomalie, pas une arbitraire.
    #[test]
    fn la_cause_nommee_est_la_premiere_anomalie() {
        // Tout est faux à la fois : c'est la version qui est signalée.
        assert_eq!(
            Requete::from_bytes(&brut(0xFF, 0xFF, 0xFFFF, 0xFFFF, 0xFFFF)),
            Err(ErreurRequete::Version { trouvee: 0xFF })
        );
        // Version réparée : le champ réservé passe avant l'ordre, parce qu'il condamne
        // la trame entière quel que soit l'ordre.
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, 0xFF, 0xFFFF, 0xFFFF, 0xFFFF)),
            Err(ErreurRequete::Reserve { brut: 0xFFFF })
        );
        // Puis l'ordre, puis le câble, puis les canaux.
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, 0xFF, 0xFFFF, 0xFFFF, 0)),
            Err(ErreurRequete::Ordre { code: 0xFF })
        );
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 0xFFFF, 0xFFFF, 0)),
            Err(ErreurRequete::Cable { brut: 0xFFFF })
        );
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, 0xFFFF, 0)),
            Err(ErreurRequete::Canaux { brut: 0xFFFF })
        );
    }

    /// Chaque cause de refus a le statut qui dit au client quoi faire.
    #[test]
    fn chaque_cause_a_son_statut() {
        let cas = [
            (ErreurRequete::Longueur { recus: 7 }, Statut::TrameInvalide),
            (
                ErreurRequete::Version { trouvee: 2 },
                Statut::VersionInconnue,
            ),
            (ErreurRequete::Ordre { code: 9 }, Statut::OrdreInconnu),
            (ErreurRequete::Cable { brut: 0 }, Statut::CableInconnu),
            (ErreurRequete::Canaux { brut: 99 }, Statut::CanauxInvalides),
            (
                ErreurRequete::ChampParasite {
                    champ: "câble",
                    brut: 1,
                },
                Statut::TrameInvalide,
            ),
            (ErreurRequete::Reserve { brut: 1 }, Statut::TrameInvalide),
        ];
        for (erreur, statut) in cas {
            assert_eq!(erreur.statut(), statut, "{erreur:?}");
            // Le message nomme la valeur trouvée, pas seulement le fait qu'on a refusé.
            assert!(!erreur.to_string().is_empty(), "{erreur:?}");
        }
    }

    /// Les messages nomment le champ, la valeur et le domaine — pas « requête
    /// invalide ».
    #[test]
    fn les_messages_nomment_le_champ_et_sa_valeur() {
        let ligne = ErreurRequete::Longueur { recus: 9 }.to_string();
        assert!(ligne.contains('9'), "{ligne}");
        assert!(ligne.contains('8'), "{ligne}");

        let ligne = ErreurRequete::Cable { brut: 99 }.to_string();
        assert!(ligne.contains("99"), "{ligne}");
        assert!(ligne.contains("câble"), "{ligne}");
        assert!(ligne.contains("16"), "{ligne}");

        let ligne = ErreurRequete::Canaux { brut: 12 }.to_string();
        assert!(ligne.contains("12"), "{ligne}");
        assert!(ligne.contains("canaux"), "{ligne}");

        let ligne = ErreurRequete::ChampParasite {
            champ: "canaux",
            brut: 4,
        }
        .to_string();
        assert!(ligne.contains("canaux"), "{ligne}");
        assert!(ligne.contains('4'), "{ligne}");

        // Le statut des canaux non applicables **nomme M1b-05** : c'est ce qui évite
        // qu'on cherche un bogue là où il n'y a qu'une tâche pas encore faite.
        let ligne = Statut::CanauxNonApplicables.to_string();
        assert!(ligne.contains("M1b-05"), "{ligne}");
    }

    /// Les codes de statut sont stables, deux à deux distincts, et l'aller-retour est
    /// fidèle.
    #[test]
    fn statut_table() {
        let attendus: [(Statut, u8); 10] = [
            (Statut::Succes, 0),
            (Statut::VersionInconnue, 1),
            (Statut::TrameInvalide, 2),
            (Statut::OrdreInconnu, 3),
            (Statut::CableInconnu, 4),
            (Statut::CanauxInvalides, 5),
            (Statut::CanauxNonApplicables, 6),
            (Statut::PiloteAbsent, 7),
            (Statut::PrivilegeAbsent, 8),
            (Statut::ErreurSysteme, 9),
        ];
        for (statut, code) in attendus {
            assert_eq!(statut.code(), code, "{statut:?}");
            assert_eq!(Statut::from_code(code), Some(statut));
        }
        assert_eq!(Statut::ALL.len(), attendus.len());
        // Un code inconnu n'est pas replié sur une erreur générique.
        for code in 10..=u8::MAX {
            assert_eq!(Statut::from_code(code), None, "code {code}");
        }
        assert!(Statut::Succes.succes());
        assert!(!Statut::ErreurSysteme.succes());
    }

    /// La réponse fait l'aller-retour, et les masques se relisent par câble affiché.
    #[test]
    fn la_reponse_fait_l_aller_retour() {
        let reponse = Reponse {
            ordre: ORDRE_ACTIVER,
            statut: Statut::Succes,
            detail: 0,
            presents: 0xFFFF,
            actifs: 0b0000_0101,
            version_ks: 1,
            canaux: 2,
        };
        let octets = reponse.to_bytes();
        assert_eq!(octets.len(), TAILLE_REPONSE);
        assert_eq!(octets[0], PROTOCOLE_VERSION);
        assert_eq!(Reponse::from_bytes(&octets), Ok(reponse));

        // Le décalage de un est fait par la réponse : bit 0 = « Conduit 1 ».
        assert!(reponse.est_actif(CableId(1)));
        assert!(!reponse.est_actif(CableId(2)));
        assert!(reponse.est_actif(CableId(3)));
        assert!(!reponse.est_actif(CableId(0)));
        assert!(!reponse.est_actif(CableId(17)));
        assert!(reponse.est_present(CableId(16)));
        assert!(!reponse.est_present(CableId(17)));
    }

    /// Table des refus de réponse : longueur, version, statut inconnu, réservés.
    #[test]
    fn reponse_refus_table() {
        let valide = Reponse::refus(ORDRE_LISTER, Statut::PiloteAbsent).to_bytes();
        assert!(Reponse::from_bytes(&valide).is_ok());

        assert_eq!(
            Reponse::from_bytes(&valide[..27]),
            Err(ErreurReponse::Longueur { recus: 27 })
        );
        let mut trop_long = valide.to_vec();
        trop_long.push(0);
        assert_eq!(
            Reponse::from_bytes(&trop_long),
            Err(ErreurReponse::Longueur { recus: 29 })
        );

        let mut mauvaise_version = valide;
        mauvaise_version[0] = 2;
        assert_eq!(
            Reponse::from_bytes(&mauvaise_version),
            Err(ErreurReponse::Version { trouvee: 2 })
        );

        let mut reserve = valide;
        reserve[3] = 1;
        assert_eq!(
            Reponse::from_bytes(&reserve),
            Err(ErreurReponse::Reserve { brut: 1 })
        );

        let mut statut = valide;
        statut[2] = 200;
        assert_eq!(
            Reponse::from_bytes(&statut),
            Err(ErreurReponse::Statut { code: 200 })
        );

        let mut reserve2 = valide;
        reserve2[24] = 0xFF;
        assert_eq!(
            Reponse::from_bytes(&reserve2),
            Err(ErreurReponse::Reserve2 { brut: 0xFF })
        );
    }

    /// Le cadrage : longueur préfixée, bornée **avant** l'allocation.
    #[test]
    fn cadrage_table() {
        let charge = Requete::Lister.to_bytes();
        let trame = encadrer(&charge);
        assert_eq!(trame.len(), EN_TETE_OCTETS + TAILLE_REQUETE);
        assert_eq!(&trame[..4], &(TAILLE_REQUETE as u32).to_le_bytes());
        assert_eq!(Requete::Lister.encadrer(), trame);

        // Découpage : la charge et le nombre d'octets consommés.
        let (lue, consommes) = decouper(&trame).unwrap().expect("trame complète");
        assert_eq!(lue, &charge[..]);
        assert_eq!(consommes, trame.len());

        // Incomplet : on attend, on ne se trompe pas.
        for coupe in 0..trame.len() {
            assert_eq!(decouper(&trame[..coupe]), Ok(None), "coupé à {coupe}");
        }

        // Deux trames à la suite : la première est rendue, le reste attend son tour.
        let mut deux = trame.clone();
        deux.extend_from_slice(&trame);
        let (_, consommes) = decouper(&deux).unwrap().expect("première trame");
        assert_eq!(consommes, trame.len());
        assert!(decouper(&deux[consommes..]).unwrap().is_some());

        // Longueur hors borne : refusée sans qu'aucun tampon ne soit réservé.
        for annoncee in [MAX_TRAME_OCTETS as u32 + 1, 1 << 20, u32::MAX] {
            let mut hostile = annoncee.to_le_bytes().to_vec();
            hostile.extend_from_slice(&charge);
            assert_eq!(
                decouper(&hostile),
                Err(ErreurTrame::TropGrande {
                    annoncee: annoncee as usize,
                    maximum: MAX_TRAME_OCTETS
                }),
                "longueur {annoncee}"
            );
        }
        // Exactement la borne : acceptée (c'est une borne, pas un seuil).
        let en_tete = (MAX_TRAME_OCTETS as u32).to_le_bytes();
        assert_eq!(longueur_annoncee(en_tete), Ok(MAX_TRAME_OCTETS));
        // Nulle : refusée.
        assert_eq!(longueur_annoncee([0; 4]), Err(ErreurTrame::Vide));
        assert_eq!(decouper(&[0, 0, 0, 0]), Err(ErreurTrame::Vide));
        // En-tête incomplet : on attend.
        for coupe in 0..EN_TETE_OCTETS {
            assert_eq!(decouper(&trame[..coupe]), Ok(None));
        }
    }

    /// Le nom du canal est bien un canal local, et il est gravé.
    #[test]
    fn le_nom_du_canal_est_local() {
        assert_eq!(NOM_TUBE, r"\\.\pipe\conduit-helper");
        assert!(NOM_TUBE.starts_with(r"\\.\pipe\"));
        // Pas de canal distant : `\\serveur\pipe\…` serait une tout autre surface.
        assert!(!NOM_TUBE.starts_with(r"\\serveur"));
    }

    /// Les canaux applicables aujourd'hui sont ceux du contrat, pas un 2 recopié.
    #[test]
    fn les_canaux_applicables_viennent_du_contrat() {
        assert_eq!(CANAUX_APPLICABLES, DEFAULT_CHANNELS);
        const _: () =
            assert!(MIN_CHANNELS <= CANAUX_APPLICABLES && CANAUX_APPLICABLES <= MAX_CHANNELS);
        // Le prédicat du contrat et le nôtre disent la même chose : quand M1b-05 rendra
        // les canaux dynamiques, `channels_applicables` disparaîtra et ce test tombera,
        // ce qui est exactement le rappel qu'on veut.
        for canaux in MIN_CHANNELS..=MAX_CHANNELS {
            let etat = conduit_kmd_core::config::CableState {
                cable: 0,
                connected: 1,
                channels: canaux,
                reserved: 0,
            };
            assert_eq!(
                etat.channels_applicables(),
                canaux == CANAUX_APPLICABLES,
                "canaux {canaux}"
            );
        }
    }

    proptest! {
        /// Le critère de M1b-08 : **quelle que soit l'entrée**, le parseur ne panique
        /// pas, et toute requête acceptée se resérialise à l'identique.
        #[test]
        fn aucune_requete_ne_panique_et_l_aller_retour_est_fidele(
            octets in proptest::collection::vec(any::<u8>(), 0..40),
        ) {
            match Requete::from_bytes(&octets) {
                Ok(requete) => {
                    prop_assert_eq!(octets.len(), TAILLE_REQUETE);
                    prop_assert_eq!(&requete.to_bytes()[..], &octets[..]);
                    prop_assert_eq!(Requete::from_bytes(&requete.to_bytes()), Ok(requete));
                    // Toute sortie acceptée est dans les domaines annoncés.
                    if let Some(cable) = requete.cable() {
                        prop_assert!(cable.0 >= 1 && cable.0 <= CABLE_MAX);
                    }
                    if let Requete::Canaux { canaux, .. } = requete {
                        prop_assert!((MIN_CHANNELS..=MAX_CHANNELS).contains(&canaux));
                    }
                }
                Err(err) => {
                    // Une erreur de longueur exactement quand la longueur est fausse.
                    let longueur = matches!(err, ErreurRequete::Longueur { .. });
                    prop_assert_eq!(longueur, octets.len() != TAILLE_REQUETE);
                }
            }
        }

        /// Idem côté réponse.
        #[test]
        fn aucune_reponse_ne_panique(octets in proptest::collection::vec(any::<u8>(), 0..64)) {
            match Reponse::from_bytes(&octets) {
                Ok(reponse) => {
                    prop_assert_eq!(octets.len(), TAILLE_REPONSE);
                    prop_assert_eq!(&reponse.to_bytes()[..], &octets[..]);
                }
                Err(err) => {
                    let longueur = matches!(err, ErreurReponse::Longueur { .. });
                    prop_assert_eq!(longueur, octets.len() != TAILLE_REPONSE);
                }
            }
        }

        /// Le découpage ne panique jamais et ne rend jamais plus d'octets que le tampon
        /// n'en contient — c'est la propriété qui garde le serveur d'un débordement.
        #[test]
        fn le_decoupage_ne_deborde_jamais(
            octets in proptest::collection::vec(any::<u8>(), 0..200),
        ) {
            match decouper(&octets) {
                Ok(Some((charge, consommes))) => {
                    prop_assert!(consommes <= octets.len());
                    prop_assert_eq!(charge.len(), consommes - EN_TETE_OCTETS);
                    prop_assert!(charge.len() <= MAX_TRAME_OCTETS);
                    prop_assert!(!charge.is_empty());
                }
                Ok(None) => {}
                Err(ErreurTrame::TropGrande { annoncee, .. }) => {
                    prop_assert!(annoncee > MAX_TRAME_OCTETS);
                }
                Err(ErreurTrame::Vide) => {
                    prop_assert!(octets.len() >= EN_TETE_OCTETS);
                    prop_assert_eq!(&octets[..EN_TETE_OCTETS], &[0, 0, 0, 0]);
                }
            }
        }

        /// Une requête quelconque des cinq ordres survit au cadrage et au découpage.
        #[test]
        fn le_cadrage_est_reversible(
            ordre in 0_u8..5,
            cable in 1_u32..=CABLE_MAX,
            canaux in MIN_CHANNELS..=MAX_CHANNELS,
        ) {
            let requete = match ordre {
                0 => Requete::Version,
                1 => Requete::Lister,
                2 => Requete::Activer(CableId(cable)),
                3 => Requete::Desactiver(CableId(cable)),
                _ => Requete::Canaux { cable: CableId(cable), canaux },
            };
            let trame = requete.encadrer();
            let (charge, consommes) = decouper(&trame)
                .map_err(|e| TestCaseError::fail(e.to_string()))?
                .ok_or_else(|| TestCaseError::fail("trame incomplète"))?;
            prop_assert_eq!(consommes, trame.len());
            prop_assert_eq!(Requete::from_bytes(charge), Ok(requete));
        }
    }
}
