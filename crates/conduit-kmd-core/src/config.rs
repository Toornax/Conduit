//! Contrat du jeu de propriétés KS **privé** de configuration (M1b-04,
//! `docs/driver-design.md` §6) : le GUID du jeu, les identifiants de propriété, la
//! structure d'échange, son parseur, et le masque de bits qui persiste l'état des câbles.
//!
//! Tout est ici et **rien n'appelle le noyau** : ce module est le contrat que le service
//! d'assistance (M1b-20) et le pilote se partagent, celui que M1b-08 fuzzera en mode
//! utilisateur, et le seul endroit où une valeur venue de l'extérieur est déclarée valide.
//! `portcls::config` ne fait que sérialiser et appeler ; `conduit_kmd::registry` ne fait
//! que lire et écrire des octets.
//!
//! # La frontière de confiance passe exactement ici
//!
//! Une requête `IOCTL_KS_PROPERTY` vient d'un processus utilisateur quelconque — le
//! descripteur de sécurité que l'INF pose sur l'objet de périphérique
//! (`…(A;;GRGWGX;;;WD)`, « Tout le monde ») le laisse ouvrir nos filtres KS, comme tout
//! adaptateur audio. Le contenu et la **longueur** du tampon sont donc hostiles. D'où la
//! règle qui gouverne [`CableState::from_bytes`] :
//!
//! **toute longueur inattendue est refusée, y compris un préfixe valide suivi d'octets en
//! trop.** Accepter un préfixe est le défaut classique de ce genre de parseur : il rend le
//! format non extensible (une v2 plus longue serait silencieusement tronquée à la v1, et
//! le client croirait avoir réglé un champ que le pilote n'a jamais lu) et c'est
//! exactement le premier écart qu'un fuzzer trouve. La version se négocie par
//! [`KSPROPERTY_CONDUIT_VERSION`], pas par la longueur du tampon.
//!
//! # Boutisme : natif ici, petit-boutiste chez le voisin
//!
//! [`CableState`] traverse une frontière **mémoire** (KS copie le tampon d'un processus à
//! un autre sur la même machine) : ses champs sont donc en boutisme **natif**, comme les
//! `KSJACK_DESCRIPTION` de `portcls::jack` et les `KSPROPERTY_DESCRIPTION` de
//! `portcls::property`. Le masque [`ACTIVE_CABLES_VALUE_NAME`], lui, traverse une
//! frontière de **format** (`REG_DWORD`, défini petit-boutiste par `winnt.h`) et se décode
//! par [`crate::params::decode_dword`]. Les deux conventions coexistent volontairement ;
//! Windows ne tournant que sur des architectures petit-boutistes, la différence est
//! documentaire — mais elle dit d'où vient chaque tampon.
//!
//! # Ce que ce module valide et n'applique pas
//!
//! Le champ [`CableState::channels`] est **validé** contre les bornes de
//! [`crate::params`] et **rien de plus** : son application appartient à M1b-05.
//! `conduit_kmd::descriptors::CHANNELS` est aujourd'hui scellée dans les tables KS
//! (`KSDATARANGE_AUDIO`, `MaximumChannels`) et dans des assertions à la compilation
//! (`topo.rs` : `CHANNELS as usize <= MAX_CHANNELS`) ; l'accepter dans la structure
//! d'échange sans le refuser à la validation ferait croire au service d'assistance qu'il
//! peut le régler. Le gestionnaire de propriété exige donc [`DEFAULT_CHANNELS`] tant que
//! M1b-05 n'a pas rendu la valeur dynamique — voir [`CableState::channels_applicables`].

use core::fmt;

use crate::params::{DEFAULT_CHANNELS, MAX_CHANNELS, MAX_RESERVE, MIN_CHANNELS};

// ---------------------------------------------------------------------------------
// Le jeu de propriétés.
// ---------------------------------------------------------------------------------

/// Un `GUID` de `guiddef.h`, dans la disposition exacte de la structure C.
///
/// Recopié plutôt qu'importé : ce crate est portable, sans dépendance, et se teste depuis
/// Linux et macOS, là où `portcls_sys::GUID` n'existe pas. `portcls::config` le
/// reconvertit en `portcls_sys::GUID` par une `const fn`, sous assertion `const` que les
/// quatre champs se correspondent — un GUID de jeu faux ne se manifesterait que par une
/// propriété que personne ne trouve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ConfigGuid {
    /// `Data1: ULONG`.
    pub data1: u32,
    /// `Data2: USHORT`.
    pub data2: u16,
    /// `Data3: USHORT`.
    pub data3: u16,
    /// `Data4: [UCHAR; 8]`.
    pub data4: [u8; 8],
}

/// La disposition est bien celle de `guiddef.h` : 16 octets, aligné sur 4.
const _: () = assert!(size_of::<ConfigGuid>() == 16);
const _: () = assert!(align_of::<ConfigGuid>() == 4);
const _: () = assert!(core::mem::offset_of!(ConfigGuid, data1) == 0);
const _: () = assert!(core::mem::offset_of!(ConfigGuid, data2) == 4);
const _: () = assert!(core::mem::offset_of!(ConfigGuid, data3) == 6);
const _: () = assert!(core::mem::offset_of!(ConfigGuid, data4) == 8);

/// `KSPROPSETID_Conduit` : le jeu de propriétés **privé** de configuration
/// (`{3F1B27A4-8C6E-4D02-9B75-E4A0D61C8F3B}`).
///
/// **Arbitraire et définitif.** Engendré une fois par `[guid]::NewGuid()`, comme
/// `portcls::adapter::PIN_NAME_BASE` ; aucun jeu KS publié ne décrit la configuration d'un
/// câble virtuel, il fallait donc en inventer un. Il est **gravé** : le service
/// d'assistance (M1b-20), les futures versions du pilote et toute installation déjà faite
/// s'y donnent rendez-vous. Le changer rendrait invisible la propriété d'un pilote au
/// service qui l'accompagne, sans le moindre message d'erreur — le client chercherait un
/// jeu que personne n'expose et conclurait « propriété non supportée ». Le versionnement
/// se fait par [`CONFIG_VERSION`], jamais par un nouveau GUID.
pub const KSPROPSETID_CONDUIT: ConfigGuid = ConfigGuid {
    data1: 0x3F1B_27A4,
    data2: 0x8C6E,
    data3: 0x4D02,
    data4: [0x9B, 0x75, 0xE4, 0xA0, 0xD6, 0x1C, 0x8F, 0x3B],
};

