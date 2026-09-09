//! Contrat du jeu de propriétés KS **privé** de configuration (M1b-04,
//! `docs/driver-design.md` §6) : le GUID du jeu, les identifiants de propriété, les deux
//! structures d'échange ([`CableState`] et, depuis M1b-21, [`CableCounters`]), leurs
//! parseurs, et le masque de bits qui persiste l'état des câbles.
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
//! règle qui gouverne [`CableState::from_bytes`] — et, à l'identique,
//! [`CableCounters::from_bytes`] :
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
//! # Ce que ce module valide, et ce qui l'applique (M1b-05)
//!
//! Le champ [`CableState::channels`] est **validé** contre les bornes de
//! [`crate::params`] et rendu par la propriété ; ce n'est plus une valeur morte. Depuis
//! M1b-05 les descripteurs KS du pilote sont une table indexée par (fréquence, canaux) et
//! le nombre de canaux est celui que le registre fixe pour le câble, par la valeur
//! `CableFormat<n>` dont [`CableFormat`] est le codec.
//!
//! **[`CableState::channels`] est un écho vérifié, comme [`CableState::cable`]** — pas un
//! ordre. La structure d'échange est `GET`/`SET`, mais ce champ n'est pas un réglage : un
//! format ne peut pas changer sans redémarrer le périphérique (les tables KS sont
//! immuables et PortCls en retient les pointeurs à vie), et le seul chemin qui redémarre
//! le devnode est en espace utilisateur — `cfgmgr32`, dans le service d'assistance. Un
//! `SET` qui prétendrait changer les canaux rendrait `STATUS_SUCCESS` pour un réglage sans
//! effet jusqu'au prochain démarrage, ce qui est pire qu'un refus : le gestionnaire exige
//! donc que le champ **égale la valeur courante du câble**
//! ([`CableState::channels_appliquables`]).
//!
//! Un client qui veut seulement **brancher le jack** ne fabrique donc pas sa requête : il
//! relit l'état par un `GET` et n'en change que la connexion
//! ([`CableState::avec_connexion`]). La décision et ce qu'elle écarte sont écrites sur le
//! champ lui-même.
//!
//! # Le format d'un câble est une configuration, pas un état
//!
//! [`ACTIVE_CABLES_VALUE_NAME`] est de l'**état** : le pilote l'écrit à chaque `SET`, une
//! seule valeur pour les seize câbles. [`CABLE_FORMAT_VALUE_NAMES`] est de la
//! **configuration** : le pilote ne fait que la **lire au démarrage**, exactement comme
//! `ReserveSize`, et c'est le service qui l'écrit puis redémarre le devnode. D'où une
//! valeur **par câble** — un format ne tient pas dans deux bits, et personne n'a besoin
//! qu'une écriture couvre les seize d'un coup, puisqu'il faut de toute façon redémarrer.

use core::fmt;

use crate::format::{sample_rate_index, RATE_44100, RATE_48000, RATE_96000};
use crate::params::{DEFAULT_CHANNELS, MAX_CHANNELS, MAX_RESERVE, MIN_CHANNELS};
use crate::ring::SampleFormat;

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

