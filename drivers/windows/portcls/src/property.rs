//! Propriétés KS : trait sûr [`PropertyHandler`], thunk [`handler`] et constructeur
//! d'entrée de table [`item`].
//!
//! PortCls route les propriétés de façon déclarative : il trouve la cible (filtre,
//! broche ou nœud), cherche le `PCPROPERTY_ITEM` de même `Set`/`Id` dans la
//! `PCAUTOMATION_TABLE` de cette cible, puis appelle son `Handler` avec une
//! `PCPROPERTY_REQUEST` qui décrit tout : `MajorTarget` (l'`IUnknown` de notre
//! miniport), `Node`, `Verb`, `Instance`/`InstanceSize`, `Value`/`ValueSize`, `Irp`.
//!
//! # Frontière de confiance
//!
//! C'est le point d'entrée le plus exposé du pilote : la requête vient d'un
//! `IOCTL_KS_PROPERTY` émis par un processus utilisateur quelconque.
//!
//! - **Garanti par KS** : `Instance` et `Value`, s'ils ne sont pas nuls, sont des
//!   adresses **noyau** valides sur `InstanceSize`/`ValueSize` octets — KS a fait les
//!   `ProbeForRead`/`ProbeForWrite` et la copie avant de nous appeler.
//! - **Hostile** : les **tailles** (0, `u32::MAX`, n'importe quoi) et le **contenu**.
//!   Jamais de supposition du genre « `ValueSize >= size_of::<Machin>()` ».
//! - **Pas garanti** : l'**alignement** des tampons.
//!
//! D'où trois règles que ce module applique sans exception :
//!
//! 1. le thunk ne construit que des **tranches d'octets** (`&[u8]` / `&mut [u8]`) —
//!    `u8` n'a aucune contrainte d'alignement — et une tranche vide si le pointeur est
//!    nul ou la taille nulle ;
//! 2. **aucun transtypage vers un `*mut` de structure, aucune écriture à travers** : une
//!    écriture désalignée par un `*mut T` est un comportement indéfini en Rust même là où
//!    x64 la tolère au niveau du processeur. Les gestionnaires sérialisent champ par
//!    champ (`value.get_mut(a..b)` + `copy_from_slice(&x.to_ne_bytes())`), ce qui passe
//!    naturellement le lint `indexing_slicing` du workspace ;
//! 3. `usize::try_from` sur toute taille venue de la requête, `STATUS_INVALID_PARAMETER`
//!    en cas d'échec, comme [`crate::miniport`].
//!
//! # Garde de vtable
//!
//! `MajorTarget` **est** le `*mut ComObject<V, T>` de notre miniport : `QueryInterface`
//! rend `this` tel quel pour `IID_IUnknown`, `IID_IMiniport` et `IID_IMiniportTopology`
//! (un seul objet, une seule vtable, pas d'ajustement de pointeur COM). `ComObject::inner`
//! suffit donc à retrouver l'état.
//!
//! Mais **rien, au niveau des types, ne relie une `PCAUTOMATION_TABLE` au miniport auquel
//! elle appartient** : la table est une `static` du pilote, désignée par un pointeur dans
//! un `PCFILTER_DESCRIPTOR`. Une erreur de câblage donnerait un `inner::<V, T>` sur un
//! objet d'un autre type — un écran bleu, au mieux. Le thunk compare donc le premier mot
//! de l'objet (le `&'static V` d'un `ComObject`, à l'offset 0 par `repr(C)`) à la vtable
//! attendue, et répond `STATUS_INVALID_DEVICE_REQUEST` si elle diffère.
//!
//! Encore faut-il que « la vtable attendue » ait **une seule adresse**, et c'est le point
//! délicat. `&T::VTBL` est une **constante promue**, émise en `unnamed_addr` : ni son
//! unicité ni sa non-duplication ne sont garanties. La comparaison passe donc par
//! [`TargetVtbl::vtbl`], dont chaque implémentation délègue à une unique fonction
//! générique — [`topology::vtbl_of`](crate::topology::vtbl_of),
//! [`wavert::vtbl_of`](crate::wavert::vtbl_of) — qui porte la **seule** occurrence de
//! `&T::VTBL` du crate, celle-là même que `new_topology_object` / `new_wavert_object`
//! donnent à `ComObject::new`, et qui est marquée **`#[inline(never)]`** pour que
//! l'allocation promue ne soit pas dupliquée d'une unité de génération de code à l'autre.
//! Les deux précautions sont nécessaires : sans l'attribut, la garde refuse des objets
//! légitimes dès qu'on optimise (voir la documentation de `topology::vtbl_of`, et les
//! tests de `tests/property.rs`, qui le démontrent en `--release`).