/// `KSPROPERTY_CONDUIT_CABLE_STATE` : l'état du câble que la requête vise (`GET`/`SET`).
///
/// La valeur échangée est une [`CableState`]. Le câble n'est **pas** désigné par un
/// paramètre de la requête mais par le filtre auquel elle s'adresse : le service ouvre
/// l'interface `KSCATEGORY_TOPOLOGY` du câble voulu, et le gestionnaire retrouve son câble
/// par le `MajorTarget` de la requête, comme le volume et le jack. Le champ
/// [`CableState::cable`] n'est qu'un **écho vérifié**, pas un sélecteur : voir sa
/// documentation.
pub const KSPROPERTY_CONDUIT_CABLE_STATE: u32 = 0;

/// `KSPROPERTY_CONDUIT_VERSION` : la version de ce contrat (`GET` seulement).
///
/// Un `ULONG`, et un seul, pour que le service d'assistance (M1b-20) détecte une
/// inadéquation **avant** d'échanger une [`CableState`] dont il croirait connaître la
/// forme. C'est la seule voie de versionnement : ni la longueur du tampon (refusée dès
/// qu'elle change), ni un second GUID de jeu.
pub const KSPROPERTY_CONDUIT_VERSION: u32 = 1;

/// Le `pid` de la **marque de câble** dans le magasin de propriétés d'un endpoint
/// MMDevices : la valeur `{3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b},1`.
///
/// # Ce que la marque est, et pourquoi elle existe
///
/// Renommer un câble écrase `PKEY_Device_DeviceDesc`, la description par laquelle on
/// retrouvait « Conduit *N* ». La marque est écrite **avant** ce renommage, dans la même
/// clé, et porte le nom d'origine du câble (`Conduit 3`) : c'est elle qui rattache un
/// endpoint renommé à son numéro. Un identifiant déduit d'un texte d'affichage ne
/// survit pas au changement de ce texte ; celui-ci, si.
///
/// # Espace de nommage
///
/// Ce n'est **pas** un `KSPROPERTY` : les `pid` d'un `PROPERTYKEY` de magasin de
/// propriétés Windows et les `Id` d'un `KSPROPERTY` sont deux numérotations
/// indépendantes, et la valeur 1 partagée avec [`KSPROPERTY_CONDUIT_VERSION`] n'est pas
/// une collision — le pilote ne voit jamais cette clé, et personne ne la lui envoie. Le
/// `pid` 0 est en revanche réservé par le système de propriétés Windows, d'où 1.
///
/// # Pourquoi ici
///
/// La marque a **deux** lecteurs en espace utilisateur : `conduit-helper` l'écrit dans le
/// registre (`registre::valeur_marque`) et le dorsal WASAPI la lit par `IPropertyStore`
/// (`devices::MARQUE_KEY`). Elle vit donc avec le GUID dont elle dépend, plutôt que
/// recopiée dans chacun des deux — le crate est portable et sans dépendance, les deux y
/// accèdent.
pub const PID_MARQUE_CABLE: u32 = 1;

/// Version du contrat rendue par [`KSPROPERTY_CONDUIT_VERSION`].
///
/// À incrémenter **à chaque** changement observable de [`CableState`] — un champ ajouté,
/// un domaine élargi, une sémantique modifiée. Le service d'assistance compare, refuse de
/// piloter un pilote qu'il ne connaît pas, et le dit ; sans ce numéro, la panne serait un
/// `STATUS_INVALID_PARAMETER` inexplicable sur une longueur d'un octet de trop.
pub const CONFIG_VERSION: u32 = 1;

/// Nombre de câbles que le contrat sait adresser : le plafond de la réserve
/// ([`crate::params::MAX_RESERVE`], SPEC F-06).
///
/// C'est aussi le nombre de bits utiles du masque [`ACTIVE_CABLES_VALUE_NAME`]. Les deux
/// viennent de la même constante à dessein : un masque de 16 bits pour 32 câbles serait
/// une perte silencieuse de l'état des câbles 16 à 31.
pub const CABLE_MAX: u32 = MAX_RESERVE;

// ---------------------------------------------------------------------------------
// La structure d'échange.
// ---------------------------------------------------------------------------------

/// Taille d'un `ULONG`, l'unité de tout ce qui est sérialisé ici.
const TAILLE_MOT: usize = 4;

/// `CableState::cable` (`ULONG`) : décalage 0.
pub const O_CABLE: usize = 0;
/// `CableState::connected` (`ULONG`) : décalage 4.
pub const O_CONNECTED: usize = 4;
/// `CableState::channels` (`ULONG`) : décalage 8.
pub const O_CHANNELS: usize = 8;
/// `CableState::reserved` (`ULONG`) : décalage 12.
pub const O_RESERVED: usize = 12;

/// Taille de la valeur de [`KSPROPERTY_CONDUIT_CABLE_STATE`], en octets : **16**.
///
/// Fixe et vérifiée par assertion `const` contre `size_of::<CableState>()`. Une structure
/// qui grandirait sans que cette constante bouge donnerait un `GET` qui écrit hors de ce
/// que le client a réservé.
pub const CABLE_STATE_BYTES: usize = 16;