/// `KSPROPERTY_CONDUIT_COUNTERS` : les compteurs de la boucle locale du câble
/// (`GET` seulement).
///
/// La valeur échangée est une [`CableCounters`]. Comme pour
/// [`KSPROPERTY_CONDUIT_CABLE_STATE`], le câble n'est pas désigné par un paramètre de la
/// requête mais par le filtre auquel elle s'adresse ; [`CableCounters::cable`] n'est qu'un
/// **écho**, et il n'y a rien à comparer puisqu'il n'y a pas de `SET`.
///
/// # Pourquoi une propriété, et pas une ligne de journal
///
/// Les compteurs existaient déjà (M1b-07) mais ne se lisaient qu'au **débogueur noyau** :
/// `kmd_log!` est vide en release. Or attacher le débogueur fausse précisément ce qu'on
/// cherche à mesurer — 17 passes sur 20 attaché contre 20 sur 20 détaché, mesuré le
/// 2026-09-08 — et coûte un redémarrage de la machine virtuelle, qui ferme la session
/// console dont l'audio a besoin. La moitié « rendu seul » de M1b-07 restait donc
/// invisible. Une propriété `GET` la rend lisible **sans débogueur**, session console
/// ouverte, par le chemin `IOCTL_KS_PROPERTY` déjà en place.
///
/// # Pas de `SET`, et pas de contrôle de privilège
///
/// Un compteur n'est pas un réglage : il n'y a rien à écrire, donc le bit `SET` n'est pas
/// déclaré (comme [`KSPROPERTY_CONDUIT_VERSION`]). Et la lecture est libre, comme celle de
/// l'état : savoir combien de ticks un câble a jetés n'apprend rien qu'un compte
/// privilégié devrait seul connaître, et un diagnostic qui exigerait l'élévation ne
/// servirait pas là où il sert — sur la machine où plus rien ne marche.
pub const KSPROPERTY_CONDUIT_COUNTERS: u32 = 2;

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
/// À incrémenter **à chaque** changement observable du jeu de propriétés — un champ
/// ajouté à une structure d'échange, un domaine élargi, une sémantique modifiée, une
/// propriété qui apparaît. Le service d'assistance compare, refuse de piloter un pilote
/// qu'il ne connaît pas, et le dit ; sans ce numéro, la panne serait un
/// `STATUS_INVALID_PARAMETER` inexplicable sur une longueur d'un octet de trop.
///
/// # Pourquoi 2 : une sémantique qui bouge sans que la forme bouge
///
/// La forme de [`CableState`] n'a pas bougé d'un octet en M1b-05 ; sa **sémantique**, si,
/// et c'est exactement le cas que ce numéro doit couvrir. En version 1, `channels` valait
/// toujours 2 et un `SET` était refusé au-delà : un service pouvait en déduire que le
/// pilote était stéréo. En version 2, il porte le nombre de canaux réellement servi par ce
/// câble-là, entre 1 et 8, et le `SET` exige cette valeur-là et non plus 2
/// ([`CableState::channels_appliquables`]). Un service v1 devant un pilote v2 lirait « 6 »
/// et conclurait à un pilote cassé ; il reçoit un refus de version, qui dit quoi faire.
///
/// # Pourquoi 3 : une propriété qui apparaît est un changement du contrat
///
/// M1b-21 ajoute [`KSPROPERTY_CONDUIT_COUNTERS`] au jeu, sans toucher à un seul octet de
/// [`CableState`] ni à sa sémantique. Le numéro monte quand même, et la raison est écrite
/// deux lignes plus haut, sur [`KSPROPSETID_CONDUIT`] : **le GUID du jeu est gravé et la
/// longueur des tampons est refusée dès qu'elle change, donc ce numéro est la seule voie
/// de versionnement qui reste**. Ne pas le monter ferait désigner par « version 2 » deux
/// jeux de propriétés différents — celui à deux propriétés et celui à trois — sans qu'aucun
/// canal du contrat ne puisse les distinguer. Un client aurait alors pour seul recours
/// d'envoyer la requête et d'interpréter le refus, c'est-à-dire de deviner la version au
/// lieu de la lire, ce que cette constante existe précisément pour éviter.
///
/// # Ce que le passage de 2 à 3 **ne** dit **pas**, et ce que les messages doivent en tenir
///
/// Un changement **additif** : un client v2 devant un pilote v3 lit exactement la même
/// [`CableState`], avec la même sémantique, et tout ce qu'il sait faire continue de
/// marcher. C'est la différence avec 1 → 2, où l'ancien client se **trompait** sur ce qu'il
/// lisait. Un message d'inadéquation ne doit donc pas affirmer que « la structure
/// d'échange n'a pas la forme attendue » : il ne sait pas ce qui diffère, il sait
/// seulement que le pilote et l'outil ne sont pas du même millésime. C'est ainsi que
/// `conduit-looptest` le formule depuis M1b-21 ; le service d'assistance, lui, se contente
/// de rapporter les deux numéros et ne refuse rien sur ce seul critère.
pub const CONFIG_VERSION: u32 = 3;

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
    /// **Un écho vérifié, pas un ordre** — la même nature que [`Self::cable`], et la
    /// décision est écrite ici parce que le champ en admettait deux lectures depuis
    /// M1b-05.
    ///
    /// Au `GET`, c'est le nombre de canaux que le câble sert réellement, celui de
    /// [`CableFormat::channels`] lu au démarrage. Au `SET`, le gestionnaire le
    /// **compare** à cette valeur et refuse `STATUS_INVALID_PARAMETER` s'il diffère
    /// ([`Self::channels_appliquables`]) : un client qui croirait le câble stéréo alors
    /// qu'il est en 5.1 se verrait sinon accepter un branchement fondé sur une idée fausse
    /// du format, et découvrirait l'écart au premier flux audio.
    ///
    /// # Ce que la lecture « ordre » aurait voulu dire, et pourquoi elle est écartée
    ///
    /// « Mets ce câble à *N* canaux » demanderait au pilote de reconstruire ses
    /// descripteurs à chaud, ce que PortCls interdit : ses tables sont immuables et il en
    /// retient les pointeurs pour toute la vie du filtre. Le seul `SET` que le pilote
    /// pourrait honorer serait celui qui ne change rien — c'est-à-dire un ordre dont la
    /// seule valeur acceptable est la valeur courante, donc un écho déguisé. Le changement
    /// de format passe par [`CABLE_FORMAT_VALUE_NAMES`] et un redémarrage du devnode, tous
    /// deux en espace utilisateur.
    ///
    /// # Ce qu'envoie un client qui ne veut rien dire des canaux
    ///
    /// **Ce que le `GET` vient de lui rendre.** C'est la différence pratique avec
    /// [`Self::cable`] : le câble visé, un client le connaît par le descripteur qu'il a
    /// ouvert, tandis que le format, il ne peut que le lire. Un `SET` est donc toujours
    /// une lecture-modification-écriture, et [`Self::avec_connexion`] est ce geste en une
    /// méthode. Le coût est nul là où il compte : le seul écrivain du dépôt — le service
    /// d'assistance — relit déjà l'état avant d'écrire, pour dire dans son journal ce qui
    /// a changé.
    ///
    /// Une valeur sentinelle (0 = « peu importe ») aurait évité cette relecture. Elle est
    /// écartée : elle élargit le domaine du contrat que M1b-08 fuzze, elle change la
    /// sémantique observable de la structure — donc [`CONFIG_VERSION`] —, et elle rend
    /// muet le seul cas que la comparaison attrape, celui d'un client qui se trompe sur le
    /// format. Un champ qu'on a le droit de ne pas remplir ne vérifie plus rien.
    ///
    /// Le domaine est celui de [`crate::params::Param::Channels`] — une seule définition
    /// des bornes, comme pour le registre.
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
    /// L'état d'un câble **neuf** : connecté ou non, canaux par défaut, réservé nul.
    ///
    /// # À ne pas envoyer en `SET` d'un câble dont on n'a pas lu le format
    ///
    /// Le [`Self::channels`] que cette fonction pose est [`DEFAULT_CHANNELS`], c'est-à-dire
    /// ce que sert un câble **fraîchement installé** — pas ce que sert le câble qu'on a
    /// sous la main. L'envoyer tel quel à un câble configuré autrement le fait refuser par
    /// le gestionnaire, et le client n'en voit qu'un `STATUS_INVALID_PARAMETER`
    /// (`ERROR_INVALID_PARAMETER`, 87 côté Win32) qu'il ne saura pas relier au format.
    /// C'est exactement le défaut mesuré entre M1b-05 et M1b-20 : le pilote avait changé de
    /// contrat, le client fabriquait encore un « 2 » universel.
    ///
    /// Pour modifier l'état d'un câble existant, c'est [`Self::avec_connexion`] sur ce
    /// qu'un `GET` vient de rendre. Cette fonction reste le point de départ des tests, et
    /// le repli du service quand la relecture échoue et qu'il ne reste rien de mieux.
    #[must_use]
    pub const fn new(cable: u32, connected: bool) -> Self {
        Self {
            cable,
            connected: if connected { 1 } else { 0 },
            channels: DEFAULT_CHANNELS,
            reserved: 0,
        }
    }

    /// L'état **relu**, la seule connexion changée : la lecture-modification-écriture en
    /// une méthode.
    ///
    /// C'est le geste que tout client de `SET` doit faire, et la conséquence directe de la
    /// nature d'écho de [`Self::cable`] et [`Self::channels`] : les deux champs sont
    /// **comparés** par le gestionnaire, donc les deux se reprennent de l'état lu au lieu
    /// d'être fabriqués. Le champ réservé repart à zéro, seule valeur que le contrat
    /// accepte dans les deux sens.
    ///
    /// Sur un état sorti de [`Self::from_bytes`], le résultat est valide par construction :
    /// `cable` et `channels` sont ceux que le pilote vient d'annoncer, et `connected` est
    /// dans `{0, 1}`.
    #[must_use]
    pub const fn avec_connexion(&self, connected: bool) -> Self {
        Self {
            cable: self.cable,
            connected: if connected { 1 } else { 0 },
            channels: self.channels,
            reserved: 0,
        }
    }

    /// L'état actif, en booléen. `connected` est déjà validé dans `{0, 1}` par
    /// [`Self::from_bytes`].
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected != 0
    }

    /// Le nombre de canaux demandé est-il celui que **ce câble** sert (`courants`) ?
    ///
    /// [`Self::from_bytes`] accepte tout le domaine `MIN_CHANNELS..=MAX_CHANNELS`, parce
    /// que c'est le domaine du contrat et que le fuzzer doit l'explorer en entier. Le
    /// gestionnaire de `SET`, lui, exige l'égalité avec la valeur courante — non plus
    /// avec [`DEFAULT_CHANNELS`], comme avant M1b-05, mais avec ce que le registre a fixé
    /// pour ce câble-là.
    ///
    /// La raison a changé de nature et pas de conclusion. Avant M1b-05, le pilote ne
    /// *savait* pas servir autre chose que deux canaux. Depuis, il sait les servir tous —
    /// mais pas **en changer à chaud** : les tables KS sont immuables et PortCls en retient
    /// les pointeurs pour toute la vie du filtre (`descriptors::Shared`). Accepter un `SET`
    /// à six canaux rendrait `STATUS_SUCCESS` pour un réglage qui n'agirait sur rien avant
    /// le prochain démarrage du périphérique, ce qui est pire qu'un refus. Le changement
    /// de format passe donc par le registre **et** un redémarrage du devnode, tous deux en
    /// espace utilisateur.
    #[must_use]
    pub const fn channels_appliquables(&self, courants: u32) -> bool {
        self.channels == courants
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
// Les compteurs de la boucle locale (M1b-21).
// ---------------------------------------------------------------------------------

/// Taille d'un `ULONGLONG`, l'unité des compteurs.
const TAILLE_MOT_LONG: usize = 8;

/// `CableCounters::cable` (`ULONG`) : décalage 0.
pub const OC_CABLE: usize = 0;
/// `CableCounters::reserved` (`ULONG`) : décalage 4.
pub const OC_RESERVED: usize = 4;
/// `CableCounters::ticks` (`ULONGLONG`) : décalage 8.
pub const OC_TICKS: usize = 8;
/// `CableCounters::copied` (`ULONGLONG`) : décalage 16.
pub const OC_COPIED: usize = 16;
/// `CableCounters::silenced_no_render` (`ULONGLONG`) : décalage 24.
pub const OC_SILENCED_NO_RENDER: usize = 24;
/// `CableCounters::silenced_before_render` (`ULONGLONG`) : décalage 32.
pub const OC_SILENCED_BEFORE_RENDER: usize = 32;
/// `CableCounters::discarded_ticks` (`ULONGLONG`) : décalage 40.
pub const OC_DISCARDED_TICKS: usize = 40;
/// `CableCounters::overruns` (`ULONGLONG`) : décalage 48.
pub const OC_OVERRUNS: usize = 48;

/// Taille de la valeur de [`KSPROPERTY_CONDUIT_COUNTERS`], en octets : **56**.
///
/// Fixe et vérifiée par assertion `const` contre `size_of::<CableCounters>()`, comme
/// [`CABLE_STATE_BYTES`] : une structure qui grandirait sans que cette constante bouge
/// donnerait un `GET` qui écrit hors de ce que le client a réservé.
pub const CABLE_COUNTERS_BYTES: usize = 56;

/// Les compteurs de la boucle locale d'un câble, tels qu'ils traversent
/// `IOCTL_KS_PROPERTY`.
///
/// # Ce que chaque compteur démontre
///
/// Ce ne sont pas six nombres interchangeables : chacun est le **témoin** d'un régime, et
/// c'est leur combinaison qui distingue des situations qu'un compteur unique confondrait
/// (M1b-07). Les trois lectures qui comptent :
///
/// - `copied` monte, silences à zéro : les deux côtés tournent, le câble transporte ;
/// - `silenced_no_render` monte seul : **capture seule**, l'entrée sans producteur lit du
///   silence, en régime permanent ;
/// - `discarded_ticks` monte seul, `copied` et les deux silences à zéro : **rendu seul**,
///   les trames sont jetées et rien ne s'accumule. C'est le cas qui ne laissait aucune
///   trace avant M1b-07 et qui ne se distinguait pas d'un câble au repos.
///
/// `silenced_before_render` est borné par le décalage du lien — quelques trames à chaque
/// ouverture, pas un régime ; le voir monter sans fin dirait que le lien se refait sans
/// cesse. `overruns` compte les ticks trop en retard pour rattraper, donc les trous.
///
/// # Huit octets pavés en tête, et pourquoi
///
/// `#[repr(C)]`, alignement 8, **rembourrage explicite** : [`Self::cable`] et
/// [`Self::reserved`] sont deux `ULONG` qui pavent ensemble les huit premiers octets, de
/// sorte que le premier `ULONGLONG` tombe sur un multiple de 8 **sans** que le compilateur
/// ait à insérer un trou. Un rembourrage implicite serait de la mémoire noyau non
/// initialisée recopiée vers l'espace utilisateur au `GET` — c'est la même règle que sur
/// [`CableState`], et elle est plus contraignante ici parce que les champs n'ont pas tous
/// la même taille.
///
/// # Un instantané, pas une transaction
///
/// Les six valeurs sont lues **indépendamment** dans le pilote
/// (`conduit_kmd::cable::Cable::counters_snapshot`) : elles peuvent donc mélanger deux
/// ticks. C'est voulu, et la raison est écrite sur cette méthode-là. Rien de ce qu'on
/// cherche à lire n'en souffre : on regarde quels compteurs **bougent**, pas si leur somme
/// est exacte à une trame près.
///
/// # Remis à zéro à chaque `StartDevice`
///
/// Comme les compteurs qu'ils recopient. Une valeur qui additionnerait les cycles de
/// périphérique précédents ferait croire à une image obsolète du pilote — plusieurs heures
/// perdues ainsi le 2026-09-06.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct CableCounters {
    /// Numéro du câble, de 0 à [`CABLE_MAX`] − 1.
    ///
    /// **Un écho, comme [`CableState::cable`]**, et ici rien de plus : il n'y a pas de
    /// `SET`, donc rien à comparer. Il dit au lecteur quel câble il vient d'interroger,
    /// sans qu'il ait à se souvenir du descripteur qu'il a ouvert — ce qui compte quand on
    /// relève les seize câbles d'affilée.
    pub cable: u32,
    /// Rembourrage **explicite**, toujours nul.
    ///
    /// Deux rôles à la fois : garder libre la place d'un futur champ 32 bits, et amener le
    /// premier `ULONGLONG` sur un multiple de 8 sans trou implicite (voir la note de
    /// structure). Le `GET` l'écrit toujours à zéro, et [`CableCounters::from_bytes`] le
    /// refuse non nul — c'est ce refus qui garde la place réellement libre.
    pub reserved: u32,
    /// Ticks du timer du câble depuis le dernier `StartDevice`.
    pub ticks: u64,
    /// Trames copiées du rendu vers la capture.
    pub copied: u64,
    /// Trames de silence écrites faute de rendu en `RUN` (`SilenceCause::NoRender`) :
    /// « l'entrée sans producteur lit du silence ». Le témoin de la **capture seule**.
    pub silenced_no_render: u64,
    /// Trames de silence écrites alors qu'un rendu tourne, pour des trames antérieures à
    /// son départ (`SilenceCause::BeforeRenderStart`). Borné par le décalage du lien.
    pub silenced_before_render: u64,
    /// Ticks où le rendu tournait **sans capture** : ses trames sont jetées, rien n'est
    /// accumulé. Le témoin du **rendu seul**, et le seul compteur qui ne compte pas des
    /// trames écrites.
    pub discarded_ticks: u64,
    /// Ticks trop en retard pour rattraper : un trou dans la capture.
    pub overruns: u64,
}

