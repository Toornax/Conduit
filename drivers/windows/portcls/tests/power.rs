//! `AdapterPowerManagement` vu par un faux PortCls : `QueryInterface`,
//! `PowerChangeState(PowerDeviceD3)`, `QueryPowerChangeState`, `QueryDeviceCapabilities`,
//! `Release`, uniquement par la vtable.
//!
//! Depuis M1b-06, ce fichier couvre aussi la **règle** que le pilote tire d'un
//! `PowerChangeState` ([`portcls::is_powered`]) et le **schéma d'armement** que
//! `conduit_kmd::power` en fait — sur un double, `Adaptateur`, qui compte des câbles au
//! lieu d'armer de vrais `EX_TIMER`. C'est la seule façon de les éprouver : `conduit-kmd`
//! ne se teste pas en mode utilisateur (`wdk-sys` lie les bibliothèques noyau même sous
//! `cargo test`). Ce qui reste hors de portée d'ici, et ne se mesure qu'en machine :
//! l'appel réel à `ExCancelTimer`/`ExSetTimer`, et le fait que PortCls appelle bien notre
//! objet.

#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use std::mem::offset_of;
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use common::{This, query_interface, refcount, release, vtbl_de};
use conduit_com::{NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS, STATUS_UNSUCCESSFUL};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    _DEVICE_POWER_STATE, DEVICE_CAPABILITIES, DEVICE_POWER_STATE, IAdapterPowerManagement,
    IAdapterPowerManagementVtbl, IID_IAdapterPowerManagement, IID_IMiniportTopology, NTSTATUS,
    PDEVICE_CAPABILITIES, POWER_STATE,
};
use portcls::{
    AdapterPowerManagement, PowerVtbl, is_powered, new_power_object, try_new_power_object,
};