/// L'état d'un câble, tel qu'il traverse `IOCTL_KS_PROPERTY`.
///
/// `#[repr(C)]`, taille fixe, **rembourrage explicite** : les quatre champs sont des
/// `ULONG` contigus, il n'y a donc aucun trou que le compilateur choisirait pour nous, et
/// [`CableState::reserved`] est un champ nommé qu'on **exige nul** plutôt qu'un silence de
/// disposition. Un rembourrage implicite serait de la mémoire noyau non initialisée
/// recopiée vers l'espace utilisateur au `GET`, et un champ que personne ne valide au
/// `SET`.
///
/// Les décalages sont ceux de [`O_CABLE`] à [`O_RESERVED`], et les assertions `const`
/// ci-dessous les confrontent à `offset_of!` : **c'est notre structure, pas une structure
/// du WDK**, donc l'oracle n'est pas `layout.golden` mais le `repr(C)` de ce fichier —
/// et il est vérifié à la compilation plutôt que recopié de mémoire. `portcls::config`
/// sérialise à ces mêmes constantes : une seule source, pas deux copies qui divergeraient
/// d'un octet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct CableState {
    /// Numéro du câble, de 0 à [`CABLE_MAX`] − 1 (« Conduit *n+1* » pour l'utilisateur).
    ///
    /// **Un écho, pas un sélecteur.** Le câble visé est celui du filtre auquel la requête
    /// s'adresse ; ce champ le répète pour deux raisons. Au `GET`, il dit au service
    /// d'assistance quel câble il vient de lire, sans qu'il ait à se souvenir du
    /// descripteur qu'il a ouvert. Au `SET`, le gestionnaire le **compare** à son propre
    /// câble et refuse `STATUS_INVALID_PARAMETER` s'il diffère : un service qui se
    /// tromperait de descripteur déconnecterait sinon le mauvais câble en silence.
    ///
    /// La comparaison ne peut pas se faire ici — ce module ne connaît aucun câble — mais
    /// le **domaine**, si : `cable < CABLE_MAX`.
    pub cable: u32,
    /// L'état actif du câble : **0 ou 1, strictement**.
    ///
    /// Volontairement plus strict que le `BOOL` de KS, dont `portcls::audio` accepte
    /// n'importe quelle valeur non nulle pour la sourdine. La différence est celle des
    /// deux protocoles : un `KSPROPERTY_AUDIO_MUTE` vient du moteur audio de Windows, on
    /// n'a rien à lui apprendre et le refuser ferait apparaître l'endpoint cassé ; une
    /// [`CableState`] vient de **notre** service, sur **notre** jeu de propriétés
    /// versionné. Un `connected = 0xFF` y est un défaut du service, pas une convention
    /// tolérée, et le refuser le fait apparaître à l'écriture du service plutôt qu'à la
    /// lecture du journal six mois plus tard.
    pub connected: u32,
    /// Nombre de canaux du câble, dans `MIN_CHANNELS..=MAX_CHANNELS`.
    ///
    /// **Validé, pas appliqué** (M1b-05) : voir l'en-tête de module et
    /// [`CableState::channels_applicables`]. Le domaine est celui de
    /// [`crate::params::Param::Channels`] — une seule définition des bornes, comme pour
    /// le registre.
    pub channels: u32,
    /// Rembourrage **explicite**, exigé nul dans les deux sens.
    ///
    /// Le refuser non nul est ce qui garde la place libre : un client qui y écrirait
    /// aujourd'hui n'importe quoi rendrait impossible d'y loger un vrai champ demain sans
    /// casser ce client. Le `GET` l'écrit toujours à zéro : rien de la mémoire du noyau ne
    /// transite par ce champ.
    pub reserved: u32,
}

// La taille annoncée est celle de la structure, et chaque décalage nommé est celui que
// `repr(C)` produit : c'est ce couple d'assertions qui remplace un golden.
const _: () = assert!(size_of::<CableState>() == CABLE_STATE_BYTES);
const _: () = assert!(align_of::<CableState>() == 4);
const _: () = assert!(core::mem::offset_of!(CableState, cable) == O_CABLE);
const _: () = assert!(core::mem::offset_of!(CableState, connected) == O_CONNECTED);
const _: () = assert!(core::mem::offset_of!(CableState, channels) == O_CHANNELS);
const _: () = assert!(core::mem::offset_of!(CableState, reserved) == O_RESERVED);
// Les quatre champs pavent la structure : aucun trou, aucun recouvrement, donc aucun
// octet de rembourrage implicite à recopier vers l'espace utilisateur.
const _: () = assert!(O_RESERVED.saturating_add(TAILLE_MOT) == CABLE_STATE_BYTES);

/// Ce qui a fait refuser une [`CableState`] : un cas, une cause, une ligne de journal.
///
/// Chaque variante porte ce qui a été trouvé, pas seulement le fait qu'on a refusé :
/// « paramètre invalide » ne sert à rien à 3 h du matin (même principe que
/// [`crate::params::Correction`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigError {
    /// Longueur du tampon différente de [`CABLE_STATE_BYTES`] — plus courte **ou** plus
    /// longue. Voir la règle en tête de module.
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// [`CableState::cable`] au-delà du dernier câble.
    Cable(u32),
    /// [`CableState::connected`] hors de `{0, 1}`.
    Connected(u32),
    /// [`CableState::channels`] hors de `MIN_CHANNELS..=MAX_CHANNELS`.
    Channels(u32),
    /// [`CableState::reserved`] non nul.
    Reserved(u32),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "valeur de {recus} octets, {CABLE_STATE_BYTES} attendus exactement"
            ),
            Self::Cable(cable) => write!(f, "câble {cable} inconnu (0 à {} )", {
                CABLE_MAX.saturating_sub(1)
            }),
            Self::Connected(brut) => write!(f, "état de connexion {brut} hors de 0 et 1"),
            Self::Channels(brut) => {
                write!(f, "{brut} canaux hors de {MIN_CHANNELS} à {MAX_CHANNELS}")
            }
            Self::Reserved(brut) => write!(f, "champ réservé non nul ({brut:#010x})"),
        }
    }
}

/// Lit un `ULONG` au décalage `offset` d'un tampon d'alignement quelconque.
///
/// Les quatre octets sont **recopiés** puis interprétés, jamais transtypés : c'est la
/// règle de `portcls::property`, et elle vaut ici aussi, ce module recevant le même
/// tampon.
fn mot(data: &[u8], offset: usize) -> Option<u32> {
    let fin = offset.checked_add(TAILLE_MOT)?;
    let quatre = data.get(offset..fin)?;
    let mut octets = [0u8; TAILLE_MOT];
    octets.copy_from_slice(quatre);
    Some(u32::from_ne_bytes(octets))
}

impl CableState {
    /// L'état d'un câble au repos : connecté, canaux par défaut, réservé nul.
    ///
    /// Sert de point de départ aux tests et au service d'assistance ; le pilote, lui,
    /// construit toujours la sienne depuis l'état vivant du câble.
    #[must_use]
    pub const fn new(cable: u32, connected: bool) -> Self {
        Self {
            cable,
            connected: if connected { 1 } else { 0 },
            channels: DEFAULT_CHANNELS,
            reserved: 0,
        }
    }