// La taille annoncée est celle de la structure, et chaque décalage nommé est celui que
// `repr(C)` produit — même couple d'assertions que pour `CableState`, en remplacement d'un
// golden.
const _: () = assert!(size_of::<CableCounters>() == CABLE_COUNTERS_BYTES);
const _: () = assert!(align_of::<CableCounters>() == 8);
const _: () = assert!(core::mem::offset_of!(CableCounters, cable) == OC_CABLE);
const _: () = assert!(core::mem::offset_of!(CableCounters, reserved) == OC_RESERVED);
const _: () = assert!(core::mem::offset_of!(CableCounters, ticks) == OC_TICKS);
const _: () = assert!(core::mem::offset_of!(CableCounters, copied) == OC_COPIED);
const _: () =
    assert!(core::mem::offset_of!(CableCounters, silenced_no_render) == OC_SILENCED_NO_RENDER);
const _: () = assert!(
    core::mem::offset_of!(CableCounters, silenced_before_render) == OC_SILENCED_BEFORE_RENDER
);
const _: () = assert!(core::mem::offset_of!(CableCounters, discarded_ticks) == OC_DISCARDED_TICKS);
const _: () = assert!(core::mem::offset_of!(CableCounters, overruns) == OC_OVERRUNS);
// Les deux `ULONG` de tête pavent les huit premiers octets, et les six `ULONGLONG` le
// reste : aucun trou, aucun recouvrement, donc aucun octet de rembourrage implicite à
// recopier vers l'espace utilisateur.
const _: () = assert!(OC_RESERVED.saturating_add(TAILLE_MOT) == OC_TICKS);
const _: () = assert!(OC_OVERRUNS.saturating_add(TAILLE_MOT_LONG) == CABLE_COUNTERS_BYTES);

/// Ce qui a fait refuser une [`CableCounters`] : un cas, une cause, une ligne de journal.
///
/// Distinct de [`ConfigError`] parce que les longueurs attendues ne sont pas les mêmes et
/// qu'un message qui annoncerait « 16 attendus » pour une structure de 56 octets serait un
/// diagnostic faux — le genre d'erreur qui coûte une heure à 3 h du matin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountersError {
    /// Longueur du tampon différente de [`CABLE_COUNTERS_BYTES`] — plus courte **ou** plus
    /// longue, préfixe valide compris (voir la règle en tête de module).
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// [`CableCounters::cable`] au-delà du dernier câble.
    Cable(u32),
    /// [`CableCounters::reserved`] non nul.
    Reserved(u32),
}

impl fmt::Display for CountersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "valeur de {recus} octets, {CABLE_COUNTERS_BYTES} attendus exactement"
            ),
            Self::Cable(cable) => write!(f, "câble {cable} inconnu (0 à {} )", {
                CABLE_MAX.saturating_sub(1)
            }),
            Self::Reserved(brut) => write!(f, "champ réservé non nul ({brut:#010x})"),
        }
    }
}

/// Lit un `ULONGLONG` au décalage `offset` d'un tampon d'alignement quelconque.
///
/// Même règle que [`mot`] : les huit octets sont **recopiés** puis interprétés, jamais
/// transtypés — et l'alignement compte davantage ici, un `u64` désaligné n'étant pas
/// seulement mal vu mais un comportement indéfini en Rust.
fn mot_long(data: &[u8], offset: usize) -> Option<u64> {
    let fin = offset.checked_add(TAILLE_MOT_LONG)?;
    let huit = data.get(offset..fin)?;
    let mut octets = [0u8; TAILLE_MOT_LONG];
    octets.copy_from_slice(huit);
    Some(u64::from_ne_bytes(octets))
}

impl CableCounters {
    /// Les compteurs d'un câble qui n'a pas encore tourné : tout à zéro, sauf le numéro.
    #[must_use]
    pub const fn new(cable: u32) -> Self {
        Self {
            cable,
            reserved: 0,
            ticks: 0,
            copied: 0,
            silenced_no_render: 0,
            silenced_before_render: 0,
            discarded_ticks: 0,
            overruns: 0,
        }
    }

    /// Le câble a-t-il tourné en **rendu seul** depuis le dernier `StartDevice` ?
    ///
    /// C'est la lecture que M1b-07 demandait et que le débogueur seul savait faire : des
    /// ticks jetés, et **aucune** trame écrite d'aucune sorte. La conjonction compte — des
    /// ticks jetés à côté de trames copiées ne diraient qu'un rendu qui a commencé avant la
    /// capture, pas un régime.
    #[must_use]
    pub const fn rendu_seul(&self) -> bool {
        self.discarded_ticks > 0
            && self.copied == 0
            && self.silenced_no_render == 0
            && self.silenced_before_render == 0
    }

    /// Le câble a-t-il tourné en **capture seule** depuis le dernier `StartDevice` ?
    ///
    /// Le symétrique : du silence écrit faute de rendu, et rien de copié.
    #[must_use]
    pub const fn capture_seule(&self) -> bool {
        self.silenced_no_render > 0 && self.copied == 0
    }

