//! Thunks des deux slots hérités de `IMiniport` (`GetDescription`,
//! `DataRangeIntersection`), génériques sur la vtable `V` et le type implémenteur `T`.
//!
//! `IMiniportTopology` et `IMiniportWaveRT` héritent toutes deux de `IMiniport` (préfixe
//! de vtable : `IUnknown`, `GetDescription`, `DataRangeIntersection`) ; la traduction
//! `Result<u32, NtStatus>` → (`NTSTATUS`, `ResultantFormatLength`) est identique. Le
//! trait [`MiniportSlots`] est implémenté **par vtable** (`impl<T: MiniportTopology>
//! MiniportSlots<T> for IMiniportTopologyVtbl`, idem pour `IMiniportWaveRTVtbl`) : deux
//! `impl<T: …> Trait for T` couvrants seraient rejetés par la cohérence, deux `impl` sur
//! des types `Self` distincts ne le sont pas.

use core::ffi::c_void;
use core::slice;

use conduit_com::{ComObject, ComVtable, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls_sys::{
    KSDATARANGE, NTSTATUS, PCFILTER_DESCRIPTOR, PKSDATARANGE, PPCFILTER_DESCRIPTOR, PULONG, PVOID,
    ULONG,
};

use crate::status::{STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL};

/// Partie `IMiniport` d'une vtable de miniport : comment ses thunks atteignent le trait
/// de `T`.
///
/// Implémenté par [`crate::topology`] sur `IMiniportTopologyVtbl` pour tout
/// `T: MiniportTopology` et par [`crate::wavert`] sur `IMiniportWaveRTVtbl` pour tout
/// `T: MiniportWaveRT` ; les contrats sont documentés sur ces deux traits.
pub trait MiniportSlots<T: Send + Sync + 'static>: ComVtable {
    /// `GetDescription`.
    fn description(me: &T) -> &'static PCFILTER_DESCRIPTOR;

    /// `DataRangeIntersection`.
    fn data_range_intersection(
        me: &T,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus>;
}

/// `IMiniport::GetDescription`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `description`, s'il
/// n'est pas nul, pointe un emplacement de pointeur inscriptible.
pub unsafe extern "C" fn get_description<V: MiniportSlots<T>, T: Send + Sync + 'static>(
    this: *mut c_void,
    description: *mut PPCFILTER_DESCRIPTOR,
) -> NTSTATUS {
    if description.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    let desc: *const PCFILTER_DESCRIPTOR = V::description(me);
    // SAFETY: `description` est non nul et inscriptible (contrat). Le prototype C veut un
    // `PPCFILTER_DESCRIPTOR` (non `const`) mais PortCls ne fait que lire le descripteur :
    // le `cast_mut` ne donne lieu à aucune écriture.
    unsafe { *description = desc.cast_mut() };
    STATUS_SUCCESS
}

/// `IMiniport::DataRangeIntersection`.
///
/// Traduit `Result<u32, NtStatus>` (taille requise du format) en `NTSTATUS` :
/// `STATUS_SUCCESS` si le tampon a pu recevoir le format, `STATUS_BUFFER_OVERFLOW` si le
/// tampon est absent ou vide (interrogation de taille), `STATUS_BUFFER_TOO_SMALL` s'il est
/// trop petit ; la taille est toujours écrite dans `result_len` en cas de `Ok`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `data_range` et
/// `matching` pointent des `KSDATARANGE` lisibles ; `out`, s'il n'est pas nul, pointe
/// `out_len` octets inscriptibles ; `result_len` pointe un `ULONG` inscriptible.
pub unsafe extern "C" fn data_range_intersection<V: MiniportSlots<T>, T: Send + Sync + 'static>(
    this: *mut c_void,
    pin_id: ULONG,
    data_range: PKSDATARANGE,
    matching: PKSDATARANGE,
    out_len: ULONG,
    out: PVOID,
    result_len: PULONG,
) -> NTSTATUS {
    if data_range.is_null() || matching.is_null() || result_len.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let Ok(available) = usize::try_from(out_len) else {
        return STATUS_INVALID_PARAMETER;
    };
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    // SAFETY: `data_range` est non nul et pointe une `KSDATARANGE` lisible le temps de
    // l'appel (contrat).
    let client = unsafe { &*data_range };
    // SAFETY: idem pour `matching`.
    let my = unsafe { &*matching };
    let buffer: Option<&mut [u8]> = if out.is_null() {
        None
    } else {
        // SAFETY: `out` est non nul et pointe `out_len` octets inscriptibles, exclusifs
        // le temps de l'appel (contrat) ; `u8` n'a pas de contrainte d'alignement.
        Some(unsafe { slice::from_raw_parts_mut(out.cast::<u8>(), available) })
    };
    let has_buffer = buffer.is_some();

    match V::data_range_intersection(me, pin_id, client, my, buffer) {
        Ok(needed) => {
            // SAFETY: `result_len` est non nul et inscriptible (contrat).
            unsafe { *result_len = needed };
            if has_buffer && needed <= out_len {
                STATUS_SUCCESS
            } else if out_len == 0 {
                STATUS_BUFFER_OVERFLOW
            } else {
                STATUS_BUFFER_TOO_SMALL
            }
        }
        Err(status) => status,
    }
}