struct Puissance {
    states: Mutex<Vec<DEVICE_POWER_STATE>>,
    queries: AtomicU32,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Puissance {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl AdapterPowerManagement for Puissance {
    fn power_change_state(&self, new_state: DEVICE_POWER_STATE) {
        self.states.lock().unwrap().push(new_state);
    }

    fn query_power_change_state(&self, new_state: DEVICE_POWER_STATE) -> NtStatus {
        self.queries.fetch_add(1, Ordering::SeqCst);
        if new_state == _DEVICE_POWER_STATE::PowerDeviceD3 {
            STATUS_UNSUCCESSFUL
        } else {
            STATUS_SUCCESS
        }
    }

    fn query_device_capabilities(&self, caps: &mut DEVICE_CAPABILITIES) -> NtStatus {
        caps.D3Latency = 42;
        caps.DeviceWake = _DEVICE_POWER_STATE::PowerDeviceD0;
        STATUS_SUCCESS
    }
}

/// Implémentation minimale : les défauts du trait.
struct Defauts;

impl AdapterPowerManagement for Defauts {
    fn power_change_state(&self, _: DEVICE_POWER_STATE) {}
}

fn nouveau() -> (This, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let obj = new_power_object(Puissance {
        states: Mutex::new(Vec::new()),
        queries: AtomicU32::new(0),
        dropped: Arc::clone(&dropped),
    });
    (obj.into_raw(), dropped)
}

fn puissance(this: This) -> &'static Puissance {
    unsafe { conduit_com::ComObject::<IAdapterPowerManagementVtbl, Puissance>::inner(this) }
}

fn etat(device: DEVICE_POWER_STATE) -> POWER_STATE {
    POWER_STATE {
        DeviceState: device,
    }
}

fn appel_power_change_state(this: This, state: DEVICE_POWER_STATE) {
    let slot = unsafe { vtbl_de::<IAdapterPowerManagementVtbl>(this) }
        .PowerChangeState
        .expect("slot PowerChangeState");
    unsafe { slot(this, etat(state)) }
}

fn appel_query_power_change_state(this: This, state: DEVICE_POWER_STATE) -> NTSTATUS {
    let slot = unsafe { vtbl_de::<IAdapterPowerManagementVtbl>(this) }
        .QueryPowerChangeState
        .expect("slot QueryPowerChangeState");
    unsafe { slot(this, etat(state)) }
}

fn appel_query_device_capabilities(this: This, caps: PDEVICE_CAPABILITIES) -> NTSTATUS {
    let slot = unsafe { vtbl_de::<IAdapterPowerManagementVtbl>(this) }
        .QueryDeviceCapabilities
        .expect("slot QueryDeviceCapabilities");
    unsafe { slot(this, caps) }
}

#[test]
fn query_interface_iunknown_et_iadapterpowermanagement() {
    let (this, dropped) = nouveau();

    for iid in [IID_IUNKNOWN, guid(&IID_IAdapterPowerManagement)] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_SUCCESS, "{iid}");
        assert_eq!(out, this, "{iid}");
    }
    assert_eq!(refcount(this), 3);

    let (status, out) = query_interface(this, &guid(&IID_IMiniportTopology));
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());

    assert_eq!(release(this), 2);
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn power_change_state_transmet_l_etat() {
    let (this, dropped) = nouveau();

    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD0);
    assert_eq!(
        *puissance(this).states.lock().unwrap(),
        vec![
            _DEVICE_POWER_STATE::PowerDeviceD3,
            _DEVICE_POWER_STATE::PowerDeviceD0
        ]
    );

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn query_power_change_state_et_capabilities() {
    let (this, dropped) = nouveau();

    assert_eq!(
        appel_query_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD1),
        STATUS_SUCCESS
    );
    assert_eq!(
        appel_query_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3),
        STATUS_UNSUCCESSFUL
    );
    assert_eq!(puissance(this).queries.load(Ordering::SeqCst), 2);

    let mut caps = DEVICE_CAPABILITIES::default();
    assert_eq!(
        appel_query_device_capabilities(this, &mut caps),
        STATUS_SUCCESS
    );
    assert_eq!(caps.D3Latency, 42);
    assert_eq!(caps.DeviceWake, _DEVICE_POWER_STATE::PowerDeviceD0);
    assert_eq!(
        appel_query_device_capabilities(this, ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn defauts_du_trait() {
    let this = new_power_object(Defauts).into_raw();
    assert_eq!(
        appel_query_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3),
        STATUS_SUCCESS
    );
    let mut caps = DEVICE_CAPABILITIES {
        D3Latency: 7,
        ..Default::default()
    };
    assert_eq!(
        appel_query_device_capabilities(this, &mut caps),
        STATUS_SUCCESS
    );
    assert_eq!(caps.D3Latency, 7, "les capacités ne sont pas touchées");
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD0);
    assert_eq!(release(this), 0);
}

#[test]
fn dispositions_et_vtable_par_type() {
    assert_eq!(offset_of!(IAdapterPowerManagement, lpVtbl), 0);

    let (this, _dropped) = nouveau();
    let premier_mot = unsafe { *this.cast::<*const IAdapterPowerManagementVtbl>() };
    let obj =
        unsafe { conduit_com::ComPtr::<IAdapterPowerManagementVtbl, Puissance>::from_raw(this) };
    assert!(ptr::eq(premier_mot, obj.object().vtbl()));
    let vt = obj.object().vtbl();
    assert!(vt.QueryInterface.is_some() && vt.AddRef.is_some() && vt.Release.is_some());
    assert!(
        vt.PowerChangeState.is_some()
            && vt.QueryPowerChangeState.is_some()
            && vt.QueryDeviceCapabilities.is_some()
    );
    assert!(<Puissance as PowerVtbl>::VTBL.PowerChangeState.is_some());

    let autre = try_new_power_object(Defauts).expect("allocation en mode utilisateur");
    assert!(
        !ptr::eq(premier_mot, autre.object().vtbl()),
        "une vtable par type"
    );
    fn exige<T: Send + Sync>(_: &T) {}
    exige(&obj);
    exige(&autre);
}