    /// Sérialise les compteurs en [`CABLE_COUNTERS_BYTES`] octets, champ par champ, aux
    /// décalages [`OC_CABLE`] à [`OC_OVERRUNS`].
    ///
    /// Le pilote n'utilise pas cette fonction pour répondre à un `GET` — il écrit
    /// directement dans le tampon `Value`, d'alignement quelconque, par
    /// `portcls::property::Champs` — mais elle en est le miroir exact, et c'est elle que
    /// l'aller-retour du proptest vérifie.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; CABLE_COUNTERS_BYTES] {
        let cable = self.cable.to_ne_bytes();
        let reserved = self.reserved.to_ne_bytes();
        let ticks = self.ticks.to_ne_bytes();
        let copied = self.copied.to_ne_bytes();
        let sans_rendu = self.silenced_no_render.to_ne_bytes();
        let avant_rendu = self.silenced_before_render.to_ne_bytes();
        let jetes = self.discarded_ticks.to_ne_bytes();
        let debordements = self.overruns.to_ne_bytes();
        [
            cable[0],
            cable[1],
            cable[2],
            cable[3],
            reserved[0],
            reserved[1],
            reserved[2],
            reserved[3],
            ticks[0],
            ticks[1],
            ticks[2],
            ticks[3],
            ticks[4],
            ticks[5],
            ticks[6],
            ticks[7],
            copied[0],
            copied[1],
            copied[2],
            copied[3],
            copied[4],
            copied[5],
            copied[6],
            copied[7],
            sans_rendu[0],
            sans_rendu[1],
            sans_rendu[2],
            sans_rendu[3],
            sans_rendu[4],
            sans_rendu[5],
            sans_rendu[6],
            sans_rendu[7],
            avant_rendu[0],
            avant_rendu[1],
            avant_rendu[2],
            avant_rendu[3],
            avant_rendu[4],
            avant_rendu[5],
            avant_rendu[6],
            avant_rendu[7],
            jetes[0],
            jetes[1],
            jetes[2],
            jetes[3],
            jetes[4],
            jetes[5],
            jetes[6],
            jetes[7],
            debordements[0],
            debordements[1],
            debordements[2],
            debordements[3],
            debordements[4],
            debordements[5],
            debordements[6],
            debordements[7],
        ]
    }

    /// **Le** parseur des compteurs : des octets hostiles vers un instantané valide, ou une
    /// cause de refus.
    ///
    /// Pure, sans allocation, sans panique, sans appel noyau — appelable à n'importe quel
    /// IRQL et fuzzable en mode utilisateur, comme [`CableState::from_bytes`].
    ///
    /// La longueur est vérifiée **avant** tout le reste et exigée **exacte** : ni plus
    /// courte, ni plus longue, ni un préfixe valide suivi d'octets en trop.
    ///
    /// # Ce qui n'est pas validé, et pourquoi
    ///
    /// Les six compteurs eux-mêmes : **tout `u64` est une valeur légitime**. Inventer une
    /// borne (« pas plus de N ticks ») ferait refuser un pilote qui tourne depuis longtemps,
    /// et une relation entre compteurs (« `copied` ≤ `ticks` × avance ») serait fausse par
    /// construction, l'instantané n'étant pas pris d'un seul coup (voir la note de
    /// structure). Seuls l'écho de câble et le champ réservé ont un domaine.
    ///
    /// # Erreurs
    ///
    /// [`CountersError`], qui nomme le **premier** champ fautif dans l'ordre de la
    /// structure.
    pub fn from_bytes(data: &[u8]) -> Result<Self, CountersError> {
        if data.len() != CABLE_COUNTERS_BYTES {
            return Err(CountersError::Longueur { recus: data.len() });
        }
        // Les huit lectures ne peuvent plus échouer (la longueur est exacte) ; le `ok_or`
        // remplace un `unwrap` interdit par les lints du crate.
        let longueur = || CountersError::Longueur { recus: data.len() };
        let cable = mot(data, OC_CABLE).ok_or_else(longueur)?;
        let reserved = mot(data, OC_RESERVED).ok_or_else(longueur)?;
        let ticks = mot_long(data, OC_TICKS).ok_or_else(longueur)?;
        let copied = mot_long(data, OC_COPIED).ok_or_else(longueur)?;
        let silenced_no_render = mot_long(data, OC_SILENCED_NO_RENDER).ok_or_else(longueur)?;
        let silenced_before_render =
            mot_long(data, OC_SILENCED_BEFORE_RENDER).ok_or_else(longueur)?;
        let discarded_ticks = mot_long(data, OC_DISCARDED_TICKS).ok_or_else(longueur)?;
        let overruns = mot_long(data, OC_OVERRUNS).ok_or_else(longueur)?;

        if cable >= CABLE_MAX {
            return Err(CountersError::Cable(cable));
        }
        if reserved != 0 {
            return Err(CountersError::Reserved(reserved));
        }
        Ok(Self {
            cable,
            reserved,
            ticks,
            copied,
            silenced_no_render,
            silenced_before_render,
            discarded_ticks,
            overruns,
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

// ---------------------------------------------------------------------------------
// `CableFormat<n>` : le format d'un câble, un `REG_DWORD` par câble (M1b-05).
// ---------------------------------------------------------------------------------

/// Noms des valeurs `REG_DWORD` qui portent le format de chaque câble, dans la clé
/// **matérielle** du périphérique (`HKR`, comme [`ACTIVE_CABLES_VALUE_NAME`] et pour la
/// même raison : PnP les supprime avec le périphérique, F-52).
///
/// Écrites en toutes lettres plutôt qu'engendrées par concaténation : le crate est
/// `no_std` et sans allocation, un nom se compose donc à la compilation ou pas du tout —
/// et ces seize chaînes sont ce que l'INF écrit, ce que le pilote lit, ce que le service
/// réécrit et ce qu'un administrateur voit dans `regedit`. Une seule liste, quatre
/// lecteurs.
pub const CABLE_FORMAT_VALUE_NAMES: [&str; CABLE_MAX as usize] = [
    "CableFormat0",
    "CableFormat1",
    "CableFormat2",
    "CableFormat3",
    "CableFormat4",
    "CableFormat5",
    "CableFormat6",
    "CableFormat7",
    "CableFormat8",
    "CableFormat9",
    "CableFormat10",
    "CableFormat11",
    "CableFormat12",
    "CableFormat13",
    "CableFormat14",
    "CableFormat15",
];

/// Nom lisible du format, en français, pour le journal.
pub const CABLE_FORMAT_LABEL: &str = "format du câble";

/// Le nom de valeur du câble `cable`, ou `None` au-delà du dernier.
#[must_use]
pub fn cable_format_value_name(cable: u32) -> Option<&'static str> {
    usize::try_from(cable)
        .ok()
        .and_then(|i| CABLE_FORMAT_VALUE_NAMES.get(i))
        .copied()
}

/// Code de fréquence dans l'octet de poids faible : 44 100 Hz.
pub const CODE_RATE_44100: u32 = 1;
/// Code de fréquence : 48 000 Hz.
pub const CODE_RATE_48000: u32 = 2;
/// Code de fréquence : 96 000 Hz.
pub const CODE_RATE_96000: u32 = 3;

/// Code de profondeur dans le deuxième octet : PCM 16 bits.
pub const CODE_DEPTH_PCM16: u32 = 1;
/// Code de profondeur : PCM 24 bits (conteneur de trois octets).
pub const CODE_DEPTH_PCM24: u32 = 2;
/// Code de profondeur : flottant 32 bits.
pub const CODE_DEPTH_F32: u32 = 3;

/// Décalage du champ « fréquence » dans le `REG_DWORD`.
const DECALAGE_RATE: u32 = 0;
/// Décalage du champ « profondeur préférée ».
const DECALAGE_DEPTH: u32 = 8;
/// Décalage du champ « canaux ».
const DECALAGE_CHANNELS: u32 = 16;
/// Décalage du champ réservé.
const DECALAGE_RESERVE: u32 = 24;
/// Masque d'un champ : un octet.
const MASQUE_CHAMP: u32 = 0xFF;

/// Le format d'un câble : la fréquence et le nombre de canaux qu'il **fige**, plus la
/// profondeur qu'il **préfère**.
///
/// # Pourquoi trois champs alors que deux seulement figent quelque chose
///
/// La fréquence et les canaux sont figés parce que [`crate::ring::copy_frames`] ne sait ni
/// rééchantillonner ni remapper les canaux : les deux bouts du câble doivent s'accorder,
/// et le pilote ne déclare donc que ceux-là ([`crate::format::cable_formats`]). La
/// profondeur, elle, se convertit à la volée dans les neuf sens — les trois sont donc
/// **toujours** déclarées, quelle que soit la valeur de ce champ.
///
/// [`Self::depth`] n'est pas un réglage du pilote mais une **préférence** que la
/// configuration transporte pour ses lecteurs d'espace utilisateur : le dorsal WASAPI
/// ouvre ses flux dans ce format, `conduitctl` l'affiche. Le pilote la lit, la valide et
/// la journalise ; il n'en tire aucun descripteur, et c'est précisément ce qui garde le
/// nombre de variantes à 3 × 8 = **24** par sens au lieu de 72.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CableFormat {
    /// Fréquence d'échantillonnage en Hz : une des trois de [`crate::format::SAMPLE_RATES`].
    pub sample_rate: u32,
    /// Profondeur préférée (voir la note de structure : préférence, pas restriction).
    pub depth: SampleFormat,
    /// Nombre de canaux, dans `MIN_CHANNELS..=MAX_CHANNELS`.
    pub channels: u8,
}

/// Le format d'un câble neuf : **48 kHz, float32, 2 canaux**, soit `0x0002_0302`.
///
/// 48 kHz est la fréquence par défaut de Windows ; float32 est ce que le moteur audio
/// emploie en mode partagé ; deux canaux, c'est ce que M1a servait et ce qu'un câble
/// virtuel sert le plus souvent. C'est aussi la valeur que l'INF écrit pour les seize
/// câbles à l'installation (`[ConduitCable_HW_AddReg]`, engendré par
/// `portcls/tests/inf.rs`) et celle sur laquelle la lecture se replie.
pub const CABLE_FORMAT_DEFAULT: CableFormat = CableFormat {
    sample_rate: RATE_48000,
    depth: SampleFormat::F32,
    channels: DEFAULT_CHANNELS as u8,
};

/// Ce qui a fait refuser un encodage de [`CableFormat::decode`] : un champ, une valeur.
///
/// Chaque variante porte le code trouvé, pas seulement le fait qu'on a refusé — même
/// principe que [`ConfigError`] et [`crate::params::Correction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatCodeError {
    /// Code de fréquence hors de `{1, 2, 3}`.
    Rate(u32),
    /// Code de profondeur hors de `{1, 2, 3}`.
    Depth(u32),
    /// Nombre de canaux hors de `MIN_CHANNELS..=MAX_CHANNELS`.
    Channels(u32),
    /// Octet de poids fort non nul.
    Reserve(u32),
}

