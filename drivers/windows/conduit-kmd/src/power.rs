//! Gestion d'alimentation de l'adaptateur (M1b-06) : l'objet `IAdapterPowerManagement`
//! du pilote, enregistré auprès de PortCls par [`register`] depuis
//! [`crate::adapter::start_device`].
//!
//! L'enveloppe sûre est celle de `portcls::power` (trait
//! [`AdapterPowerManagement`], vtable engendrée par type) ; ce module n'en fournit que
//! l'implémenteur, [`AdapterPower`], et le branchement sur les câbles.
//!
//! # Ce que PortCls fait déjà — et qu'on ne refait pas
//!
//! C'est le point de départ de tout ce module, et il est **documenté**. Sur
//! `IAdapterPowerManagement::PowerChangeState` : « To assist the driver, PortCls will
//! pause any active audio streams prior to calling this method to place the device in a
//! sleep state. After calling this method, PortCls will unpause active audio streams, to
//! wake the device up. » Et, sur le sens de la montée : « PortCls always places the device
//! in the PowerDeviceD0 state before calling the miniport driver's NewStream method. »
//!
//! Conséquence directe pour Conduit : à la descente, PortCls a **déjà** sorti les flux de
//! `KSSTATE_RUN` quand il nous appelle ; nos `stream::WaveStream::set_state` ont donc déjà
//! appelé `Cable::refresh_timer`, qui a désarmé. À la montée, PortCls ne les remet en
//! marche qu'**après** notre retour, et c'est encore `set_state` qui réarme. Le pilote
//! n'a donc, en régime nominal, **rien à faire** de ces transitions.
//!
//! Ce module existe pour ce qui reste : la garantie que **rien de périodique ne tourne
//! pendant que le périphérique est éteint**, sans dépendre de l'ordre exact dans lequel
//! PortCls aura mis les flux en pause. Un minuteur haute résolution de 1 ms par câble qui
//! survivrait à une entrée en veille est précisément le genre de chose qu'on ne veut pas
//! avoir à déduire d'une trace. Deux appels par câble et par transition, idempotents :
//! c'est la version la plus légère de « ne rien faire tourner en D3 » qui ne suppose rien.
//!
//! # La règle, et où elle est éprouvée
//!
//! `PowerChangeState` reçoit une union `POWER_STATE` dont c'est le champ **`DeviceState`**
//! qui est valide pour cette interface (documenté ; `SystemState` n'arrive qu'à
//! `IAdapterPowerManagement2::PowerChangeState2`, que le pilote n'implémente pas). La
//! décision qu'on en tire tient en une ligne — `D0` ou pas `D0` — et vit dans
//! `portcls::power::is_powered`, **pas ici** : `conduit-kmd` ne se teste pas en mode
//! utilisateur, `portcls` si. `portcls/tests/power.rs` éprouve donc la règle et le
//! passage par la vtable ; seul le branchement sur les minuteurs se mesure en machine.
//!
//! # `QueryPowerChangeState` : on n'refuse rien, et c'est délibéré
//!
//! La documentation autorise le refus (« The driver can deny the power state change by
//! returning a value other than STATUS_SUCCESS »), mais ne dit **pas** ce qu'un pilote
//! devrait refuser. Trois raisons de tout accepter, et la première suffirait :
//!
//! 1. **Un câble virtuel n'a rien à préserver.** Il n'y a ni matériel à vidanger, ni
//!    tampon d'échantillons dont la perte serait irréparable, ni opération longue à
//!    terminer. Refuser une mise en veille pour protéger une copie de 1 ms serait un
//!    mauvais échange contre l'autonomie de la machine.
//! 2. **Le refus n'est pas fiable.** La documentation le dit deux fois : « A call to
//!    QueryPowerStateChange is not guaranteed to occur prior to all PowerChangeState
//!    calls », et, côté gestionnaire d'alimentation, « Although a driver might fail a
//!    system query-power IRP, the power manager might still change the system power state
//!    to a sleep state ». Un pilote qui compterait sur son veto se tromperait.
//! 3. **`PowerChangeState` n'a pas le droit d'échouer** (« This call must not fail »,
//!    retour `void`). Toute la logique doit donc supporter la transition de toute façon :
//!    un veto ne ferait que masquer, parfois, ce qui doit marcher toujours.
//!
//! Le défaut du trait (`STATUS_SUCCESS`) est donc conservé tel quel, et cette absence de
//! surcharge est le comportement voulu, pas un oubli.
//!
//! # `QueryDeviceCapabilities` : jamais appelé, et c'est sans conséquence
//!
//! Le WDK est explicite (`km/portcls.h`, l. 2777-2785, et la page de la méthode) : « In
//! order to fill in the PowerDeviceCaps structure for a device, the adapter driver should
//! call PcRegisterAdapterPowerManagement to register the IAdapterPowerManagement interface
//! at device-startup time. **The operating system queries devices before calling the
//! adapter driver's device-startup routine.** » Un enregistrement fait dans `StartDevice`
//! — le nôtre, comme celui que la page « Implementing IAdapterPowerManagement » recommande
//! — arrive donc **après** l'interrogation des capacités : la méthode ne sera pas appelée.
//!
//! Ce n'est pas une perte : la documentation ajoute « Typically, the adapter driver should
//! not change these settings », et les seules modifications permises vont vers un état
//! **plus** éteint. Conduit n'a aucune raison d'approfondir la correspondance
//! système ↔ périphérique d'un périphérique racine sans matériel. Le défaut du trait — ne
//! rien toucher — est exactement ce qu'on veut ; s'enregistrer depuis `AddDevice` pour
//! obtenir un appel dont on n'a que faire serait du travail en pure perte.
//!
//! # L'IRQL, ce qu'on en sait et ce qu'on en fait
//!
//! **Il n'est pas documenté.** Les pages des quatre rappels d'alimentation de PortCls
//! (`PowerChangeState`, `QueryPowerChangeState`, `QueryDeviceCapabilities`,
//! `IPowerNotify::PowerChangeNotify`) n'ont **pas** de ligne IRQL ; la seule contrainte
//! écrite est « The code for this method must reside in paged memory », qui exclut
//! `DISPATCH_LEVEL` sans l'énoncer. En amont, l'IRQL d'un `IRP_MN_SET_POWER` est
//! conditionnel : `PASSIVE_LEVEL` si la pile porte `DO_POWER_PAGABLE`,
//! `DISPATCH_LEVEL` si elle porte `DO_POWER_INRUSH` — et **la documentation ne dit pas**
//! lequel PortCls pose sur le FDO qu'il crée dans `PcAddAdapterDevice`.
//!
//! Le trait de `portcls::power` annonce `PASSIVE_LEVEL`, ce qui est la lecture de bon sens
//! de « paged memory ». Ce module ne s'y **fie pas** : tout ce qu'il fait en `D3` reste
//! valide jusqu'à `DISPATCH_LEVEL` inclus. Concrètement, [`crate::cable::Cable::suspend`]
//! n'appelle que `ExCancelTimer` (`<= DISPATCH_LEVEL`, documenté, et qui n'attend jamais)
//! et **jamais** `ExDeleteTimer(…, Wait = TRUE, …)`, dont la page dit en toutes lettres :
//! « If Wait is TRUE, the routine must be called at IRQL <= APC_LEVEL ». La suppression du
//! minuteur reste donc réservée à [`crate::cable::Cable::stop`], appelé depuis
//! `DriverUnload`, où `PASSIVE_LEVEL` est garanti.
//!
//! Le prix de cette prudence est nul : le minuteur reste alloué en veille, ce qui était de
//! toute façon la décision de `crate::cable` (« seuls leurs timers sont alloués par le
//! noyau, une fois au premier `StartDevice` »), et le réveil n'a donc rien à réallouer.
//!
//! # Enregistrer **une seule fois**, sous peine de bug check
//!
//! Driver Verifier, domaine `audio`, règle **`PcRegisterAdapterPower`** : un pilote
//! PortCls ne doit pas « call PcRegisterAdapterPowerManagement twice without an
//! intervening call to PcUnregisterAdapterPowerManagement ». Violation :
//! `DRIVER_VERIFIER_DETECTED_VIOLATION` (`0xC4`, sous-code `0x00071006`) — et M1b-09
//! prévoit justement de tourner Verifier armé.
//!
//! Or `StartDevice` **peut** courir plusieurs fois pour un même chargement du pilote (un
//! rééquilibrage PnP arrête puis redémarre le périphérique sans le retirer), et le pilote
//! n'a aujourd'hui aucun rappel d'arrêt d'où appeler le désenregistrement : PortCls n'en
//! offre un que par `IAdapterPnpManagement` (`PcRegisterAdapterPnpManagement`,
//! Windows 10 1511+), que Conduit n'implémente pas encore. Le seul enregistrement sûr est
//! donc **le premier**, et [`REGISTERED_FDO`] s'en souvient.
//!
//! **Ce qui reste incertain** : si PortCls conserve la registration d'un cycle PnP à
//! l'autre. La documentation ne le dit pas. Si elle se perdait, le symptôme serait
//! l'absence d'appels d'alimentation après un rééquilibrage — bénin (voir « Ce que PortCls
//! fait déjà »), et sans commune mesure avec un bug check. C'est l'arbitrage qui est fait
//! ici, et il se défait le jour où `IAdapterPnpManagement` est implémenté : le
//! désenregistrement dans `PnpStop` rendrait le second enregistrement légitime.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use portcls::conduit_com::{NtStatus, STATUS_INSUFFICIENT_RESOURCES, nt_success};
use portcls::portcls_sys::{DEVICE_POWER_STATE, PDEVICE_OBJECT};
use portcls::{
    AdapterPowerManagement, as_unknown, is_powered, register_adapter_power_management,
    try_new_power_object,
};