    /// L'état actif, en booléen. `connected` est déjà validé dans `{0, 1}` par
    /// [`Self::from_bytes`].
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected != 0
    }

    /// Le nombre de canaux est-il celui que le pilote sait **servir** aujourd'hui ?
    ///
    /// [`Self::from_bytes`] accepte tout le domaine `MIN_CHANNELS..=MAX_CHANNELS`, parce
    /// que c'est le domaine du contrat et que le fuzzer doit l'explorer en entier. Mais
    /// tant que M1b-05 n'a pas rendu les canaux dynamiques, `descriptors::CHANNELS` est
    /// scellée dans les tables KS et dans des assertions à la compilation : servir un
    /// `SET` à six canaux rendrait `STATUS_SUCCESS` pour un réglage qui n'agirait sur
    /// rien, ce qui est pire qu'un refus. Le gestionnaire refuse donc, et ce prédicat
    /// nomme la frontière au lieu de la laisser dans un `if` anonyme.
    ///
    /// **À supprimer avec M1b-05**, pas avant.
    #[must_use]
    pub const fn channels_applicables(&self) -> bool {
        self.channels == DEFAULT_CHANNELS
    }

    /// Sérialise l'état en [`CABLE_STATE_BYTES`] octets, champ par champ, aux décalages
    /// [`O_CABLE`] à [`O_RESERVED`].
    ///
    /// Le pilote n'utilise pas cette fonction pour répondre à un `GET` — il écrit
    /// directement dans le tampon `Value`, d'alignement quelconque, par
    /// `portcls::property::Champs` — mais elle en est le miroir exact, et c'est elle que
    /// l'aller-retour du proptest vérifie.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; CABLE_STATE_BYTES] {
        let cable = self.cable.to_ne_bytes();
        let connected = self.connected.to_ne_bytes();
        let channels = self.channels.to_ne_bytes();
        let reserved = self.reserved.to_ne_bytes();
        [
            cable[0],
            cable[1],
            cable[2],
            cable[3],
            connected[0],
            connected[1],
            connected[2],
            connected[3],
            channels[0],
            channels[1],
            channels[2],
            channels[3],
            reserved[0],
            reserved[1],
            reserved[2],
            reserved[3],
        ]
    }

    /// **Le** parseur : des octets hostiles vers un état valide, ou une cause de refus.
    ///
    /// Pure, sans allocation, sans panique, sans appel noyau — appelable à n'importe quel
    /// IRQL et fuzzable en mode utilisateur (M1b-08). L'ordre des contrôles est celui des
    /// champs, pour que le message nomme la **première** anomalie rencontrée plutôt qu'une
    /// arbitraire.
    ///
    /// La longueur est vérifiée **avant** tout le reste et exigée **exacte** : ni plus
    /// courte, ni plus longue, ni un préfixe valide suivi d'octets en trop (voir la règle
    /// en tête de module).
    pub fn from_bytes(data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() != CABLE_STATE_BYTES {
            return Err(ConfigError::Longueur { recus: data.len() });
        }
        // Les quatre lectures ne peuvent plus échouer (la longueur est exacte) ; le
        // `ok_or` remplace un `unwrap` interdit par les lints du crate.
        let longueur = || ConfigError::Longueur { recus: data.len() };
        let cable = mot(data, O_CABLE).ok_or_else(longueur)?;
        let connected = mot(data, O_CONNECTED).ok_or_else(longueur)?;
        let channels = mot(data, O_CHANNELS).ok_or_else(longueur)?;
        let reserved = mot(data, O_RESERVED).ok_or_else(longueur)?;

        if cable >= CABLE_MAX {
            return Err(ConfigError::Cable(cable));
        }
        if connected > 1 {
            return Err(ConfigError::Connected(connected));
        }
        if !(MIN_CHANNELS..=MAX_CHANNELS).contains(&channels) {
            return Err(ConfigError::Channels(channels));
        }
        if reserved != 0 {
            return Err(ConfigError::Reserved(reserved));
        }
        Ok(Self {
            cable,
            connected,
            channels,
            reserved,
        })
    }
}

// ---------------------------------------------------------------------------------
// Persistance : un masque de bits, une seule valeur de registre.
// ---------------------------------------------------------------------------------

/// Nom de la valeur `REG_DWORD` qui persiste l'état actif des câbles, dans la clé
/// **matérielle** du périphérique (`HKR`, sous `Device Parameters`).
///
/// # Pourquoi un masque et pas seize valeurs
///
/// Une seule écriture, donc **indivisible du point de vue d'un lecteur** : un
/// `ZwSetValueKey` de quatre octets remplace l'état des seize câbles d'un coup, là où
/// seize valeurs laisseraient une fenêtre où le registre décrit une configuration qui n'a
/// jamais existé — précisément la fenêtre qu'une coupure de courant élargit. Et une seule
/// valeur à retirer : `regedit` montre une ligne, la désinstallation en supprime une.
///
/// # Pourquoi `HKR` et pas `HKLM`
///
/// PnP supprime la clé matérielle avec le périphérique : la désinstallation propre (F-52)
/// en dépend. Une valeur sous `Services` ou ailleurs dans `HKLM` survivrait à la
/// désinstallation et laisserait le registre sale — et `infverif /w` refuserait l'`AddReg`
/// correspondant. Le test `portcls/tests/inf.rs::defauts_de_parametres_identiques_au_pilote`
/// vérifie que toute la section reste en `HKR`.
pub const ACTIVE_CABLES_VALUE_NAME: &str = "ActiveCables";

/// Nom lisible du masque, en français, pour le journal.
pub const ACTIVE_CABLES_LABEL: &str = "câbles actifs";

/// Masque par défaut : **0x3**, les câbles 1 et 2 (« Conduit 1 » et « Conduit 2 »).
///
/// Écrit par l'INF dans `[ConduitCable_HW_AddReg]` (engendré depuis cette constante par
/// `portcls/tests/inf.rs`) et retenu par le pilote quand la valeur est absente, d'un autre
/// type ou illisible.
///
/// Les seize câbles s'énumèrent toujours tous — un endpoint apparaît pour chacun — mais
/// quatorze se présentent à Windows comme des prises vides et se rangent sous
/// « Périphériques déconnectés », hors de la liste des périphériques utilisables. C'est ce
/// qui rend un pilote à seize câbles supportable dans le panneau de son sans rien changer
/// à l'énumération PnP.
///
/// **Remplace `conduit_kmd::cable::CONNECTED_BY_DEFAULT`** : l'état initial ne vient plus
/// d'une constante lue à la construction des câbles mais du registre, ce défaut n'étant
/// que le repli.
pub const ACTIVE_CABLES_DEFAULT: u32 = 0b11;