#[test]
fn seul_d0_est_allume() {
    assert!(is_powered(_DEVICE_POWER_STATE::PowerDeviceD0));
    for eteint in [
        _DEVICE_POWER_STATE::PowerDeviceUnspecified,
        _DEVICE_POWER_STATE::PowerDeviceD1,
        _DEVICE_POWER_STATE::PowerDeviceD2,
        _DEVICE_POWER_STATE::PowerDeviceD3,
        _DEVICE_POWER_STATE::PowerDeviceMaximum,
    ] {
        assert!(!is_powered(eteint), "état {eteint} pris pour allumé");
    }
    // Le champ est un `c_int` : une valeur hors de l'énumération doit tomber du côté
    // prudent, pas provoquer de panique ni passer pour `D0`.
    for inattendu in [-1, 42, i32::MIN, i32::MAX] {
        assert!(!is_powered(inattendu), "état {inattendu} pris pour allumé");
    }
}

// ---------------------------------------------------------------------------
// Le schéma d'armement de `conduit_kmd::power`, sur un double
// ---------------------------------------------------------------------------

/// Nombre de câbles du double (peu importe lequel : c'est la mécanique qu'on éprouve).
const CABLES_FACTICES: usize = 3;

/// Un câble du double : ce que `conduit_kmd::cable::Cable` retiendrait de l'armement,
/// sans `EX_TIMER`.
#[derive(Debug, Default)]
struct CableFactice {
    /// Un flux tourne avec un tampon (`StreamState::is_live`).
    live: AtomicU32,
    /// Le minuteur est armé (`CableState::armed`).
    armed: AtomicU32,
    /// Nombre d'appels effectifs à « armer » : c'est lui qui distingue un vrai réarmement
    /// d'un différentiel qui n'a rien fait.
    armements: AtomicU32,
}

impl CableFactice {
    fn live(&self) -> bool {
        self.live.load(Ordering::SeqCst) != 0
    }

    fn armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst) != 0
    }

    fn set_live(&self, live: bool) {
        self.live.store(u32::from(live), Ordering::SeqCst);
    }

    /// `Cable::refresh_timer` : un **différentiel**, qui ne fait rien quand l'état voulu
    /// est celui qu'il croit avoir.
    fn refresh_timer(&self) {
        let wanted = self.live();
        if wanted == self.armed() {
            return;
        }
        self.armed.store(u32::from(wanted), Ordering::SeqCst);
        if wanted {
            self.armements.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// `Cable::suspend` : désarme **et oublie** qu'on était armé.
    fn suspend(&self) {
        self.armed.store(0, Ordering::SeqCst);
    }
}

/// Le double de `conduit_kmd::power::AdapterPower` : même règle, mêmes deux branches.
#[derive(Debug)]
struct Adaptateur {
    cables: [CableFactice; CABLES_FACTICES],
}

impl Default for Adaptateur {
    fn default() -> Self {
        Self {
            cables: std::array::from_fn(|_| CableFactice::default()),
        }
    }
}

impl AdapterPowerManagement for Adaptateur {
    fn power_change_state(&self, new_state: DEVICE_POWER_STATE) {
        let allume = is_powered(new_state);
        for cable in &self.cables {
            if allume {
                cable.refresh_timer();
            } else {
                cable.suspend();
            }
        }
    }
}

fn adaptateur(this: This) -> &'static Adaptateur {
    unsafe { conduit_com::ComObject::<IAdapterPowerManagementVtbl, Adaptateur>::inner(this) }
}

/// Un adaptateur dont tous les câbles tournent, minuteurs armés — l'état d'un pilote qui
/// transporte de l'audio au moment où la machine s'endort.
fn adaptateur_en_marche() -> This {
    let obj = new_power_object(Adaptateur::default());
    let this = obj.into_raw();
    for cable in &adaptateur(this).cables {
        cable.set_live(true);
        cable.refresh_timer();
        assert!(cable.armed(), "le câble devait être armé au départ");
    }
    this
}

