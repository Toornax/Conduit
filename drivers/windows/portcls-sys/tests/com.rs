//! Feature `com` : chaque vtable générée satisfait le contrat `ComVtable` de
//! `conduit-com` (préfixe `IUnknown` de 24 octets aux offsets 0, 8, 16), sa table `IIDS`
//! commence par `IID_IUnknown` et se termine par l'IID généré de l'interface ; `GUID`
//! généré et `conduit_com::Guid` ont la même disposition.

#![cfg(feature = "com")]
// Tests en mode utilisateur : une panique est un échec de test, pas un écran bleu.
#![allow(
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::mem::{align_of, offset_of, size_of};

use conduit_com::{ComInterface, ComVtable, Guid, IID_IUNKNOWN};
use portcls_sys::com::{IID_IUNKNOWN_PORTCLS, guid};
use portcls_sys::*;

/// Taille d'un slot : pointeur de fonction x64.
const SLOT: usize = 8;

/// Pour chaque interface : préfixe `IUnknown`, `IIDS` bien bornée, `ComInterface::Vtbl`
/// est la vtable générée.
macro_rules! contrats_com {
    ($($iface:ident / $vtbl:ident : $iid:ident [$($base:ident),*]),* $(,)?) => {
        #[test]
        fn prefixe_iunknown_de_chaque_vtable() {
            assert_eq!(size_of::<IUnknownVtbl>(), 3 * SLOT);
            $(
                assert_eq!(offset_of!($vtbl, QueryInterface), 0, stringify!($vtbl));
                assert_eq!(offset_of!($vtbl, AddRef), SLOT, stringify!($vtbl));
                assert_eq!(offset_of!($vtbl, Release), 2 * SLOT, stringify!($vtbl));
                assert!(size_of::<$vtbl>() >= 3 * SLOT, stringify!($vtbl));
                assert_eq!(align_of::<$vtbl>(), SLOT, stringify!($vtbl));
            )*
        }

        #[test]
        fn iids_de_chaque_vtable() {
            $(
                let iids = <$vtbl as ComVtable>::IIDS;
                let attendus: &[Guid] = &[IID_IUNKNOWN, $(guid(&$base),)* guid(&$iid)];
                assert_eq!(iids, attendus, stringify!($vtbl));
                assert_eq!(iids[0], IID_IUNKNOWN, stringify!($vtbl));
                assert_eq!(*iids.last().unwrap(), guid(&$iid), stringify!($vtbl));
                // Aucun doublon : chaque IID est distinct.
                for (i, a) in iids.iter().enumerate() {
                    for b in &iids[i + 1..] {
                        assert_ne!(a, b, stringify!($vtbl));
                    }
                }
            )*
        }

        #[test]
        fn interface_et_vtable_associees() {
            $(
                assert_eq!(size_of::<<$iface as ComInterface>::Vtbl>(), size_of::<$vtbl>());
                assert_eq!(offset_of!($iface, lpVtbl), 0);
                assert_eq!(size_of::<$iface>(), SLOT);
            )*
        }
    };
}

contrats_com! {
    IMiniport / IMiniportVtbl : IID_IMiniport [],
    IMiniportTopology / IMiniportTopologyVtbl : IID_IMiniportTopology [IID_IMiniport],
    IMiniportWaveRT / IMiniportWaveRTVtbl : IID_IMiniportWaveRT [IID_IMiniport],
    IMiniportWaveRTStream / IMiniportWaveRTStreamVtbl : IID_IMiniportWaveRTStream [],
    IMiniportWaveRTStreamNotification / IMiniportWaveRTStreamNotificationVtbl :
        IID_IMiniportWaveRTStreamNotification [IID_IMiniportWaveRTStream],
    IAdapterPowerManagement / IAdapterPowerManagementVtbl : IID_IAdapterPowerManagement [],
    IPort / IPortVtbl : IID_IPort [],
    IPortTopology / IPortTopologyVtbl : IID_IPortTopology [IID_IPort],
    IPortWaveRT / IPortWaveRTVtbl : IID_IPortWaveRT [IID_IPort],
    IPortWaveRTStream / IPortWaveRTStreamVtbl : IID_IPortWaveRTStream [],
    IResourceList / IResourceListVtbl : IID_IResourceList [],
    IRegistryKey / IRegistryKeyVtbl : IID_IRegistryKey [],
}

/// `IUnknown` seul : sa table ne contient que `IID_IUnknown` (pas de doublon).
#[test]
fn iunknown_repond_a_iid_iunknown_seulement() {
    assert_eq!(<IUnknownVtbl as ComVtable>::IIDS, &[IID_IUNKNOWN]);
    assert_eq!(offset_of!(IUnknownVtbl, QueryInterface), 0);
    assert_eq!(offset_of!(IUnknownVtbl, AddRef), SLOT);
    assert_eq!(offset_of!(IUnknownVtbl, Release), 2 * SLOT);
    assert_eq!(size_of::<<IUnknown as ComInterface>::Vtbl>(), 3 * SLOT);
    assert_eq!(offset_of!(IUnknown, lpVtbl), 0);
}

/// `GUID` (bindgen) et `Guid` (conduit-com) : même disposition, et `IID_IUnknown` de
/// `punknown.h` vaut bien la constante de `conduit-com` (octet `C0` en Data4[2]).
#[test]
fn guid_meme_disposition_et_iid_iunknown() {
    assert_eq!(size_of::<GUID>(), size_of::<Guid>());
    assert_eq!(align_of::<GUID>(), align_of::<Guid>());
    assert_eq!(offset_of!(GUID, Data1), offset_of!(Guid, data1));
    assert_eq!(offset_of!(GUID, Data2), offset_of!(Guid, data2));
    assert_eq!(offset_of!(GUID, Data3), offset_of!(Guid, data3));
    assert_eq!(offset_of!(GUID, Data4), offset_of!(Guid, data4));

    assert_eq!(guid(&IID_IUnknown), IID_IUNKNOWN);
    assert_eq!(IID_IUNKNOWN_PORTCLS, IID_IUNKNOWN);
    assert_eq!(
        guid(&IID_IMiniportTopology),
        Guid::from_u128(0xB4C90A31_5791_11D0_86F9_00A0C911B544)
    );

    // Lecture croisée : un `*const GUID` se lit comme un `*const Guid`.
    let g = IID_IMiniportTopology;
    let p: *const GUID = &g;
    // SAFETY: même disposition (assertions ci-dessus), `p` pointe une variable vivante.
    let lu = unsafe { *p.cast::<Guid>() };
    assert_eq!(lu, guid(&g));
}