impl fmt::Display for FormatCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rate(code) => write!(
                f,
                "code de fréquence {code} inconnu ({CODE_RATE_44100} = 44 100 Hz, \
                 {CODE_RATE_48000} = 48 000 Hz, {CODE_RATE_96000} = 96 000 Hz)"
            ),
            Self::Depth(code) => write!(
                f,
                "code de profondeur {code} inconnu ({CODE_DEPTH_PCM16} = PCM 16 bits, \
                 {CODE_DEPTH_PCM24} = PCM 24 bits, {CODE_DEPTH_F32} = float 32 bits)"
            ),
            Self::Channels(brut) => {
                write!(f, "{brut} canaux hors de {MIN_CHANNELS} à {MAX_CHANNELS}")
            }
            Self::Reserve(brut) => write!(f, "octet de poids fort non nul ({brut:#04x})"),
        }
    }
}

/// Ce que [`CableFormat::sanitize`] a corrigé : la valeur lue, celle retenue, et la
/// cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CableFormatFix {
    /// Numéro du câble concerné, pour que la ligne de journal se suffise à elle-même.
    pub cable: u32,
    /// Encodage trouvé dans le registre, tel quel.
    pub found: u32,
    /// Encodage retenu à la place.
    pub applied: u32,
    /// Le champ fautif et sa valeur.
    pub cause: FormatCodeError,
}

impl fmt::Display for CableFormatFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{CABLE_FORMAT_LABEL} {} = {:#010x} : {}, repli sur {:#010x}",
            self.cable, self.found, self.cause, self.applied
        )
    }
}

impl CableFormat {
    /// L'encodage `REG_DWORD` de ce format.
    ///
    /// Un octet par champ, du poids faible au poids fort : fréquence, profondeur, canaux,
    /// réservé nul. Le défaut se lit `0x0002_0302` dans `regedit` — **et c'est le point**.
    /// Un encodage compact (trois bits de fréquence, deux de profondeur…) tiendrait dans un
    /// octet et serait illisible ; un administrateur qui ouvre la clé doit pouvoir dire ce
    /// que la valeur signifie, et un journal qui l'affiche en hexadécimal doit être
    /// utilisable. Vingt-quatre bits perdus dans un `REG_DWORD` ne coûtent rien.
    #[must_use]
    pub const fn encode(self) -> u32 {
        let rate = match sample_rate_index(self.sample_rate) {
            Some(0) => CODE_RATE_44100,
            Some(1) => CODE_RATE_48000,
            Some(2) => CODE_RATE_96000,
            // Inatteignable pour une valeur construite par `decode` ou `sanitize` ; le
            // repli sur le défaut vaut mieux qu'un encodage que personne ne saura relire.
            _ => CODE_RATE_48000,
        };
        let depth = match self.depth {
            SampleFormat::I16 => CODE_DEPTH_PCM16,
            SampleFormat::Pcm24 => CODE_DEPTH_PCM24,
            SampleFormat::F32 => CODE_DEPTH_F32,
        };
        let channels = self.channels as u32;
        // Chaque champ tient dans son octet (les codes vont de 1 à 3, les canaux de 1 à
        // 8) : les décalages ne peuvent pas déborder, et `wrapping_shl` remplace un
        // opérateur que les lints du crate refusent.
        (rate & MASQUE_CHAMP).wrapping_shl(DECALAGE_RATE)
            | (depth & MASQUE_CHAMP).wrapping_shl(DECALAGE_DEPTH)
            | (channels & MASQUE_CHAMP).wrapping_shl(DECALAGE_CHANNELS)
    }

    /// **Le** parseur : un `REG_DWORD` vers un format valide, ou le champ fautif.
    ///
    /// Pur, sans allocation, sans panique, appelable à n'importe quel IRQL et fuzzable en
    /// mode utilisateur (M1b-08), comme [`CableState::from_bytes`]. L'ordre des contrôles
    /// suit celui des octets, pour que la cause nommée soit la **première** anomalie.
    ///
    /// L'octet de poids fort est exigé nul, et pour la raison qui fait exiger nul le
    /// [`CableState::reserved`] : c'est ce qui garde la place libre. Un client qui y
    /// écrirait n'importe quoi aujourd'hui rendrait impossible d'y loger un vrai champ
    /// demain sans le casser.
    pub const fn decode(raw: u32) -> Result<Self, FormatCodeError> {
        let rate_code = raw.wrapping_shr(DECALAGE_RATE) & MASQUE_CHAMP;
        let depth_code = raw.wrapping_shr(DECALAGE_DEPTH) & MASQUE_CHAMP;
        let channels = raw.wrapping_shr(DECALAGE_CHANNELS) & MASQUE_CHAMP;
        let reserve = raw.wrapping_shr(DECALAGE_RESERVE) & MASQUE_CHAMP;

        let sample_rate = match rate_code {
            CODE_RATE_44100 => RATE_44100,
            CODE_RATE_48000 => RATE_48000,
            CODE_RATE_96000 => RATE_96000,
            autre => return Err(FormatCodeError::Rate(autre)),
        };
        let depth = match depth_code {
            CODE_DEPTH_PCM16 => SampleFormat::I16,
            CODE_DEPTH_PCM24 => SampleFormat::Pcm24,
            CODE_DEPTH_F32 => SampleFormat::F32,
            autre => return Err(FormatCodeError::Depth(autre)),
        };
        if channels < MIN_CHANNELS || channels > MAX_CHANNELS {
            return Err(FormatCodeError::Channels(channels));
        }
        if reserve != 0 {
            return Err(FormatCodeError::Reserve(reserve));
        }
        Ok(Self {
            sample_rate,
            depth,
            channels: channels as u8,
        })
    }

    /// Lit un encodage venu du registre : **jamais d'échec**.
    ///
    /// Comme [`crate::params::sanitize`] et [`sanitize_mask`], et pour la même raison —
    /// refuser de charger pour une faute de frappe dans un `REG_DWORD` coûterait à
    /// l'administrateur toutes les cartes son virtuelles du poste. Un encodage aberrant
    /// donne [`CABLE_FORMAT_DEFAULT`] **en entier** et une ligne de journal qui nomme le
    /// champ fautif.
    ///
    /// Le repli est global et non champ par champ, contrairement à
    /// [`crate::params::sanitize`] qui écrête chaque paramètre séparément : les trois
    /// champs décrivent **un** format, et rendre « 96 kHz sur les canaux par défaut » pour
    /// une valeur dont seul l'octet des canaux est faux fabriquerait une configuration que
    /// personne n'a demandée. Reprendre le défaut entier est le seul repli qui reste
    /// explicable dans le journal.
    #[must_use]
    pub const fn sanitize(cable: u32, raw: u32) -> (Self, Option<CableFormatFix>) {
        match Self::decode(raw) {
            Ok(format) => (format, None),
            Err(cause) => (
                CABLE_FORMAT_DEFAULT,
                Some(CableFormatFix {
                    cable,
                    found: raw,
                    applied: CABLE_FORMAT_DEFAULT.encode(),
                    cause,
                }),
            ),
        }
    }