/// Bits utiles du masque : un par câble adressable, les autres à zéro.
///
/// `checked_shl` plutôt qu'un décalage nu : à [`CABLE_MAX`] = 32, `1 << 32` déborderait, et
/// le lint `arithmetic_side_effects` du crate refuse l'opérateur de toute façon. Le
/// repli `u32::MAX` est le masque correct dans ce cas.
pub const ACTIVE_CABLES_MASK: u32 = match 1_u32.checked_shl(CABLE_MAX) {
    Some(apres_dernier) => apres_dernier.wrapping_sub(1),
    None => u32::MAX,
};

// Le défaut n'active que des câbles qui existent, et le masque couvre bien les seize.
const _: () = assert!(ACTIVE_CABLES_DEFAULT & ACTIVE_CABLES_MASK == ACTIVE_CABLES_DEFAULT);
const _: () = assert!(ACTIVE_CABLES_MASK.count_ones() == CABLE_MAX);
const _: () = assert!(ACTIVE_CABLES_DEFAULT == 0x3 && CABLE_MAX == 16);
const _: () = assert!(ACTIVE_CABLES_MASK == 0x0000_FFFF);

/// Le bit du câble `cable` dans le masque, ou `None` au-delà du dernier câble.
#[must_use]
pub const fn cable_bit(cable: u32) -> Option<u32> {
    if cable >= CABLE_MAX {
        return None;
    }
    1_u32.checked_shl(cable)
}

/// Le câble `cable` est-il actif dans `mask` ? **Faux** au-delà du dernier câble : un
/// numéro qui n'existe pas n'est jamais connecté.
#[must_use]
pub const fn is_active(mask: u32, cable: u32) -> bool {
    match cable_bit(cable) {
        Some(bit) => mask & bit != 0,
        None => false,
    }
}

/// `mask` où le câble `cable` vaut `active`. Rend `mask` **inchangé** au-delà du dernier
/// câble : le gestionnaire de propriété a déjà refusé ce cas, et fabriquer ici un masque
/// à bit perdu serait une seconde façon de se tromper.
#[must_use]
pub const fn with_active(mask: u32, cable: u32, active: bool) -> u32 {
    match cable_bit(cable) {
        Some(bit) => {
            if active {
                mask | bit
            } else {
                mask & !bit
            }
        }
        None => mask,
    }
}

/// Ce que [`sanitize_mask`] a corrigé : des bits au-delà du dernier câble, ignorés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaskFix {
    /// Masque trouvé dans le registre, tel quel.
    pub found: u32,
    /// Masque retenu à la place.
    pub applied: u32,
}

impl fmt::Display for MaskFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) = {:#010x} : bits au-delà \
             du câble {}, repli sur {:#010x}",
            self.found,
            CABLE_MAX.saturating_sub(1),
            self.applied
        )
    }
}