use core::ffi::c_void;
use core::fmt;
use core::slice;

use conduit_com::{ComObject, ComVtable, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls_sys::{
    GUID, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_SET, NTSTATUS,
    PCPROPERTY_ITEM, PPCPROPERTY_REQUEST,
};

use crate::status::{
    STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_SUPPORTED,
};

/// Occurrence **canonique** de la vtable d'un `ComObject<Self, T>`.
///
/// Implémenté par vtable (`IMiniportTopologyVtbl`, `IMiniportWaveRTVtbl`) pour tout type
/// implémenteur du trait métier correspondant, comme [`MiniportSlots`](crate::miniport::MiniportSlots) :
/// deux `impl<T: …> Trait for T` couvrants seraient rejetés par la cohérence, deux `impl`
/// sur des types `Self` distincts ne le sont pas.
///
/// # Safety
///
/// [`vtbl`](Self::vtbl) doit renvoyer **exactement** la référence que le constructeur
/// d'objet du crate passe à `ComObject::<Self, T>::new` — même allocation, même adresse,
/// pas une seconde promotion de `&T::VTBL` (voir la garde de vtable en tête de module :
/// une seule occurrence, et `#[inline(never)]` dessus). La garde de [`handler`] en dépend
/// dans les deux sens : un faux négatif (deux adresses pour la même vtable) rendrait le
/// pilote muet, un faux positif (la même adresse pour deux types) ferait un `inner` sur un
/// objet d'un autre type, donc un comportement indéfini.
pub unsafe trait TargetVtbl<T: Send + Sync + 'static>: ComVtable {
    /// La vtable de `T`, à l'adresse unique que porte tout `ComObject<Self, T>` vivant.
    fn vtbl() -> &'static Self;
}

/// Ce qu'un gestionnaire de propriété reçoit d'une `PCPROPERTY_REQUEST`, en sûr.
///
/// Les tampons `Value` ne sont pas ici : ils sont passés séparément aux méthodes de
/// [`PropertyHandler`], en lecture (`set`) ou en écriture (`get`, `basic_support`).
pub struct Request<'a, T> {
    /// Le miniport, retrouvé depuis `MajorTarget` après vérification de sa vtable.
    pub target: &'a T,
    /// `PCPROPERTY_REQUEST::Node` : le nœud visé, ou `ULONG(-1)` si la propriété porte
    /// sur le filtre ou une broche.
    pub node: u32,
    /// `Instance`/`InstanceSize` en octets bruts (la partie de `KSPROPERTY` qui suit
    /// l'en-tête : indice de canal d'un nœud de volume, par exemple). Tranche **vide**
    /// si `Instance` est nul ou `InstanceSize` nulle ; jamais alignée par contrat.
    pub instance: &'a [u8],
}

impl<T> fmt::Debug for Request<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("node", &self.node)
            .field("instance_len", &self.instance.len())
            .finish_non_exhaustive()
    }
}