    /// Le rang de la fréquence dans [`crate::format::SAMPLE_RATES`], donc la première
    /// moitié de l'index de variante des descripteurs du pilote.
    #[must_use]
    pub const fn rate_index(self) -> Option<usize> {
        sample_rate_index(self.sample_rate)
    }

    /// L'index de **variante de descripteurs** de ce format, ou `None` s'il n'en a pas.
    ///
    /// C'est la seule traduction « format d'un câble → rangée des tables KS » du dépôt,
    /// et elle est ici plutôt que dans le pilote pour être testable en mode utilisateur
    /// (voir la matrice de [`crate::format`]). `None` est **impossible** pour un format
    /// sorti de [`Self::sanitize`] ou de [`Self::decode`] — les deux bornent la fréquence
    /// aux trois et les canaux à `MIN_CHANNELS..=MAX_CHANNELS` —, ce que l'assertion
    /// `const` ci-dessous dit du défaut et ce que les tests disent de tout le domaine.
    /// L'appelant qui reçoit quand même `None` tient la preuve d'une divergence entre ce
    /// magasin et la matrice, et le pilote la consigne au journal d'événements.
    #[must_use]
    pub const fn variant(self) -> Option<usize> {
        crate::format::variant_of(self.sample_rate, self.channels)
    }
}