/// Écrête un masque lu dans le registre à ses bits utiles : **jamais d'échec**.
///
/// Comme [`crate::params::sanitize`], et pour la même raison — refuser de charger pour une
/// faute de frappe dans un `REG_DWORD` coûterait à l'administrateur toutes les cartes son
/// virtuelles du poste. Les bits au-delà du dernier câble sont ignorés et signalés.
///
/// **Un masque nul n'est pas corrigé**, et c'est la différence avec
/// [`crate::params::sanitize`], qui remonte une réserve de 0 à 1 : zéro câble actif est un
/// choix légitime de l'utilisateur (tous les câbles rangés sous « Périphériques
/// déconnectés »), là où zéro câble enregistré est un pilote qui n'a plus de raison
/// d'être. Corriger ici reviendrait à rallumer un câble que l'utilisateur vient
/// d'éteindre, à chaque redémarrage.
#[must_use]
pub const fn sanitize_mask(raw: u32) -> (u32, Option<MaskFix>) {
    let retenu = raw & ACTIVE_CABLES_MASK;
    if retenu == raw {
        (retenu, None)
    } else {
        (
            retenu,
            Some(MaskFix {
                found: raw,
                applied: retenu,
            }),
        )
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::string::ToString;
    use std::vec::Vec;

    /// Un tampon de la bonne longueur, aux quatre champs donnés.
    fn octets(cable: u32, connected: u32, channels: u32, reserved: u32) -> [u8; 16] {
        CableState {
            cable,
            connected,
            channels,
            reserved,
        }
        .to_bytes()
    }

    /// Un tampon valide de référence : câble 0, connecté, deux canaux.
    fn valide() -> [u8; 16] {
        octets(0, 1, DEFAULT_CHANNELS, 0)
    }

    /// Le GUID du jeu est bien formé, et il est celui qu'on a gravé.
    ///
    /// Les deux moitiés du test comptent : la valeur littérale attrape une modification
    /// accidentelle (que rien d'autre ne signalerait — la propriété deviendrait
    /// simplement introuvable), la forme attrape un GUID recopié de travers.
    #[test]
    fn le_guid_du_jeu_est_grave() {
        assert_eq!(KSPROPSETID_CONDUIT.data1, 0x3F1B_27A4);
        assert_eq!(KSPROPSETID_CONDUIT.data2, 0x8C6E);
        assert_eq!(KSPROPSETID_CONDUIT.data3, 0x4D02);
        assert_eq!(
            KSPROPSETID_CONDUIT.data4,
            [0x9B, 0x75, 0xE4, 0xA0, 0xD6, 0x1C, 0x8F, 0x3B]
        );
        // UUID version 4 (aléatoire) : les quatre bits hauts de `data3` valent 4.
        assert_eq!(KSPROPSETID_CONDUIT.data3 >> 12, 4);
        // Variante RFC 4122 : les deux bits hauts du premier octet de `data4` valent 10.
        assert_eq!(KSPROPSETID_CONDUIT.data4[0] >> 6, 0b10);
        // Ni nul, ni le GUID de base des noms de broche (`portcls::adapter`).
        assert_ne!(KSPROPSETID_CONDUIT.data1, 0);
        assert_ne!(KSPROPSETID_CONDUIT.data1, 0xCAA7_4E3D);
    }

    /// Les deux identifiants de propriété sont distincts et stables.
    #[test]
    fn les_identifiants_de_propriete_sont_distincts() {
        assert_eq!(KSPROPERTY_CONDUIT_CABLE_STATE, 0);
        assert_eq!(KSPROPERTY_CONDUIT_VERSION, 1);
        assert_ne!(
            KSPROPERTY_CONDUIT_CABLE_STATE, KSPROPERTY_CONDUIT_VERSION,
            "deux propriétés du même jeu ne peuvent pas partager un identifiant : \
             PortCls cherche la première entrée de même Set/Id et servirait la mauvaise"
        );
        assert_eq!(CONFIG_VERSION, 1);
    }

    /// Le `pid` de la marque : celui qu'écrit le service et celui que lit le dorsal.
    ///
    /// Il vit ici parce que **deux** crates s'y donnent rendez-vous ; si quelqu'un le
    /// change, `registre::valeur_marque` et `devices::MARQUE_KEY` bougent ensemble, et
    /// les endpoints déjà renommés d'une machine deviennent orphelins — d'où la valeur
    /// écrite en toutes lettres.
    #[test]
    fn le_pid_de_la_marque_est_grave() {
        assert_eq!(PID_MARQUE_CABLE, 1);
        // Le `pid` 0 est réservé par le système de propriétés Windows.
        assert_ne!(PID_MARQUE_CABLE, 0);
    }

    /// La disposition : seize octets, quatre `ULONG`, aucun trou.
    #[test]
    fn la_disposition_est_celle_des_decalages_nommes() {
        assert_eq!(CABLE_STATE_BYTES, 16);
        assert_eq!(size_of::<CableState>(), CABLE_STATE_BYTES);
        let decalages = [O_CABLE, O_CONNECTED, O_CHANNELS, O_RESERVED];
        for (i, decalage) in decalages.iter().enumerate() {
            assert_eq!(*decalage, i * TAILLE_MOT, "décalage n° {i}");
        }
        assert_eq!(
            decalages.len() * TAILLE_MOT,
            CABLE_STATE_BYTES,
            "la structure est exactement ces quatre mots"
        );

        // Chaque champ se relit à son décalage : un `to_bytes` qui intervertirait deux
        // champs passerait l'aller-retour sans que rien ne le signale.
        let brut = octets(3, 1, 2, 0);
        assert_eq!(mot(&brut, O_CABLE), Some(3));
        assert_eq!(mot(&brut, O_CONNECTED), Some(1));
        assert_eq!(mot(&brut, O_CHANNELS), Some(2));
        assert_eq!(mot(&brut, O_RESERVED), Some(0));
    }

    /// Table des longueurs : exacte, tronquée d'un octet, allongée d'un octet, vide.
    ///
    /// L'allongement est le cas qui compte : un parseur qui accepterait un préfixe valide
    /// rendrait `Ok` sur les dix-sept octets, et c'est le premier écart qu'un fuzzer
    /// trouve.
    #[test]
    fn longueur_table() {
        let complet = valide();

        // Taille exacte : accepté.
        assert!(CableState::from_bytes(&complet).is_ok());

        // Tronqué d'un octet : refusé, et la cause nomme les deux longueurs.
        assert_eq!(
            CableState::from_bytes(&complet[..15]),
            Err(ConfigError::Longueur { recus: 15 })
        );
        // Allongé d'un octet : un préfixe parfaitement valide, refusé quand même.
        let mut trop_long = Vec::from(complet);
        trop_long.push(0);
        assert_eq!(
            CableState::from_bytes(&trop_long),
            Err(ConfigError::Longueur { recus: 17 })
        );
        // L'octet en trop n'a même pas besoin d'être nul pour changer quoi que ce soit.
        let mut trop_long = Vec::from(complet);
        trop_long.push(0xFF);
        assert_eq!(
            CableState::from_bytes(&trop_long),
            Err(ConfigError::Longueur { recus: 17 })
        );

        // Vide (interrogation de taille de KS) et très long : mêmes refus.
        assert_eq!(
            CableState::from_bytes(&[]),
            Err(ConfigError::Longueur { recus: 0 })
        );
        let enorme = std::vec![0u8; 4096];
        assert_eq!(
            CableState::from_bytes(&enorme),
            Err(ConfigError::Longueur { recus: 4096 })
        );

        // Toutes les longueurs de 0 à 32 sauf la bonne sont refusées, sans exception.
        for taille in 0..=32usize {
            let tampon = std::vec![0u8; taille];
            let resultat = CableState::from_bytes(&tampon);
            if taille == CABLE_STATE_BYTES {
                // Un tampon nul est refusé par le domaine (canaux = 0), pas par la
                // longueur : c'est ce qui distingue les deux contrôles.
                assert_eq!(resultat, Err(ConfigError::Channels(0)), "taille {taille}");
            } else {
                assert_eq!(
                    resultat,
                    Err(ConfigError::Longueur { recus: taille }),
                    "taille {taille}"
                );
            }
        }
    }

    /// Table du numéro de câble : dans le domaine, à la borne, au-delà, saturé.
    #[test]
    fn cable_table() {
        let cases: [(u32, Option<ConfigError>); 6] = [
            (0, None),
            (1, None),
            (CABLE_MAX - 1, None),
            (CABLE_MAX, Some(ConfigError::Cable(CABLE_MAX))),
            (CABLE_MAX + 1, Some(ConfigError::Cable(CABLE_MAX + 1))),
            (u32::MAX, Some(ConfigError::Cable(u32::MAX))),
        ];
        for (cable, attendu) in cases {
            let brut = octets(cable, 1, DEFAULT_CHANNELS, 0);
            match attendu {
                None => assert_eq!(
                    CableState::from_bytes(&brut).map(|s| s.cable),
                    Ok(cable),
                    "câble {cable}"
                ),
                Some(err) => assert_eq!(CableState::from_bytes(&brut), Err(err), "câble {cable}"),
            }
        }
    }

    /// Table de l'état de connexion : **0 et 1 seulement**, pas la convention `BOOL`.
    #[test]
    fn connected_table() {
        assert_eq!(
            CableState::from_bytes(&octets(0, 0, DEFAULT_CHANNELS, 0)).map(|s| s.is_connected()),
            Ok(false)
        );
        assert_eq!(
            CableState::from_bytes(&octets(0, 1, DEFAULT_CHANNELS, 0)).map(|s| s.is_connected()),
            Ok(true)
        );
        // Toute autre valeur est refusée, y compris celles qu'un `BOOL` de KS accepterait.
        for brut in [2, 0xFF, 0xFFFF_FFFF, 0x8000_0000] {
            assert_eq!(
                CableState::from_bytes(&octets(0, brut, DEFAULT_CHANNELS, 0)),
                Err(ConfigError::Connected(brut)),
                "connected {brut}"
            );
        }
    }

    /// Table des canaux : les bornes de `params`, ni plus ni moins.
    #[test]
    fn channels_table() {
        let cases: [(u32, Option<ConfigError>); 7] = [
            (0, Some(ConfigError::Channels(0))),
            (MIN_CHANNELS, None),
            (DEFAULT_CHANNELS, None),
            (MAX_CHANNELS, None),
            (
                MAX_CHANNELS + 1,
                Some(ConfigError::Channels(MAX_CHANNELS + 1)),
            ),
            (99, Some(ConfigError::Channels(99))),
            (u32::MAX, Some(ConfigError::Channels(u32::MAX))),
        ];
        for (channels, attendu) in cases {
            let brut = octets(0, 1, channels, 0);
            let resultat = CableState::from_bytes(&brut);
            match attendu {
                None => assert_eq!(
                    resultat.map(|s| s.channels),
                    Ok(channels),
                    "canaux {channels}"
                ),
                Some(err) => assert_eq!(resultat, Err(err), "canaux {channels}"),
            }
        }

        // Le domaine est plus large que ce que M1b-04 sait appliquer : la validation
        // accepte 1 à 8, le gestionnaire n'appliquera que 2 (voir l'en-tête de module).
        let huit = CableState::from_bytes(&octets(0, 1, MAX_CHANNELS, 0)).unwrap();
        assert!(!huit.channels_applicables(), "M1b-05 n'est pas faite");
        let deux = CableState::from_bytes(&octets(0, 1, DEFAULT_CHANNELS, 0)).unwrap();
        assert!(deux.channels_applicables());
    }

    /// Table du champ réservé : nul, ou refusé.
    #[test]
    fn reserved_table() {
        assert!(CableState::from_bytes(&octets(0, 1, DEFAULT_CHANNELS, 0)).is_ok());
        for brut in [1, 0x8000_0000, 0xDEAD_BEEF, u32::MAX] {
            assert_eq!(
                CableState::from_bytes(&octets(0, 1, DEFAULT_CHANNELS, brut)),
                Err(ConfigError::Reserved(brut)),
                "réservé {brut:#x}"
            );
        }
    }

    /// L'ordre des contrôles suit celui des champs : la cause nommée est la première
    /// anomalie, pas une arbitraire.
    #[test]
    fn la_cause_nommee_est_la_premiere_anomalie() {
        // Les quatre champs fautifs à la fois : c'est le câble qui est signalé.
        let tout_faux = octets(99, 7, 0, 42);
        assert_eq!(
            CableState::from_bytes(&tout_faux),
            Err(ConfigError::Cable(99))
        );
        // Câble réparé : l'état de connexion prend la suite.
        let brut = octets(0, 7, 0, 42);
        assert_eq!(
            CableState::from_bytes(&brut),
            Err(ConfigError::Connected(7))
        );
        // Puis les canaux, puis le réservé.
        assert_eq!(
            CableState::from_bytes(&octets(0, 1, 0, 42)),
            Err(ConfigError::Channels(0))
        );
        assert_eq!(
            CableState::from_bytes(&octets(0, 1, 2, 42)),
            Err(ConfigError::Reserved(42))
        );
    }

    /// Le message dit quel champ, quelle valeur, quel domaine — pas « paramètre
    /// invalide ».
    #[test]
    fn le_message_nomme_le_champ_et_sa_valeur() {
        let ligne = ConfigError::Longueur { recus: 17 }.to_string();
        assert!(ligne.contains("17"), "{ligne}");
        assert!(ligne.contains("16"), "{ligne}");

        let ligne = ConfigError::Cable(99).to_string();
        assert!(ligne.contains("99"), "{ligne}");
        assert!(ligne.contains("câble"), "{ligne}");

        let ligne = ConfigError::Connected(7).to_string();
        assert!(ligne.contains('7'), "{ligne}");

        let ligne = ConfigError::Channels(9).to_string();
        assert!(ligne.contains('9'), "{ligne}");
        assert!(ligne.contains("canaux"), "{ligne}");

        let ligne = ConfigError::Reserved(0xDEAD_BEEF).to_string();
        assert!(ligne.contains("réservé"), "{ligne}");
        assert!(ligne.contains("deadbeef"), "{ligne}");
    }

    /// Le masque : bit par câble, défaut 0x3, écrêtage des bits qui n'existent pas.
    #[test]
    fn masque_table() {
        assert_eq!(ACTIVE_CABLES_DEFAULT, 0b11);
        assert_eq!(ACTIVE_CABLES_MASK, 0xFFFF);
        assert_eq!(ACTIVE_CABLES_VALUE_NAME, "ActiveCables");

        // Le défaut : les deux premiers câbles, et eux seuls.
        assert!(is_active(ACTIVE_CABLES_DEFAULT, 0));
        assert!(is_active(ACTIVE_CABLES_DEFAULT, 1));
        for cable in 2..CABLE_MAX {
            assert!(!is_active(ACTIVE_CABLES_DEFAULT, cable), "câble {cable}");
        }
        // Un câble inexistant n'est jamais actif, quelle que soit la valeur du masque.
        assert!(!is_active(u32::MAX, CABLE_MAX));
        assert!(!is_active(u32::MAX, u32::MAX));

        // Le bit de chaque câble est bien 1 << n, et il n'y en a pas au-delà.
        for cable in 0..CABLE_MAX {
            assert_eq!(cable_bit(cable), Some(1 << cable), "câble {cable}");
        }
        assert_eq!(cable_bit(CABLE_MAX), None);
        assert_eq!(cable_bit(u32::MAX), None);

        // Poser puis retirer un bit revient au masque de départ.
        let masque = with_active(ACTIVE_CABLES_DEFAULT, 5, true);
        assert_eq!(masque, 0b10_0011);
        assert!(is_active(masque, 5));
        assert_eq!(with_active(masque, 5, false), ACTIVE_CABLES_DEFAULT);
        // Un câble inexistant laisse le masque intact plutôt que d'y perdre un bit.
        assert_eq!(with_active(masque, CABLE_MAX, true), masque);
        assert_eq!(with_active(masque, u32::MAX, false), masque);
    }

    /// Écrêtage du masque lu : les bits utiles passent, les autres sont signalés.
    #[test]
    fn sanitize_mask_table() {
        let cases: [(u32, u32, bool); 7] = [
            // Dans le domaine : rien à signaler.
            (0x0000, 0x0000, false),
            (0x0003, 0x0003, false),
            (0xFFFF, 0xFFFF, false),
            // Bits au-delà du câble 15 : ignorés et signalés.
            (0x0001_0000, 0x0000, true),
            (0x0001_0003, 0x0003, true),
            (u32::MAX, 0xFFFF, true),
            (0x8000_0000, 0x0000, true),
        ];
        for (brut, attendu, corrige) in cases {
            let (retenu, fix) = sanitize_mask(brut);
            assert_eq!(retenu, attendu, "masque {brut:#010x}");
            assert_eq!(fix.is_some(), corrige, "masque {brut:#010x}");
            if let Some(fix) = fix {
                assert_eq!(fix.found, brut);
                assert_eq!(fix.applied, attendu);
                let ligne = fix.to_string();
                assert!(ligne.contains("ActiveCables"), "{ligne}");
            }
        }

        // Un masque nul n'est PAS corrigé : zéro câble actif est un choix de
        // l'utilisateur, contrairement à zéro câble enregistré (`params::sanitize`).
        let (retenu, fix) = sanitize_mask(0);
        assert_eq!(retenu, 0);
        assert!(
            fix.is_none(),
            "rallumer un câble éteint serait une régression"
        );

        // Utilisable dans un contexte constant, comme `params::sanitize`.
        const LU: (u32, Option<MaskFix>) = sanitize_mask(0x0003);
        assert_eq!(LU.0, 0x0003);
    }

    /// Le masque et le décodage du registre s'enchaînent comme le pilote les enchaîne.
    #[test]
    fn le_decodage_du_registre_alimente_le_masque() {
        use crate::params::{decode_dword, REG_DWORD};
        // `REG_DWORD` est petit-boutiste par définition de `winnt.h` : 0x0003, pas
        // 0x0300_0000 (voir la note de boutisme en tête de module).
        let lu = decode_dword(REG_DWORD, &[0x03, 0x00, 0x00, 0x00]).unwrap();
        let (masque, fix) = sanitize_mask(lu);
        assert_eq!(masque, ACTIVE_CABLES_DEFAULT);
        assert!(fix.is_none());
        assert!(is_active(masque, 0) && is_active(masque, 1) && !is_active(masque, 2));
    }

    proptest! {
        /// Le critère de M1b-08 : **quelle que soit l'entrée**, le parseur ne panique
        /// pas, et toute sortie acceptée se resérialise à l'identique.
        ///
        /// L'aller-retour est ce qui attrape un champ lu au mauvais décalage : un
        /// `connected` lu à la place du `cable` passerait toutes les tables de cas
        /// ci-dessus (les deux valeurs y sont souvent 0 ou 1) mais pas celle-ci.
        #[test]
        fn rien_ne_panique_et_l_aller_retour_est_fidele(
            octets in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            match CableState::from_bytes(&octets) {
                Ok(etat) => {
                    prop_assert_eq!(octets.len(), CABLE_STATE_BYTES);
                    prop_assert_eq!(&etat.to_bytes()[..], &octets[..]);
                    prop_assert_eq!(CableState::from_bytes(&etat.to_bytes()), Ok(etat));
                    // Toute sortie acceptée est dans les domaines annoncés.
                    prop_assert!(etat.cable < CABLE_MAX);
                    prop_assert!(etat.connected <= 1);
                    prop_assert!((MIN_CHANNELS..=MAX_CHANNELS).contains(&etat.channels));
                    prop_assert_eq!(etat.reserved, 0);
                }
                Err(err) => {
                    // Une erreur de longueur exactement quand la longueur est fausse :
                    // les autres causes ne peuvent tomber que sur seize octets.
                    let longueur = matches!(err, ConfigError::Longueur { .. });
                    prop_assert_eq!(longueur, octets.len() != CABLE_STATE_BYTES);
                }
            }
        }

        /// Sur tout le domaine des quatre champs : accepté si et seulement si les quatre
        /// sont dans leurs bornes.
        #[test]
        fn accepte_exactement_les_quatre_champs_valides(
            cable in any::<u32>(),
            connected in any::<u32>(),
            channels in any::<u32>(),
            reserved in any::<u32>(),
        ) {
            let brut = octets(cable, connected, channels, reserved);
            let valide = cable < CABLE_MAX
                && connected <= 1
                && (MIN_CHANNELS..=MAX_CHANNELS).contains(&channels)
                && reserved == 0;
            prop_assert_eq!(CableState::from_bytes(&brut).is_ok(), valide);
        }

        /// Le masque : poser puis retirer un bit est l'identité, et l'écrêtage ne touche
        /// jamais aux bits utiles.
        #[test]
        fn le_masque_est_reversible_et_l_ecretage_conserve_les_bits_utiles(
            masque in any::<u32>(),
            cable in 0_u32..CABLE_MAX,
            actif in any::<bool>(),
        ) {
            let pose = with_active(masque, cable, actif);
            prop_assert_eq!(is_active(pose, cable), actif);
            // Les autres câbles n'ont pas bougé.
            for autre in 0..CABLE_MAX {
                if autre != cable {
                    prop_assert_eq!(is_active(pose, autre), is_active(masque, autre));
                }
            }
            // L'écrêtage est idempotent et ne perd aucun bit utile.
            let (retenu, _) = sanitize_mask(masque);
            prop_assert_eq!(retenu & ACTIVE_CABLES_MASK, retenu);
            prop_assert_eq!(sanitize_mask(retenu).0, retenu);
            for c in 0..CABLE_MAX {
                prop_assert_eq!(is_active(retenu, c), is_active(masque, c));
            }
        }
    }
}
