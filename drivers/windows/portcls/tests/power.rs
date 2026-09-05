//! `AdapterPowerManagement` vu par un faux PortCls : `QueryInterface`,
//! `PowerChangeState(PowerDeviceD3)`, `QueryPowerChangeState`, `QueryDeviceCapabilities`,
//! `Release`, uniquement par la vtable.

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
use portcls::{AdapterPowerManagement, PowerVtbl, new_power_object, try_new_power_object};

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