// L'encodage du défaut est bien celui qu'on annonce partout — dans l'INF, dans la
// documentation, dans le journal. Une divergence donnerait un INF qui écrit une valeur et
// un pilote qui en attend une autre : tous les câbles se replieraient, avec seize lignes
// de journal, sur un format qui se trouverait être le bon.
const _: () = assert!(CABLE_FORMAT_DEFAULT.encode() == 0x0002_0302);
// Le défaut a bien une variante, et c'est celle qu'on croit : 48 kHz (rang 1) sur 2 canaux,
// donc 1 × 8 + 1 = 9. C'est la rangée sur laquelle tout se replie ; qu'elle existe est ce
// qui rend le repli du pilote sûr.
const _: () = assert!(matches!(CABLE_FORMAT_DEFAULT.variant(), Some(9)));
const _: () = assert!(matches!(
    CableFormat::decode(0x0002_0302),
    Ok(f) if f.sample_rate == RATE_48000 && f.channels == 2
));
// Aller-retour sur les bornes du domaine : le codec ne perd rien.
const _: () = assert!(matches!(
    CableFormat::decode(CABLE_FORMAT_DEFAULT.encode()),
    Ok(CABLE_FORMAT_DEFAULT)
));
// Le nombre de noms de valeur couvre exactement les câbles adressables.
const _: () = assert!(CABLE_FORMAT_VALUE_NAMES.len() == CABLE_MAX as usize);

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
    use crate::format::{cable_formats, RATE_44100, RATE_96000};
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

    /// Les trois identifiants de propriété sont distincts et stables.
    #[test]
    fn les_identifiants_de_propriete_sont_distincts() {
        assert_eq!(KSPROPERTY_CONDUIT_CABLE_STATE, 0);
        assert_eq!(KSPROPERTY_CONDUIT_VERSION, 1);
        assert_eq!(KSPROPERTY_CONDUIT_COUNTERS, 2);
        let ids = [
            KSPROPERTY_CONDUIT_CABLE_STATE,
            KSPROPERTY_CONDUIT_VERSION,
            KSPROPERTY_CONDUIT_COUNTERS,
        ];
        for (i, gauche) in ids.iter().enumerate() {
            for droite in ids.iter().skip(i + 1) {
                assert_ne!(
                    gauche, droite,
                    "deux propriétés du même jeu ne peuvent pas partager un identifiant : \
                     PortCls cherche la première entrée de même Set/Id et servirait la mauvaise"
                );
            }
        }
        // M1b-21 : une propriété qui apparaît est un changement observable du jeu, et ce
        // numéro en est la seule voie (voir sa documentation).
        assert_eq!(CONFIG_VERSION, 3);
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

    /// La disposition des compteurs : 56 octets, deux `ULONG` puis six `ULONGLONG`, aucun
    /// trou — et chaque compteur se relit **à son décalage nommé**.
    ///
    /// Les six valeurs sont volontairement toutes différentes : c'est la seule façon
    /// d'attraper deux champs intervertis, qu'un aller-retour sur des zéros laisserait
    /// passer.
    #[test]
    fn la_disposition_des_compteurs_est_celle_des_decalages_nommes() {
        assert_eq!(CABLE_COUNTERS_BYTES, 56);
        assert_eq!(size_of::<CableCounters>(), CABLE_COUNTERS_BYTES);
        assert_eq!(align_of::<CableCounters>(), 8);

        // Les deux `ULONG` de tête pavent les huit premiers octets…
        assert_eq!(OC_CABLE, 0);
        assert_eq!(OC_RESERVED, TAILLE_MOT);
        assert_eq!(OC_TICKS, TAILLE_MOT * 2);
        // …puis six `ULONGLONG` contigus, sans trou ni recouvrement.
        let longs = [
            OC_TICKS,
            OC_COPIED,
            OC_SILENCED_NO_RENDER,
            OC_SILENCED_BEFORE_RENDER,
            OC_DISCARDED_TICKS,
            OC_OVERRUNS,
        ];
        for (i, decalage) in longs.iter().enumerate() {
            assert_eq!(*decalage, OC_TICKS + i * TAILLE_MOT_LONG, "compteur n° {i}");
            assert_eq!(*decalage % TAILLE_MOT_LONG, 0, "compteur n° {i} désaligné");
        }
        assert_eq!(
            OC_OVERRUNS + TAILLE_MOT_LONG,
            CABLE_COUNTERS_BYTES,
            "la structure est exactement deux mots puis six mots longs"
        );

        let compteurs = CableCounters {
            cable: 3,
            reserved: 0,
            ticks: 11,
            copied: 22,
            silenced_no_render: 33,
            silenced_before_render: 44,
            discarded_ticks: 55,
            overruns: 66,
        };
        let brut = compteurs.to_bytes();
        assert_eq!(mot(&brut, OC_CABLE), Some(3));
        assert_eq!(mot(&brut, OC_RESERVED), Some(0));
        assert_eq!(mot_long(&brut, OC_TICKS), Some(11));
        assert_eq!(mot_long(&brut, OC_COPIED), Some(22));
        assert_eq!(mot_long(&brut, OC_SILENCED_NO_RENDER), Some(33));
        assert_eq!(mot_long(&brut, OC_SILENCED_BEFORE_RENDER), Some(44));
        assert_eq!(mot_long(&brut, OC_DISCARDED_TICKS), Some(55));
        assert_eq!(mot_long(&brut, OC_OVERRUNS), Some(66));
        assert_eq!(CableCounters::from_bytes(&brut), Ok(compteurs));
    }

    /// Les longueurs refusées, y compris le préfixe valide suivi d'un octet.
    #[test]
    fn seule_la_longueur_exacte_des_compteurs_est_acceptee() {
        let complet = CableCounters::new(0).to_bytes();
        assert!(CableCounters::from_bytes(&complet).is_ok());

        for taille in 0..CABLE_COUNTERS_BYTES {
            let court = complet.get(..taille).unwrap().to_vec();
            assert_eq!(
                CableCounters::from_bytes(&court),
                Err(CountersError::Longueur { recus: taille }),
                "taille {taille}"
            );
        }

        // Un préfixe **parfaitement valide** suivi d'un octet : le cas qu'un fuzzer trouve
        // en premier, et le seul que la longueur exacte attrape.
        let mut allonge = Vec::from(complet);
        allonge.push(0);
        assert_eq!(
            CableCounters::from_bytes(&allonge),
            Err(CountersError::Longueur { recus: 57 })
        );
    }

    /// Les deux échos ont un domaine, les six compteurs n'en ont pas — et le message de
    /// refus nomme la bonne longueur, pas celle de `CableState`.
    #[test]
    fn les_compteurs_refusent_l_echo_faux_et_le_reserve_non_nul() {
        let hors = CableCounters {
            cable: CABLE_MAX,
            ..CableCounters::new(0)
        };
        assert_eq!(
            CableCounters::from_bytes(&hors.to_bytes()),
            Err(CountersError::Cable(CABLE_MAX))
        );

        let sale = CableCounters {
            reserved: 1,
            ..CableCounters::new(0)
        };
        assert_eq!(
            CableCounters::from_bytes(&sale.to_bytes()),
            Err(CountersError::Reserved(1))
        );

        // `u64::MAX` partout : accepté, un compteur n'a pas de borne inventée.
        let plein = CableCounters {
            cable: CABLE_MAX - 1,
            reserved: 0,
            ticks: u64::MAX,
            copied: u64::MAX,
            silenced_no_render: u64::MAX,
            silenced_before_render: u64::MAX,
            discarded_ticks: u64::MAX,
            overruns: u64::MAX,
        };
        assert_eq!(CableCounters::from_bytes(&plein.to_bytes()), Ok(plein));

        // Le message dit **cinquante-six**, pas seize : un diagnostic qui annoncerait la
        // longueur de l'autre structure coûterait une heure.
        let message = CountersError::Longueur { recus: 16 }.to_string();
        assert!(message.contains("56"), "{message}");
        assert!(!message.contains("16 attendus"), "{message}");
    }

    /// Les deux lectures que M1b-07 demandait : « rendu seul » et « capture seule », et ce
    /// qui les distingue d'un câble qui transporte.
    #[test]
    fn les_deux_regimes_a_un_seul_cote_se_lisent_dans_les_compteurs() {
        // Rendu seul : des ticks jetés, et rien d'écrit d'aucune sorte.
        let rendu_seul = CableCounters {
            ticks: 100,
            discarded_ticks: 100,
            ..CableCounters::new(0)
        };
        assert!(rendu_seul.rendu_seul());
        assert!(!rendu_seul.capture_seule());

        // Capture seule : du silence sans rendu, et rien de copié.
        let capture_seule = CableCounters {
            ticks: 100,
            silenced_no_render: 4_800,
            ..CableCounters::new(0)
        };
        assert!(capture_seule.capture_seule());
        assert!(!capture_seule.rendu_seul());

        // Les deux côtés ouverts : ni l'un ni l'autre, quelques trames de silence au
        // départ du lien ne changent rien.
        let boucle = CableCounters {
            ticks: 100,
            copied: 480_000,
            silenced_before_render: 96,
            ..CableCounters::new(0)
        };
        assert!(!boucle.rendu_seul());
        assert!(!boucle.capture_seule());

        // Un câble au repos : aucun régime, et c'est bien ce qui le distinguait mal du
        // rendu seul avant que `discarded_ticks` existe.
        let repos = CableCounters {
            ticks: 100,
            ..CableCounters::new(0)
        };
        assert!(!repos.rendu_seul());
        assert!(!repos.capture_seule());
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

        // Le domaine est celui du contrat (1 à 8) ; le gestionnaire, lui, exige la valeur
        // **courante du câble** — non plus 2 en dur, depuis M1b-05.
        let huit = CableState::from_bytes(&octets(0, 1, MAX_CHANNELS, 0)).unwrap();
        assert!(huit.channels_appliquables(MAX_CHANNELS), "câble à 8 canaux");
        assert!(
            !huit.channels_appliquables(DEFAULT_CHANNELS),
            "un câble stéréo ne peut pas passer à 8 canaux sans redémarrer le devnode"
        );
        let deux = CableState::from_bytes(&octets(0, 1, DEFAULT_CHANNELS, 0)).unwrap();
        assert!(deux.channels_appliquables(DEFAULT_CHANNELS));
        assert!(!deux.channels_appliquables(MAX_CHANNELS));
    }

    /// **L'écho se reprend, il ne se fabrique pas.**
    ///
    /// Sur chaque format possible, l'état relu dont on ne change que la connexion reste
    /// applicable au câble ; l'état fabriqué par [`CableState::new`], lui, ne l'est que sur
    /// un câble stéréo. C'est le défaut de frontière de M1b-05 en une table de cas, et la
    /// raison d'être de [`CableState::avec_connexion`].
    #[test]
    fn brancher_le_jack_reprend_les_canaux_du_cable() {
        for servis in MIN_CHANNELS..=MAX_CHANNELS {
            // Ce qu'un `GET` rend sur un câble configuré pour `servis` canaux.
            let lu = CableState::from_bytes(&octets(3, 0, servis, 0)).unwrap();
            for connecte in [true, false] {
                let voulu = lu.avec_connexion(connecte);
                assert_eq!(voulu.channels, servis, "{servis} canaux");
                assert_eq!(voulu.cable, lu.cable, "{servis} canaux");
                assert_eq!(voulu.is_connected(), connecte, "{servis} canaux");
                assert_eq!(voulu.reserved, 0, "{servis} canaux");
                assert!(
                    voulu.channels_appliquables(servis),
                    "brancher le jack d'un câble à {servis} canaux doit être applicable"
                );
                // Et l'aller-retour du contrat l'accepte : c'est bien une requête émettable.
                assert_eq!(CableState::from_bytes(&voulu.to_bytes()), Ok(voulu));
            }

            // L'état **fabriqué** — celui que le service envoyait — n'est applicable que
            // sur un câble stéréo, et c'est le refus mesuré (erreur Win32 87).
            let fabrique = CableState::new(3, true);
            assert_eq!(
                fabrique.channels_appliquables(servis),
                servis == DEFAULT_CHANNELS,
                "un état fabriqué de toutes pièces sur un câble à {servis} canaux"
            );
        }
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

    /// Table du codec de format : les valeurs nominales, les bornes, et l'encodage lisible.
    #[test]
    fn cable_format_table() {
        let cases: [(u32, u32, SampleFormat, u8); 8] = [
            // Le défaut, tel qu'il se lit dans `regedit` : 48 kHz, float32, 2 canaux.
            (0x0002_0302, RATE_48000, SampleFormat::F32, 2),
            // Les deux autres fréquences, aux deux bornes des canaux.
            (0x0001_0101, RATE_44100, SampleFormat::I16, 1),
            (0x0008_0203, RATE_96000, SampleFormat::Pcm24, 8),
            (0x0006_0303, RATE_96000, SampleFormat::F32, 6),
            (0x0002_0201, RATE_44100, SampleFormat::Pcm24, 2),
            (0x0004_0102, RATE_48000, SampleFormat::I16, 4),
            (0x0005_0302, RATE_48000, SampleFormat::F32, 5),
            (0x0008_0101, RATE_44100, SampleFormat::I16, 8),
        ];
        for (raw, sample_rate, depth, channels) in cases {
            let attendu = CableFormat {
                sample_rate,
                depth,
                channels,
            };
            assert_eq!(CableFormat::decode(raw), Ok(attendu), "{raw:#010x}");
            assert_eq!(attendu.encode(), raw, "{attendu:?}");
            // Aucune correction pour une valeur du domaine.
            let (retenu, fix) = CableFormat::sanitize(0, raw);
            assert_eq!(retenu, attendu);
            assert!(fix.is_none(), "{raw:#010x} corrigé à tort");
        }
        assert_eq!(CABLE_FORMAT_DEFAULT.encode(), 0x0002_0302);
        assert_eq!(CableFormat::decode(0x0002_0302), Ok(CABLE_FORMAT_DEFAULT));
    }

    /// Table des encodages **invalides** : un champ fautif, une cause nommée, et jamais
    /// d'échec — c'est la règle de `params::sanitize`, appliquée au format.
    #[test]
    fn cable_format_invalide_table() {
        let cases: [(u32, FormatCodeError); 12] = [
            // Zéro : le cas d'une valeur créée sans contenu.
            (0x0000_0000, FormatCodeError::Rate(0)),
            // Code de fréquence hors des trois.
            (0x0002_0300, FormatCodeError::Rate(0)),
            (0x0002_0304, FormatCodeError::Rate(4)),
            (0x0002_03FF, FormatCodeError::Rate(255)),
            // Code de profondeur hors des trois — la fréquence, elle, est bonne.
            (0x0002_0002, FormatCodeError::Depth(0)),
            (0x0002_0402, FormatCodeError::Depth(4)),
            (0x0002_FF02, FormatCodeError::Depth(255)),
            // Canaux hors bornes.
            (0x0000_0302, FormatCodeError::Channels(0)),
            (0x0009_0302, FormatCodeError::Channels(9)),
            (0x00FF_0302, FormatCodeError::Channels(255)),
            // Octet de poids fort non nul : la place réservée reste libre.
            (0x0102_0302, FormatCodeError::Reserve(1)),
            (0xFF02_0302, FormatCodeError::Reserve(255)),
        ];
        for (raw, cause) in cases {
            assert_eq!(CableFormat::decode(raw), Err(cause), "{raw:#010x}");
            // Le repli est le défaut **entier**, jamais un mélange (voir `sanitize`).
            let (retenu, fix) = CableFormat::sanitize(7, raw);
            assert_eq!(retenu, CABLE_FORMAT_DEFAULT, "{raw:#010x}");
            let fix = fix.expect("un encodage refusé doit être signalé");
            assert_eq!(fix.cable, 7);
            assert_eq!(fix.found, raw);
            assert_eq!(fix.applied, CABLE_FORMAT_DEFAULT.encode());
            assert_eq!(fix.cause, cause);
            // La ligne de journal nomme le câble, la valeur lue et le champ fautif.
            let ligne = fix.to_string();
            assert!(ligne.contains("câble"), "{ligne}");
            assert!(ligne.contains("0x"), "{ligne}");
        }
        // L'ordre des contrôles suit celui des octets : les quatre champs fautifs à la
        // fois, c'est la fréquence qui est signalée.
        assert_eq!(
            CableFormat::decode(0xFFFF_FFFF),
            Err(FormatCodeError::Rate(255))
        );
    }

    /// Le message du codec dit quel champ et quelle valeur, comme celui de [`ConfigError`].
    #[test]
    fn le_message_du_codec_nomme_le_champ() {
        let ligne = FormatCodeError::Rate(9).to_string();
        assert!(ligne.contains('9'), "{ligne}");
        assert!(ligne.contains("44 100"), "{ligne}");
        let ligne = FormatCodeError::Depth(9).to_string();
        assert!(ligne.contains("profondeur"), "{ligne}");
        assert!(ligne.contains("24 bits"), "{ligne}");
        let ligne = FormatCodeError::Channels(9).to_string();
        assert!(ligne.contains("canaux"), "{ligne}");
        let ligne = FormatCodeError::Reserve(0x42).to_string();
        assert!(ligne.contains("poids fort"), "{ligne}");
        assert!(ligne.contains("0x42"), "{ligne}");
    }

    /// Les seize noms de valeur : un par câble, distincts, et de la forme que l'INF écrit.
    #[test]
    fn les_noms_de_valeur_couvrent_les_seize_cables() {
        assert_eq!(CABLE_FORMAT_VALUE_NAMES.len(), CABLE_MAX as usize);
        for (i, nom) in CABLE_FORMAT_VALUE_NAMES.iter().enumerate() {
            assert_eq!(*nom, std::format!("CableFormat{i}"), "câble {i}");
            assert_eq!(cable_format_value_name(i as u32), Some(*nom));
        }
        // Deux câbles ne peuvent pas partager un nom : ils partageraient un format.
        let mut vus: Vec<&str> = CABLE_FORMAT_VALUE_NAMES.to_vec();
        vus.sort_unstable();
        vus.dedup();
        assert_eq!(vus.len(), CABLE_MAX as usize);
        assert_eq!(cable_format_value_name(CABLE_MAX), None);
        assert_eq!(cable_format_value_name(u32::MAX), None);
        // Le format ne se confond pas avec l'état actif : deux natures, deux valeurs.
        assert!(!CABLE_FORMAT_VALUE_NAMES.contains(&ACTIVE_CABLES_VALUE_NAME));
    }

    /// Le format lu du registre alimente bien la liste de formats du pilote.
    #[test]
    fn le_format_lu_alimente_les_formats_declares() {
        let (format, fix) = CableFormat::sanitize(3, 0x0006_0203);
        assert!(fix.is_none());
        assert_eq!(format.sample_rate, RATE_96000);
        assert_eq!(format.channels, 6);
        assert_eq!(format.depth, SampleFormat::Pcm24);
        assert_eq!(format.rate_index(), Some(2));
        // Les trois profondeurs sont déclarées quoi qu'il arrive : `depth` n'en restreint
        // aucune, c'est une préférence pour l'espace utilisateur.
        let declares = cable_formats(format.sample_rate, format.channels);
        assert_eq!(declares.len(), 3);
        for f in declares {
            assert_eq!(f.sample_rate, RATE_96000);
            assert_eq!(f.channels, 6);
        }
    }

    proptest! {
        /// Le critère de M1b-08 sur le codec : **quelle que soit l'entrée**, il ne panique
        /// pas, tout ce qu'il accepte se réencode à l'identique, et tout ce qu'il refuse
        /// donne le défaut entier.
        #[test]
        fn le_codec_ne_panique_pas_et_l_aller_retour_est_fidele(raw in any::<u32>()) {
            match CableFormat::decode(raw) {
                Ok(format) => {
                    prop_assert_eq!(format.encode(), raw);
                    prop_assert_eq!(CableFormat::decode(format.encode()), Ok(format));
                    // Toute sortie acceptée est dans les domaines annoncés.
                    prop_assert!(crate::format::SAMPLE_RATES.contains(&format.sample_rate));
                    prop_assert!(crate::format::SAMPLE_DEPTHS.contains(&format.depth));
                    let ch = u32::from(format.channels);
                    prop_assert!((MIN_CHANNELS..=MAX_CHANNELS).contains(&ch));
                    prop_assert_eq!(CableFormat::sanitize(0, raw), (format, None));
                }
                Err(_) => {
                    let (retenu, fix) = CableFormat::sanitize(0, raw);
                    prop_assert_eq!(retenu, CABLE_FORMAT_DEFAULT);
                    prop_assert!(fix.is_some());
                }
            }
        }

        /// Sur tout le domaine des trois champs : l'aller-retour est l'identité, et tout
        /// format construit décrit une trame que `FrameLayout` sait former.
        #[test]
        fn tout_format_du_domaine_se_code_et_se_relit(
            rate_index in 0usize..3,
            depth_index in 0usize..3,
            channels in 1u8..=8,
        ) {
            let sample_rate = crate::format::sample_rate_at(rate_index).unwrap();
            let depth = crate::format::SAMPLE_DEPTHS[depth_index];
            let format = CableFormat { sample_rate, depth, channels };
            prop_assert_eq!(CableFormat::decode(format.encode()), Ok(format));
            prop_assert_eq!(format.rate_index(), Some(rate_index));
            // L'octet réservé reste nul, et chaque champ tient dans le sien.
            let raw = format.encode();
            prop_assert_eq!(raw >> 24, 0);
            prop_assert_eq!((raw >> 16) & 0xFF, u32::from(channels));
            prop_assert!(matches!((raw >> 8) & 0xFF, 1..=3));
            prop_assert!(matches!(raw & 0xFF, 1..=3));
            for f in cable_formats(sample_rate, channels) {
                prop_assert!(f.layout().is_some());
            }
        }

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

        /// Le même critère sur les compteurs : **quelle que soit l'entrée**, le parseur ne
        /// panique pas, et toute sortie acceptée se resérialise à l'identique.
        ///
        /// L'aller-retour est ce qui attrape un `ULONGLONG` lu au mauvais décalage — deux
        /// compteurs intervertis passeraient toutes les tables de cas, où ils valent
        /// souvent zéro.
        #[test]
        fn les_compteurs_ne_paniquent_pas_et_l_aller_retour_est_fidele(
            octets in proptest::collection::vec(any::<u8>(), 0..128),
        ) {
            match CableCounters::from_bytes(&octets) {
                Ok(compteurs) => {
                    prop_assert_eq!(octets.len(), CABLE_COUNTERS_BYTES);
                    prop_assert_eq!(&compteurs.to_bytes()[..], &octets[..]);
                    prop_assert_eq!(
                        CableCounters::from_bytes(&compteurs.to_bytes()),
                        Ok(compteurs)
                    );
                    prop_assert!(compteurs.cable < CABLE_MAX);
                    prop_assert_eq!(compteurs.reserved, 0);
                }
                Err(err) => {
                    // Une erreur de longueur exactement quand la longueur est fausse : les
                    // deux autres causes ne peuvent tomber que sur cinquante-six octets.
                    let longueur = matches!(err, CountersError::Longueur { .. });
                    prop_assert_eq!(longueur, octets.len() != CABLE_COUNTERS_BYTES);
                }
            }
        }

        /// Sur tout le domaine : les compteurs eux-mêmes n'ont **aucune** borne, seuls
        /// l'écho de câble et le champ réservé en ont une.
        #[test]
        fn les_compteurs_acceptent_toute_valeur_et_refusent_les_deux_echos(
            cable in any::<u32>(),
            reserved in any::<u32>(),
            ticks in any::<u64>(),
            copied in any::<u64>(),
            sans_rendu in any::<u64>(),
            avant_rendu in any::<u64>(),
            jetes in any::<u64>(),
            debordements in any::<u64>(),
        ) {
            let compteurs = CableCounters {
                cable,
                reserved,
                ticks,
                copied,
                silenced_no_render: sans_rendu,
                silenced_before_render: avant_rendu,
                discarded_ticks: jetes,
                overruns: debordements,
            };
            let valide = cable < CABLE_MAX && reserved == 0;
            prop_assert_eq!(
                CableCounters::from_bytes(&compteurs.to_bytes()).is_ok(),
                valide
            );
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