use crate::cable;

/// L'objet de périphérique pour lequel l'objet d'alimentation a été enregistré, nul tant
/// qu'aucun enregistrement n'a réussi.
///
/// Sert de **verrou d'unicité** : voir « Enregistrer une seule fois » en tête de module.
/// Il n'est jamais remis à nul — le désenregistrement n'a pas de point d'appel — et le
/// comparer au `device` reçu permet de distinguer, dans la trace, un second `StartDevice`
/// du même périphérique d'une seconde instance d'adaptateur (que l'INF de Conduit, à un
/// seul nœud racine, ne produit pas).
static REGISTERED_FDO: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// L'objet `IAdapterPowerManagement` du pilote.
///
/// **Sans état** : tout ce sur quoi il agit est le tableau `static` des câbles
/// (`crate::cable`). C'est ce qui rend l'objet interchangeable d'un cycle à l'autre et
/// permet de n'en enregistrer qu'un pour toute la durée de chargement du pilote.
#[derive(Debug)]
pub struct AdapterPower;

impl AdapterPowerManagement for AdapterPower {
    /// Le périphérique change d'état d'alimentation.
    ///
    /// `D0` : réarme le minuteur de chaque câble dont un flux tourne encore avec un tampon
    /// ([`cable::Cable::refresh_timer`], qui décide seul). Tout autre état : désarme
    /// ([`cable::Cable::suspend`]). Les deux sont idempotents et ne peuvent pas échouer —
    /// ce que la documentation exige (« This call must not fail »).
    ///
    /// En régime nominal, aucun des deux n'a d'effet observable : PortCls a déjà mis les
    /// flux en pause avant de nous appeler à la descente, et ne les relance qu'après notre
    /// retour à la montée. Voir « Ce que PortCls fait déjà » en tête de module.
    ///
    /// IRQL : non documenté par PortCls ; ce chemin reste valide jusqu'à
    /// `DISPATCH_LEVEL`.
    fn power_change_state(&self, new_state: DEVICE_POWER_STATE) {
        let allume = is_powered(new_state);
        for index in 0..cable::CABLE_COUNT {
            let Some(cable) = cable::cable(index) else {
                continue;
            };
            if allume {
                cable.refresh_timer();
            } else {
                cable.suspend();
            }
        }
        if allume {
            kmd_log!("alimentation : D0, minuteurs des câbles réarmés si besoin");
        } else {
            kmd_log!("alimentation : état {new_state} (hors D0), minuteurs désarmés");
        }
    }