#[test]
fn d3_desarme_et_d0_rearme() {
    let this = adaptateur_en_marche();

    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    for cable in &adaptateur(this).cables {
        assert!(!cable.armed(), "D3 doit désarmer");
        assert_eq!(cable.armements.load(Ordering::SeqCst), 1);
    }

    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD0);
    for cable in &adaptateur(this).cables {
        assert!(cable.armed(), "D0 doit réarmer un flux resté vivant");
        // Le compte augmente : c'est un vrai réarmement, pas un différentiel qui n'a rien
        // fait. Si `suspend` avait désarmé sans remettre `armed` à faux, `refresh_timer`
        // serait sorti tout de suite et le câble se serait réveillé muet.
        assert_eq!(cable.armements.load(Ordering::SeqCst), 2);
    }

    assert_eq!(release(this), 0);
}

#[test]
fn d0_ne_rearme_pas_un_cable_sans_flux() {
    let this = adaptateur_en_marche();
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    // La pile audio a fermé ses flux pendant la veille : plus rien à faire tourner.
    for cable in &adaptateur(this).cables {
        cable.set_live(false);
    }

    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD0);
    for cable in &adaptateur(this).cables {
        assert!(!cable.armed(), "rien à armer sans flux vivant");
        assert_eq!(cable.armements.load(Ordering::SeqCst), 1);
    }

    assert_eq!(release(this), 0);
}

#[test]
fn un_etat_inattendu_ne_panique_pas_et_desarme() {
    let this = adaptateur_en_marche();

    // Les états intermédiaires, les bornes de l'énumération, et des valeurs qu'aucune
    // constante ne nomme : `PowerChangeState` ne rend rien et n'a pas le droit d'échouer,
    // donc tout ce qu'on peut exiger est qu'aucun ne panique et qu'aucun ne passe pour D0.
    for etat in [
        _DEVICE_POWER_STATE::PowerDeviceUnspecified,
        _DEVICE_POWER_STATE::PowerDeviceD1,
        _DEVICE_POWER_STATE::PowerDeviceD2,
        _DEVICE_POWER_STATE::PowerDeviceMaximum,
        -1,
        1234,
    ] {
        appel_power_change_state(this, etat);
        for cable in &adaptateur(this).cables {
            assert!(!cable.armed(), "l'état {etat} n'aurait pas dû armer");
        }
    }
    // Deux `D3` de suite : idempotent, comme le veut le contrat de la méthode.
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    for cable in &adaptateur(this).cables {
        assert!(!cable.armed());
        assert_eq!(cable.armements.load(Ordering::SeqCst), 1);
    }

    assert_eq!(release(this), 0);
}

#[test]
fn l_enregistrement_par_query_interface_garde_l_objet_vivant() {
    // Ce que `PcRegisterAdapterPowerManagement` fait de l'`IUnknown` qu'on lui remet est
    // documenté : « The PortCls system driver queries this object for its
    // IAdapterPowerManagement interface by calling QueryInterface on this object with
    // REFIID IID_IAdapterPowerManagement ». C'est sur ce seul fait que repose le droit,
    // pour `conduit_kmd::power::register`, de relâcher sa propre référence en sortant.
    let (this, dropped) = nouveau();

    let (status, out) = query_interface(this, &guid(&IID_IAdapterPowerManagement));
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(out, this);
    assert_eq!(
        refcount(this),
        2,
        "QueryInterface doit prendre sa référence"
    );

    // Sortie de `register` : notre `ComPtr` est détruit.
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0, "l'objet doit survivre");

    // Et le faux PortCls peut encore l'appeler, longtemps après.
    appel_power_change_state(this, _DEVICE_POWER_STATE::PowerDeviceD3);
    assert_eq!(
        *puissance(this).states.lock().unwrap(),
        vec![_DEVICE_POWER_STATE::PowerDeviceD3]
    );

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
