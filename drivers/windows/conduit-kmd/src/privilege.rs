//! Contrôle d'accès aux écritures de configuration (M1b-04, driver-design.md §6) :
//! `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`.
//!
//! Le jeu de propriétés privé `KSPROPSETID_Conduit` est atteignable par **n'importe quel
//! processus** : le descripteur de sécurité que l'INF pose sur l'objet de périphérique
//! (`[ConduitCable_HW_AddReg]`, `…(A;;GRGWGX;;;WD)`) accorde lecture, écriture et exécution
//! à « Tout le monde », comme tout adaptateur audio — il le faut, `audiodg.exe` tournant
//! en `LOCAL SERVICE`. La lecture de l'état d'un câble est donc libre, et c'est voulu (un
//! outil de diagnostic non élevé doit pouvoir interroger). L'**écriture**, elle, doit être
//! réservée : sans quoi n'importe quelle application déconnecterait les câbles de
//! l'utilisateur.
//!
//! # Pourquoi `SE_LOAD_DRIVER_PRIVILEGE` et pas autre chose
//!
//! Le service d'assistance (M1b-20) tournera en `LocalSystem`, qui le détient. C'est aussi
//! le privilège que possède exactement la population qui peut déjà **installer et
//! désinstaller ce pilote** : quiconque l'a peut charger n'importe quel pilote noyau, donc
//! faire infiniment pire que débrancher un câble virtuel. Exiger un privilège plus rare
//! serait une fausse précision ; en exiger un plus commun (`SE_SHUTDOWN_PRIVILEGE`, par
//! exemple) laisserait la porte à un utilisateur interactif ordinaire.
//!
//! # La réserve, et pourquoi elle est écrite ici plutôt que tue
//!
//! `SeSinglePrivilegeCheck` interroge le jeton du **fil courant**, avec le mode d'appel
//! qu'on lui donne. Deux façons de se tromper, et elles rendraient le contrôle *pire
//! qu'absent* — présent, silencieux, et autorisant tout le monde :
//!
//! - si le gestionnaire de propriété ne s'exécutait **pas** dans le contexte du fil
//!   appelant mais sur un fil système, [`ExGetPreviousMode`] rendrait `KernelMode`, et la
//!   routine rendrait vrai inconditionnellement (le noyau n'a pas de privilège à
//!   prouver) ;
//! - même en forçant `UserMode`, le jeton interrogé serait celui du fil système, c'est-à-
//!   dire le jeton `System`, qui détient `SeLoadDriverPrivilege`. Forcer le mode ne
//!   répare donc rien.
//!
//! **Ce que la documentation dit, vérifié plutôt que supposé** : elle ne dit **rien** du
//! contexte de fil des gestionnaires PortCls. `portcls.h` ne documente que la propriété du
//! `PCPROPERTY_REQUEST` et la possibilité de rendre `STATUS_PENDING` ; la page
//! `PCPROPERTY_ITEM` a un champ IRQL **vide** ; et il n'existe pas de page dédiée à
//! `PCPFNPROPERTY_HANDLER`. Le raisonnement qui rend le contrôle crédible — un
//! `IOCTL_KS_PROPERTY` est une `IRP_MJ_DEVICE_CONTROL` que la routine de répartition traite
//! en ligne, donc dans le fil qui a appelé `DeviceIoControl` — est solide et c'est celui
//! que retient driver-design.md §6, mais il n'est **pas** documenté, et rien n'interdit à
//! PortCls de différer une requête.
//!
//! Le contrôle reste donc en place, et la vérification en machine virtuelle est **à
//! faire** : un `SET` depuis un processus non élevé doit rendre
//! `STATUS_PRIVILEGE_NOT_HELD`, et la trace de `portcls::config` le montre en une ligne. Si
//! ce n'était pas le cas, le repli est connu : passer par `PCPROPERTY_REQUEST::Irp`
//! (`RequestorMode` pour le mode, `Tail.Overlay.Thread` puis `PsReferencePrimaryToken` et
//! `SePrivilegeCheck` pour le jeton), ce qui coûte nettement plus d'`unsafe` et ne se
//! justifie que si la mesure l'exige.
//!
//! IRQL : `PASSIVE_LEVEL`.

use wdk_sys::ntddk::{ExGetPreviousMode, SeSinglePrivilegeCheck};
use wdk_sys::{LUID, SE_LOAD_DRIVER_PRIVILEGE};

/// `SE_LOAD_DRIVER_PRIVILEGE` (`ntifs.h`) en `LUID`, ce que `SeSinglePrivilegeCheck`
/// attend.
///
/// Les privilèges bien connus sont des LUID de partie haute nulle : c'est ce que fait
/// `RtlConvertUlongToLuid`, la macro que le WDK emploie partout et que les bindings ne
/// portent pas (elle est `inline`). La valeur elle-même vient des bindings, pas d'un 10
/// recopié.
const SE_LOAD_DRIVER: LUID = LUID {
    LowPart: SE_LOAD_DRIVER_PRIVILEGE,
    HighPart: 0,
};

// Les LUID de privilège bien connus vont de `SE_MIN_WELL_KNOWN_PRIVILEGE` vers le haut ;
// une partie haute non nulle serait une conversion ratée, et le contrôle refuserait alors
// **tout le monde** — panne symétrique de celle que la réserve ci-dessus décrit, et tout
// aussi silencieuse.
const _: () = assert!(SE_LOAD_DRIVER.HighPart == 0);
const _: () = assert!(SE_LOAD_DRIVER.LowPart == 10, "SE_LOAD_DRIVER_PRIVILEGE");

/// L'appelant courant détient-il `SeLoadDriverPrivilege`, **activé** ?
///
/// Rend vrai pour un appelant en mode noyau : c'est la sémantique de
/// `SeSinglePrivilegeCheck`, et elle est correcte pour un appel qui vient réellement du
/// noyau. Lire la réserve en tête de module avant de s'y fier pour un appel qui vient
/// d'ailleurs.
///
/// IRQL : `PASSIVE_LEVEL`.
pub(crate) fn may_load_driver() -> bool {
    // SAFETY: `ExGetPreviousMode` ne prend aucun paramètre et rend le mode d'accès de
    // l'appelant du fil courant ; elle est appelable à `PASSIVE_LEVEL`, ce qu'est le
    // contexte d'un gestionnaire de propriété PortCls.
    let mode = unsafe { ExGetPreviousMode() };
    // SAFETY: `SE_LOAD_DRIVER` est un `LUID` par valeur, `mode` un `KPROCESSOR_MODE` que
    // le noyau vient de rendre ; la routine ne déréférence rien et s'appelle à
    // `PASSIVE_LEVEL`. Elle interroge le jeton du fil courant — voir la réserve en tête de
    // module.
    let accorde = unsafe { SeSinglePrivilegeCheck(SE_LOAD_DRIVER, mode) };
    accorde != 0
}