    // `query_power_change_state` et `query_device_capabilities` gardent le défaut du
    // trait : voir les deux sections qui leur sont consacrées en tête de module.
}

/// Enregistre l'objet d'alimentation du pilote auprès de PortCls, **au plus une fois**
/// par chargement.
///
/// Rend `Ok(())` aussi bien pour un enregistrement réussi que pour un enregistrement déjà
/// fait : dans les deux cas, l'objet est en place. `Err` ne décrit qu'un échec réel —
/// allocation impossible, ou `NTSTATUS` de `PcRegisterAdapterPowerManagement`.
///
/// # Propriété de l'objet
///
/// L'objet est créé ici, enregistré, puis **notre** référence est relâchée à la sortie
/// (`Drop` du `ComPtr`), exactement comme les miniports de
/// [`crate::adapter::install_cable`]. C'est licite parce que la documentation décrit le
/// mécanisme : « The PortCls system driver queries this object for its
/// IAdapterPowerManagement interface … by calling QueryInterface on this object with
/// REFIID IID_IAdapterPowerManagement ». Un `QueryInterface` qui réussit **incrémente le
/// compte de références** — c'est le contrat COM, et c'est ce que fait notre thunk
/// (`conduit_com::ComObject::query_interface`) : PortCls détient donc sa propre référence
/// au retour, et l'objet vit tant qu'il la garde. Si l'enregistrement échoue, la même
/// sortie de portée détruit l'objet, sans rien à défaire.
///
/// IRQL : `PASSIVE_LEVEL` (documenté pour `PcRegisterAdapterPowerManagement`).
///
/// # Safety
///
/// `device` est l'objet de périphérique fonctionnel remis à `StartDevice`.
pub unsafe fn register(device: PDEVICE_OBJECT) -> Result<(), NtStatus> {
    let precedent = REGISTERED_FDO.load(Ordering::Acquire);
    if !precedent.is_null() {
        if ptr::eq(precedent, device.cast::<c_void>()) {
            kmd_log!("alimentation : objet déjà enregistré pour ce périphérique");
        } else {
            kmd_log!(
                "alimentation : objet déjà enregistré pour {precedent:p}, {device:p} n'en aura pas"
            );
        }
        return Ok(());
    }

    let objet = try_new_power_object(AdapterPower).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;
    // SAFETY: `device` est celui de `StartDevice` (contrat) ; `objet` est vivant le temps
    // de l'appel, et PortCls prend sa propre référence par `QueryInterface` (voir la
    // section « Propriété de l'objet »).
    let status = unsafe { register_adapter_power_management(device, &as_unknown(&objet)) };
    if !nt_success(status) {
        return Err(status);
    }
    // Publié après le succès seulement : un échec doit laisser une prochaine tentative
    // possible. `StartDevice` est sérialisé par le gestionnaire PnP, mais l'échange
    // conditionnel évite d'avoir à le supposer — le perdant a enregistré le même objet
    // sans état, ce qui ne change rien à ce que PortCls appellera.
    let _ = REGISTERED_FDO.compare_exchange(
        ptr::null_mut(),
        device.cast::<c_void>(),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    Ok(())
}