/// Gestionnaire d'une propriété KS, côté pilote : un type sans état, un verbe par
/// méthode.
///
/// Les trois méthodes ont un défaut `Err(STATUS_NOT_SUPPORTED)` : un gestionnaire ne
/// déclare que ce qu'il sait faire, et les `Flags` du [`item`] correspondant doivent
/// s'accorder avec (PortCls filtre déjà sur les flags, la valeur de retour est la
/// seconde ligne de défense).
///
/// # Asymétrie entre `get` et `basic_support`
///
/// [`get`](Self::get) renvoie la taille **requise**, écrite ou non ; le thunk en déduit
/// `STATUS_SUCCESS`, `STATUS_BUFFER_OVERFLOW` ou `STATUS_BUFFER_TOO_SMALL` et la reporte
/// toujours dans `ValueSize`. C'est la négociation de taille habituelle de KS, la même
/// que `DataRangeIntersection` (voir [`crate::miniport`]).
///
/// [`basic_support`](Self::basic_support) renvoie au contraire le nombre d'octets
/// **écrits**, et le thunk répond toujours `STATUS_SUCCESS`. La raison est dans le
/// protocole : KS interroge `KSPROPERTY_TYPE_BASICSUPPORT` **deux fois**, d'abord avec
/// `sizeof(ULONG)` pour ne récupérer que les `AccessFlags`, puis avec la taille complète
/// (`KSPROPERTY_DESCRIPTION` + `KSPROPERTY_MEMBERSHEADER` + les plages). Répondre
/// `STATUS_BUFFER_TOO_SMALL` au premier appel casserait la négociation : le client
/// conclurait que la propriété n'est pas descriptible et abandonnerait. Le gestionnaire
/// écrit donc ce qui tient et dit combien ; c'est lui qui connaît les paliers de son
/// descripteur.
///
/// # Frontière de confiance
///
/// `value` est une tranche d'octets de taille arbitraire (y compris vide) et
/// **d'alignement quelconque**. Sérialiser champ par champ (`get_mut(a..b)` +
/// `copy_from_slice(&x.to_ne_bytes())`), jamais par transtypage vers un `*mut` de
/// structure : voir la documentation du module.
pub trait PropertyHandler<T: Send + Sync + 'static>: 'static {
    /// `KSPROPERTY_TYPE_GET` : lire la propriété.
    ///
    /// Renvoie la taille **requise** de la valeur, qu'elle ait été écrite ou non ; ne
    /// rien écrire si `value` est trop court.
    ///
    /// IRQL : `PASSIVE_LEVEL` (fait documenté du contrat PortCls, non mesuré : un
    /// `IOCTL_KS_PROPERTY` est traité en ligne dans le contexte du fil appelant).
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let _ = (req, value);
        Err(STATUS_NOT_SUPPORTED)
    }

    /// `KSPROPERTY_TYPE_SET` : écrire la propriété.
    ///
    /// `value` est le contenu fourni par le client : hostile, de taille arbitraire, à
    /// valider entièrement avant d'en tirer quoi que ce soit.
    ///
    /// IRQL : `PASSIVE_LEVEL` (fait documenté du contrat PortCls, non mesuré).
    fn set(req: &Request<'_, T>, value: &[u8]) -> Result<(), NtStatus> {
        let _ = (req, value);
        Err(STATUS_NOT_SUPPORTED)
    }

    /// `KSPROPERTY_TYPE_BASICSUPPORT` : décrire la propriété (`AccessFlags`, type,
    /// plages).
    ///
    /// Renvoie le nombre d'octets **écrits** (voir l'asymétrie documentée sur le trait) :
    /// écrire ce qui tient dans `value`, et rendre ce compte.
    ///
    /// IRQL : `PASSIVE_LEVEL` (fait documenté du contrat PortCls, non mesuré).
    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let _ = (req, value);
        Err(STATUS_NOT_SUPPORTED)
    }
}

/// Entrée de `PCAUTOMATION_TABLE` câblée sur `H` : `Set`, `Id`, `Flags` et le thunk
/// [`handler`] monomorphisé.
///
/// `flags` est une combinaison de `PCPROPERTY_ITEM_FLAG_GET`, `_SET` et `_BASICSUPPORT`
/// (les `KSPROPERTY_TYPE_*` homonymes) : PortCls s'en sert pour filtrer les verbes avant
/// même de nous appeler, et pour répondre aux énumérations de propriétés. À tenir en
/// accord avec les méthodes que `H` implémente réellement.
///
/// `const fn` : les tables d'automatisation du pilote sont des `static`.
pub const fn item<V, T, H>(set: &'static GUID, id: u32, flags: u32) -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: Send + Sync + 'static,
    H: PropertyHandler<T>,
{
    PCPROPERTY_ITEM {
        Set: set,
        Id: id,
        Flags: flags,
        Handler: Some(handler::<V, T, H>),
    }
}

/// `PCPFNPROPERTY_HANDLER` : le thunk de PortCls vers `H`.
///
/// Retrouve le miniport depuis `MajorTarget` (après la garde de vtable décrite en tête de
/// module), convertit `Instance` et `Value` en tranches d'octets, aiguille sur `Verb` et
/// traduit le `Result` en (`NTSTATUS`, `ValueSize`).
///
/// # Aiguillage du verbe
///
/// `Verb` est un **masque**, testé défensivement dans cet ordre : `BASICSUPPORT` d'abord,
/// car son bit est distinct des deux autres et KS peut l'envoyer seul ou combiné ; puis
/// `SET` ; puis `GET`. Tout le reste (`0`, `SETSUPPORT`, `SERIALIZERAW`…) est
/// `STATUS_INVALID_DEVICE_REQUEST`.
///
/// # Table des statuts
///
/// | Cas | Code | `ValueSize` en sortie |
/// |---|---|---|
/// | `request` nul | `STATUS_INVALID_PARAMETER` | — |
/// | `MajorTarget` nul | `STATUS_INVALID_DEVICE_REQUEST` | intact |
/// | vtable inattendue | `STATUS_INVALID_DEVICE_REQUEST` | intact |
/// | verbe hors GET/SET/BASICSUPPORT | `STATUS_INVALID_DEVICE_REQUEST` | intact |
/// | verbe non implémenté par `H` | `STATUS_NOT_SUPPORTED` | intact |
/// | GET `Ok(n)`, place suffisante | `STATUS_SUCCESS` | `n` |
/// | GET `Ok(n)`, tampon vide | `STATUS_BUFFER_OVERFLOW` | `n` |
/// | GET `Ok(n)`, tampon non vide trop court | `STATUS_BUFFER_TOO_SMALL` | `n` |
/// | GET/SET `Err(s)` | `s` | intact |
/// | SET `Ok(())` | `STATUS_SUCCESS` | intact |
/// | BASICSUPPORT `Ok(w)` | `STATUS_SUCCESS` | `w` |
///
/// « Tampon vide » signifie `Value` nul **ou** `ValueSize` nulle : les deux sont la même
/// interrogation de taille pour KS. `ValueSize` reste intact sur toute erreur, comme pour
/// `DataRangeIntersection`.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `request`, s'il n'est pas nul, pointe une `PCPROPERTY_REQUEST` lisible et inscriptible
/// le temps de l'appel, dont `MajorTarget` est nul ou pointe un objet COM vivant (donc
/// porteur d'un pointeur de vtable à l'offset 0), et dont `Instance`/`Value`, s'ils ne
/// sont pas nuls, pointent `InstanceSize`/`ValueSize` octets noyau accessibles — c'est
/// exactement le contrat de PortCls.
pub unsafe extern "C" fn handler<V, T, H>(request: PPCPROPERTY_REQUEST) -> NTSTATUS
where
    V: TargetVtbl<T>,
    T: Send + Sync + 'static,
    H: PropertyHandler<T>,
{
    if request.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `request` est non nul et pointe une `PCPROPERTY_REQUEST` lisible et
    // inscriptible le temps de l'appel (contrat) ; PortCls ne la partage avec personne
    // pendant que le gestionnaire s'exécute.
    let req = unsafe { &mut *request };

    let this: *mut c_void = req.MajorTarget.cast();
    if this.is_null() {
        return STATUS_INVALID_DEVICE_REQUEST;
    }

    // Garde de vtable (voir la doc du module) : le premier mot de tout objet COM est son
    // pointeur de vtable ; celui d'un `ComObject<V, T>` est l'occurrence canonique
    // `V::vtbl()`. Lire ce mot ne suppose pas encore que l'objet est le nôtre.
    // SAFETY: `this` est non nul et pointe un objet COM vivant (contrat) : `lpVtbl` est à
    // l'offset 0 et fait la taille d'un pointeur (`ComObject` est `repr(C)`, avec une
    // assertion `offset_of!(…, vtbl) == 0` dans `conduit-com`).
    let observee = unsafe { *this.cast::<*const c_void>() };
    let attendue: *const c_void = (V::vtbl() as *const V).cast();
    if observee != attendue {
        return STATUS_INVALID_DEVICE_REQUEST;
    }
    // SAFETY: la vtable observée est celle que le constructeur d'objet du crate donne aux
    // `ComObject<V, T>` et à eux seuls (contrat de `TargetVtbl`) : `this` en est un, et il
    // reste vivant pendant l'appel puisque PortCls détient une référence dessus.
    let target = unsafe { ComObject::<V, T>::inner(this) };

    // Tailles hostiles : converties, jamais supposées.
    let Ok(instance_len) = usize::try_from(req.InstanceSize) else {
        return STATUS_INVALID_PARAMETER;
    };
    let Ok(value_len) = usize::try_from(req.ValueSize) else {
        return STATUS_INVALID_PARAMETER;
    };

    let instance: &[u8] = if req.Instance.is_null() || instance_len == 0 {
        &[]
    } else {
        // SAFETY: `Instance` est non nul et KS garantit `InstanceSize` octets noyau
        // lisibles (les `Probe*` et la copie sont faits) ; `u8` n'a pas de contrainte
        // d'alignement, et rien n'écrit dans ce tampon pendant l'appel.
        unsafe { slice::from_raw_parts(req.Instance.cast::<u8>(), instance_len) }
    };

    let req_sur = Request {
        target,
        node: req.Node,
        instance,
    };

    // Tampon de valeur : pointeur et longueur retenus à part, la tranche n'est construite
    // que dans la branche qui en a besoin (et à la bonne mutabilité).
    let value_ptr = req.Value.cast::<u8>();
    let value_len = if value_ptr.is_null() { 0 } else { value_len };

    let verb = req.Verb;
    if verb & KSPROPERTY_TYPE_BASICSUPPORT != 0 {
        let value: &mut [u8] = if value_len == 0 {
            &mut []
        } else {
            // SAFETY: `Value` est non nul et KS garantit `ValueSize` octets noyau
            // inscriptibles, exclusifs le temps de l'appel ; `u8` n'a pas de contrainte
            // d'alignement.
            unsafe { slice::from_raw_parts_mut(value_ptr, value_len) }
        };
        match H::basic_support(&req_sur, value) {
            Ok(ecrits) => {
                req.ValueSize = ecrits;
                STATUS_SUCCESS
            }
            Err(status) => status,
        }
    } else if verb & KSPROPERTY_TYPE_SET != 0 {
        let value: &[u8] = if value_len == 0 {
            &[]
        } else {
            // SAFETY: `Value` est non nul et KS garantit `ValueSize` octets noyau
            // lisibles ; `u8` n'a pas de contrainte d'alignement.
            unsafe { slice::from_raw_parts(value_ptr.cast_const(), value_len) }
        };
        match H::set(&req_sur, value) {
            Ok(()) => STATUS_SUCCESS,
            Err(status) => status,
        }
    } else if verb & KSPROPERTY_TYPE_GET != 0 {
        let value: &mut [u8] = if value_len == 0 {
            &mut []
        } else {
            // SAFETY: `Value` est non nul et KS garantit `ValueSize` octets noyau
            // inscriptibles, exclusifs le temps de l'appel ; `u8` n'a pas de contrainte
            // d'alignement.
            unsafe { slice::from_raw_parts_mut(value_ptr, value_len) }
        };
        match H::get(&req_sur, value) {
            Ok(requise) => {
                req.ValueSize = requise;
                if value_len == 0 {
                    // Interrogation de taille : `Value` nul ou `ValueSize` nulle.
                    STATUS_BUFFER_OVERFLOW
                } else if usize::try_from(requise).is_ok_and(|n| value_len >= n) {
                    STATUS_SUCCESS
                } else {
                    STATUS_BUFFER_TOO_SMALL
                }
            }
            Err(status) => status,
        }
    } else {
        STATUS_INVALID_DEVICE_REQUEST
    }
}
