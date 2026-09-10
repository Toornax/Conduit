//! Mode paquets vu par un faux PortCls : l'objet composite de `portcls::packet` est
//! appelé **uniquement à travers ses vtables** (celle du principal et les deux têtes
//! satellites), comme le noyau le ferait.
//!
//! Points vérifiés : les quatre interfaces répondent sur un même objet, avec un compteur
//! de références unique et juste après chaque interrogation ; les têtes satellites ont
//! une adresse stable, distincte du principal ; `IID_IUnknown` rend toujours le
//! principal, d'où qu'on l'interroge ; les slots métier atteignent bien le flux du
//! pilote avec leurs paramètres ; les onze slots de `IMiniportWaveRTStream` continuent de
//! fonctionner à travers la délégation de `PacketStream`.
//!
//! # Le cycle de vie, et pourquoi il a son propre test
//!
//! Un objet composite, c'est **trois pointeurs pour un compteur**. Une référence prise en
//! trop et l'objet ne meurt jamais : son `Drop` ne s'exécute pas, le flux ne se retire
//! pas de l'emplacement du câble, et la **deuxième** ouverture rend
//! `STATUS_DEVICE_BUSY` — pas la vingtième. Une référence oubliée et l'objet meurt sous
//! les pieds de PortCls. `cycle_de_vie_du_composite_revient_a_zero` rejoue donc une
//! séquence complète — ouverture, `QueryInterface` de chaque IID, fermeture — pour les
//! **quatre** configurations d'interfaces, en tenant le compte des `AddRef` et des
//! `Release` demandés, et vérifie l'invariant du cycle : `Release` = `AddRef` + 1 (la
//! référence de la construction), compteur à zéro, `Drop` exactement une fois.

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
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use common::{FAUX_MDL, This, add_ref, query_interface, refcount, release, vtbl_de};
use conduit_com::{ComRef, Guid, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    _MEMORY_CACHING_TYPE, BOOL, DWORD, GUID, IID_IMiniportWaveRT, IID_IMiniportWaveRTInputStream,
    IID_IMiniportWaveRTOutputStream, IID_IMiniportWaveRTStream,
    IID_IMiniportWaveRTStreamNotification, IMiniportTopologyVtbl, IMiniportWaveRTInputStreamVtbl,
    IMiniportWaveRTOutputStreamVtbl, IMiniportWaveRTStreamNotificationVtbl,
    IMiniportWaveRTStreamVtbl, IUnknown, KSAUDIO_POSITION, KSAUDIO_PRESENTATION_POSITION,
    KSEVENT_TYPE_ENABLE, KSPROPERTY_TYPE_GET, KSPROPSETID_Jack, KSSTATE,
    KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY, KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM, NTSTATUS,
    PCEVENT_ITEM, PCEVENT_REQUEST, PCEVENT_VERB_SUPPORT, PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM,
    PCPROPERTY_REQUEST, PKEVENT, PKSEVENT_ENTRY, PMDL, ULONG, ULONG64,
};
use portcls::{
    AudioBuffer, EventHandler, MiniportTopology, MiniportWaveRTInputStream,
    MiniportWaveRTOutputStream, MiniportWaveRTStream, MiniportWaveRTStreamNotification,
    PacketInterfaces, PacketStreamVtbl, PortTopology, PropertyHandler, ReadPacket, ResourceList,
    STATUS_DATA_LATE_ERROR, STATUS_DATA_OVERRUN, STATUS_DEVICE_NOT_READY,
    STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_SUPPORTED, StreamObject, event,
    new_packet_stream_object, new_topology_object, property, try_new_packet_stream_object,
};

const SLOT: usize = 8;

// ---------------------------------------------------------------------------------
// Le flux sous test : sert le mode paquets et compte ses appels.
// ---------------------------------------------------------------------------------

struct Flux {
    state: AtomicU32,
    /// Dernier `(packet_number, flags, eos_packet_length)` de `SetWritePacket`.
    dernier_write: AtomicU64,
    lectures: AtomicU32,
    dropped: Arc<AtomicUsize>,
}

impl Flux {
    fn nouveau() -> (Self, Arc<AtomicUsize>) {
        let dropped = Arc::new(AtomicUsize::new(0));
        (
            Self {
                state: AtomicU32::new(u32::MAX),
                dernier_write: AtomicU64::new(0),
                lectures: AtomicU32::new(0),
                dropped: Arc::clone(&dropped),
            },
            dropped,
        )
    }
}

impl Drop for Flux {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniportWaveRTStream for Flux {
    fn set_state(&self, state: KSSTATE::Type) -> NtStatus {
        self.state.store(state as u32, Ordering::SeqCst);
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(4321)
    }

    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        Ok(AudioBuffer {
            mdl: FAUX_MDL as PMDL,
            actual_bytes: requested_bytes,
            offset_from_first_page: 0,
            cache_type: _MEMORY_CACHING_TYPE::MmCached,
        })
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for Flux {
    fn allocate_buffer_with_notification(
        &self,
        _: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        self.allocate_audio_buffer(requested_bytes)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for Flux {
    fn read_packet(&self) -> Result<ReadPacket, NtStatus> {
        let n = self.lectures.fetch_add(1, Ordering::SeqCst);
        Ok(ReadPacket {
            packet_number: n,
            flags: 0x8000,
            performance_counter: 0x0123_4567_89AB_CDEF,
            more_data: n % 2 == 0,
        })
    }
}

impl MiniportWaveRTOutputStream for Flux {
    fn set_write_packet(&self, packet_number: u32, flags: u32, eos_packet_length: u32) -> NtStatus {
        self.dernier_write.store(
            (u64::from(packet_number) << 40)
                | (u64::from(flags) << 20)
                | u64::from(eos_packet_length),
            Ordering::SeqCst,
        );
        STATUS_SUCCESS
    }

    fn presentation_position(&self) -> Result<KSAUDIO_PRESENTATION_POSITION, NtStatus> {
        Ok(KSAUDIO_PRESENTATION_POSITION {
            u64PositionInBlocks: 77,
            u64QPCPosition: 88,
        })
    }

    fn packet_count(&self) -> Result<u32, NtStatus> {
        Ok(9)
    }
}

/// Flux qui laisse tous les défauts du mode paquets (refus).
struct FluxMuet;

impl MiniportWaveRTStream for FluxMuet {
    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(0)
    }

    fn allocate_audio_buffer(&self, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for FluxMuet {
    fn allocate_buffer_with_notification(&self, _: u32, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for FluxMuet {}
impl MiniportWaveRTOutputStream for FluxMuet {}

// ---------------------------------------------------------------------------------
// Appels de slots, à la manière de PortCls.
// ---------------------------------------------------------------------------------

fn iid_entree() -> Guid {
    guid(&IID_IMiniportWaveRTInputStream)
}

fn iid_sortie() -> Guid {
    guid(&IID_IMiniportWaveRTOutputStream)
}

/// `QueryInterface` qui doit réussir : rend le pointeur et laisse la référence prise.
fn qi_ok(this: This, iid: &Guid) -> This {
    let (status, out) = query_interface(this, iid);
    assert_eq!(status, STATUS_SUCCESS, "{iid}");
    assert!(!out.is_null(), "{iid}");
    out
}

fn appel_get_read_packet(this: This) -> (i32, u32, u32, u64, i32) {
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(this) };
    let mut numero: ULONG = 0xDEAD;
    let mut flags: DWORD = 0xDEAD;
    let mut qpc: ULONG64 = 0xDEAD;
    let mut suite: BOOL = 0xDEAD;
    let status =
        unsafe { vt.GetReadPacket.unwrap()(this, &mut numero, &mut flags, &mut qpc, &mut suite) };
    (status, numero, flags, qpc, suite)
}

fn vt_sortie(this: This) -> &'static IMiniportWaveRTOutputStreamVtbl {
    unsafe { vtbl_de::<IMiniportWaveRTOutputStreamVtbl>(this) }
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

/// Les quatre interfaces sur un seul objet, avec un compteur de références unique : chaque
/// interrogation réussie ajoute exactement une référence, chaque `Release` en rend une, et
/// le dernier détruit le flux.
#[test]
fn quatre_interfaces_sur_un_meme_objet() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    assert_eq!(refcount(this), 1);

    // Les trois interfaces « classiques » rendent le principal lui-même.
    let mut prises = Vec::new();
    for (i, iid) in [
        IID_IUNKNOWN,
        guid(&IID_IMiniportWaveRTStream),
        guid(&IID_IMiniportWaveRTStreamNotification),
    ]
    .into_iter()
    .enumerate()
    {
        let out = qi_ok(this, &iid);
        assert_eq!(out, this, "{iid} : le principal répond pour lui-même");
        assert_eq!(refcount(this), 2 + i as u32, "une référence de plus");
        prises.push(out);
    }

    // Les deux interfaces de paquets rendent des têtes satellites, distinctes entre elles
    // et du principal, mais comptées sur le même objet.
    let entree = qi_ok(this, &iid_entree());
    assert_eq!(refcount(this), 5);
    let sortie = qi_ok(this, &iid_sortie());
    assert_eq!(refcount(this), 6);
    assert_ne!(entree, this);
    assert_ne!(sortie, this);
    assert_ne!(entree, sortie);
    // Les têtes sont dans l'allocation du principal, dans l'ordre `input`, `output`.
    assert!(entree > this && sortie > entree);
    // Adresses stables : deux interrogations rendent la même tête.
    let entree2 = qi_ok(this, &iid_entree());
    assert_eq!(entree2, entree);
    assert_eq!(refcount(this), 7);
    release(entree2);

    // Un IID inconnu ne passe pas et ne compte rien.
    let (status, out) = query_interface(this, &guid(&IID_IMiniportWaveRT));
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());
    assert_eq!(refcount(this), 6);

    // Rendu des six références : rien n'est détruit avant la dernière.
    for prise in prises {
        release(prise);
    }
    assert_eq!(release(entree), 2, "le satellite décompte sur le principal");
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(sortie), 1);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Le sens du flux filtre les interfaces exposées : un flux de rendu ne répond qu'à
/// `IMiniportWaveRTOutputStream`, un flux de capture qu'à `IMiniportWaveRTInputStream`.
#[test]
fn le_sens_du_flux_filtre_les_interfaces() {
    for (interfaces, attendue, refusee) in [
        (PacketInterfaces::Output, iid_sortie(), iid_entree()),
        (PacketInterfaces::Input, iid_entree(), iid_sortie()),
    ] {
        let (flux, dropped) = Flux::nouveau();
        let this = new_packet_stream_object(flux, interfaces).into_raw();

        let out = qi_ok(this, &attendue);
        assert_ne!(out, this);
        assert_eq!(refcount(this), 2);
        release(out);

        let (status, out) = query_interface(this, &refusee);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{interfaces:?}");
        assert!(out.is_null(), "{interfaces:?}");
        assert_eq!(refcount(this), 1, "un refus ne compte pas de référence");

        assert_eq!(release(this), 0);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
    assert!(PacketInterfaces::Both.has_input() && PacketInterfaces::Both.has_output());
    assert!(!PacketInterfaces::Input.has_output());
    assert!(!PacketInterfaces::Output.has_input());
}

/// `IID_IUnknown` interrogé depuis une tête satellite rend **le principal** (identité COM),
/// et l'on passe d'une tête à l'autre.
#[test]
fn identite_et_transitivite_depuis_un_satellite() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let unknown = qi_ok(entree, &IID_IUNKNOWN);
    assert_eq!(unknown, this, "IID_IUnknown est toujours le principal");
    let notification = qi_ok(entree, &guid(&IID_IMiniportWaveRTStreamNotification));
    assert_eq!(notification, this);
    let sortie = qi_ok(entree, &iid_sortie());
    assert_ne!(sortie, entree);
    // 1 (initiale) + entrée + unknown + notification + sortie.
    assert_eq!(refcount(this), 5);

    // `AddRef` sur un satellite compte sur le principal.
    assert_eq!(add_ref(sortie), 6);
    assert_eq!(refcount(this), 6);
    release(sortie);

    let (status, out) = query_interface(entree, &guid(&IID_IMiniportWaveRT));
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());

    release(unknown);
    release(notification);
    release(sortie);
    release(entree);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Le dernier `Release` peut venir d'une tête satellite : il détruit l'objet entier.
#[test]
fn le_dernier_release_peut_venir_d_un_satellite() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Input).into_raw();
    let entree = qi_ok(this, &iid_entree());
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(entree), 0, "le satellite libère l'objet");
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Slots métier de `IMiniportWaveRTInputStream`, appelés par la vtable de la tête.
#[test]
fn get_read_packet_par_la_vtable() {
    let (flux, _dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Input).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let (status, numero, flags, qpc, suite) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        (numero, flags, qpc, suite),
        (0, 0x8000, 0x0123_4567_89AB_CDEF, 1)
    );
    let (status, numero, _, _, suite) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        (numero, suite),
        (1, 0),
        "MoreData = FALSE au deuxième appel"
    );

    // Sortie nulle : refus sans appeler le trait.
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(entree) };
    let mut flags: DWORD = 0;
    let mut qpc: ULONG64 = 0;
    let mut plus: BOOL = 0;
    let status = unsafe {
        vt.GetReadPacket.unwrap()(entree, ptr::null_mut(), &mut flags, &mut qpc, &mut plus)
    };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    let (status, ..) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);

    release(entree);
    assert_eq!(release(this), 0);
}

/// Slots métier de `IMiniportWaveRTOutputStream`, appelés par la vtable de la tête.
#[test]
fn slots_de_sortie_par_la_vtable() {
    let (flux, _dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::Output);
    let this = objet.into_raw();
    let sortie = qi_ok(this, &iid_sortie());
    let vt = vt_sortie(sortie);

    assert_eq!(
        unsafe { vt.SetWritePacket.unwrap()(sortie, 12, 0x2, 480) },
        STATUS_SUCCESS
    );

    let mut position = KSAUDIO_PRESENTATION_POSITION {
        u64PositionInBlocks: 0,
        u64QPCPosition: 0,
    };
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, &mut position) },
        STATUS_SUCCESS
    );
    assert_eq!(
        (position.u64PositionInBlocks, position.u64QPCPosition),
        (77, 88)
    );
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );

    let mut compte: ULONG = 0xDEAD;
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
        STATUS_SUCCESS
    );
    assert_eq!(compte, 9);
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );

    release(sortie);
    assert_eq!(release(this), 0);
}

/// Les défauts des deux traits refusent (`STATUS_NOT_SUPPORTED`) sans rien écrire.
#[test]
fn defauts_du_mode_paquets_refusent() {
    let this = try_new_packet_stream_object(FluxMuet, PacketInterfaces::Both)
        .unwrap()
        .into_raw();
    let entree = qi_ok(this, &iid_entree());
    let sortie = qi_ok(this, &iid_sortie());

    let (status, numero, ..) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_NOT_SUPPORTED);
    assert_eq!(numero, 0xDEAD, "sortie non touchée");

    let vt = vt_sortie(sortie);
    assert_eq!(
        unsafe { vt.SetWritePacket.unwrap()(sortie, 1, 0, 0) },
        STATUS_NOT_SUPPORTED
    );
    let mut compte: ULONG = 0xDEAD;
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
        STATUS_NOT_SUPPORTED
    );
    assert_eq!(compte, 0xDEAD);
    let mut position = KSAUDIO_PRESENTATION_POSITION {
        u64PositionInBlocks: 5,
        u64QPCPosition: 5,
    };
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, &mut position) },
        STATUS_NOT_SUPPORTED
    );
    assert_eq!(position.u64PositionInBlocks, 5);

    release(entree);
    release(sortie);
    assert_eq!(release(this), 0);
}

/// Un flux qui **sert** le mode paquets et rend, chaque fois, le statut qu'on lui a dicté :
/// c'est ce que `conduit_kmd::stream::WaveStream` fait depuis le lot 3.
struct FluxServant {
    /// Statut à rendre par `SetWritePacket`.
    write: NtStatus,
    /// Statut à rendre par `GetReadPacket` (`STATUS_SUCCESS` : un paquet est rendu).
    read: NtStatus,
    /// Statut à rendre par `GetPacketCount`.
    count: NtStatus,
}

impl MiniportWaveRTStream for FluxServant {
    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(0)
    }

    fn allocate_audio_buffer(&self, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for FluxServant {
    fn allocate_buffer_with_notification(&self, _: u32, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for FluxServant {
    fn read_packet(&self) -> Result<ReadPacket, NtStatus> {
        if self.read != STATUS_SUCCESS {
            return Err(self.read);
        }
        Ok(ReadPacket {
            packet_number: 41,
            // Le trou de capture, tel que le pilote le reporte : c'est la seule information
            // de discontinuité que la signature générée laisse passer.
            flags: KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY,
            performance_counter: 0x1234_5678,
            more_data: false,
        })
    }
}

impl MiniportWaveRTOutputStream for FluxServant {
    fn set_write_packet(&self, _: u32, _: u32, _: u32) -> NtStatus {
        self.write
    }

    fn presentation_position(&self) -> Result<KSAUDIO_PRESENTATION_POSITION, NtStatus> {
        Ok(KSAUDIO_PRESENTATION_POSITION {
            u64PositionInBlocks: 96_000,
            u64QPCPosition: 0xFEDC_BA98,
        })
    }

    fn packet_count(&self) -> Result<u32, NtStatus> {
        if self.count != STATUS_SUCCESS {
            return Err(self.count);
        }
        Ok(200)
    }
}

/// **Les statuts du lot 3 traversent les thunks tels quels**, et une sortie n'est écrite
/// que sur un succès.
///
/// Les trois codes que le contrat documente — `STATUS_DATA_LATE_ERROR`,
/// `STATUS_DATA_OVERRUN`, `STATUS_DEVICE_NOT_READY` — ne sont pas des erreurs génériques :
/// le moteur audio les traite, et se recale par `GetPacketCount` après les deux premiers.
/// Un thunk qui les écraserait en `STATUS_UNSUCCESSFUL`, ou qui écrirait ses sorties avant
/// de les rendre, ferait lire au client un numéro de paquet qui n'a jamais existé.
#[test]
fn les_statuts_du_mode_paquets_servi_traversent_les_thunks() {
    // 1. Le cas servi : les quatre méthodes réussissent et écrivent leurs sorties.
    let servant = FluxServant {
        write: STATUS_SUCCESS,
        read: STATUS_SUCCESS,
        count: STATUS_SUCCESS,
    };
    let this = new_packet_stream_object(servant, PacketInterfaces::Both).into_raw();
    let entree = qi_ok(this, &iid_entree());
    let sortie = qi_ok(this, &iid_sortie());
    let vt = vt_sortie(sortie);

    let (status, numero, flags, qpc, suite) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        (numero, flags, qpc, suite),
        (
            41,
            KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY,
            0x1234_5678,
            0
        ),
        "le trou de capture et l'horodatage arrivent intacts"
    );
    assert_eq!(
        unsafe { vt.SetWritePacket.unwrap()(sortie, 7, KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM, 240) },
        STATUS_SUCCESS
    );
    let mut position = KSAUDIO_PRESENTATION_POSITION {
        u64PositionInBlocks: 0,
        u64QPCPosition: 0,
    };
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, &mut position) },
        STATUS_SUCCESS
    );
    assert_eq!(
        (position.u64PositionInBlocks, position.u64QPCPosition),
        (96_000, 0xFEDC_BA98),
        "des trames absolues et un QPC brut, pas des octets ni des unités de 100 ns"
    );
    let mut compte: ULONG = 0xDEAD;
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
        STATUS_SUCCESS
    );
    assert_eq!(compte, 200);
    release(entree);
    release(sortie);
    assert_eq!(release(this), 0);

    // 2. Les trois refus documentés : rendus tels quels, sans toucher aux sorties.
    for (write, read, count) in [
        (
            STATUS_DATA_LATE_ERROR,
            STATUS_DEVICE_NOT_READY,
            STATUS_DEVICE_NOT_READY,
        ),
        (
            STATUS_DATA_OVERRUN,
            STATUS_DEVICE_NOT_READY,
            STATUS_DEVICE_NOT_READY,
        ),
        (
            STATUS_INVALID_PARAMETER,
            STATUS_DEVICE_NOT_READY,
            STATUS_DEVICE_NOT_READY,
        ),
    ] {
        let this =
            new_packet_stream_object(FluxServant { write, read, count }, PacketInterfaces::Both)
                .into_raw();
        let entree = qi_ok(this, &iid_entree());
        let sortie = qi_ok(this, &iid_sortie());
        let vt = vt_sortie(sortie);

        let (status, numero, ..) = appel_get_read_packet(entree);
        assert_eq!(status, read);
        assert_eq!(numero, 0xDEAD, "sortie non touchée par un refus");
        assert_eq!(
            unsafe { vt.SetWritePacket.unwrap()(sortie, 3, 0, 0) },
            write
        );
        let mut compte: ULONG = 0xDEAD;
        assert_eq!(
            unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
            count
        );
        assert_eq!(compte, 0xDEAD, "sortie non touchée par un refus");

        release(entree);
        release(sortie);
        assert_eq!(release(this), 0);
    }
}

/// La délégation de `PacketStream` laisse intacts les quinze slots de flux du principal :
/// ils atteignent le flux du pilote comme sur un objet non composite.
#[test]
fn les_slots_de_flux_traversent_le_composite() {
    let (flux, _dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::Output);
    // `Deref` : `ComPtr` → `PacketStream<Flux>` → `Flux`.
    assert_eq!(objet.interfaces(), PacketInterfaces::Output);
    assert_eq!(objet.get().lectures.load(Ordering::SeqCst), 0);
    let this = objet.into_raw();

    let vt = unsafe { vtbl_de::<IMiniportWaveRTStreamVtbl>(this) };
    assert_eq!(
        unsafe { vt.SetState.unwrap()(this, KSSTATE::KSSTATE_RUN) },
        STATUS_SUCCESS
    );
    let mut position = KSAUDIO_POSITION {
        PlayOffset: 0,
        WriteOffset: 0,
    };
    assert_eq!(
        unsafe { vt.GetPosition.unwrap()(this, &mut position) },
        STATUS_SUCCESS
    );
    assert_eq!((position.PlayOffset, position.WriteOffset), (4321, 4321));

    let mut mdl: PMDL = ptr::null_mut();
    let mut actual: ULONG = 0;
    let mut offset: ULONG = 0xDEAD;
    let mut cache = -7;
    let vt_notif = unsafe { vtbl_de::<IMiniportWaveRTStreamNotificationVtbl>(this) };
    let status = unsafe {
        vt_notif.AllocateBufferWithNotification.unwrap()(
            this,
            2,
            960,
            &mut mdl,
            &mut actual,
            &mut offset,
            &mut cache,
        )
    };
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!((mdl as usize, actual, offset), (FAUX_MDL, 960, 0));
    unsafe { vt_notif.FreeBufferWithNotification.unwrap()(this, mdl, actual) };

    assert_eq!(release(this), 0);
}

/// Ordre et remplissage des slots des deux vtables satellites, sur les vtables
/// **construites par le crate** : mêmes offsets que `portcls-sys/tests/vtables.rs`.
#[test]
fn ordre_des_slots_des_vtables_satellites() {
    let entree = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTInputStreamVtbl, QueryInterface),
        ),
        ("AddRef", offset_of!(IMiniportWaveRTInputStreamVtbl, AddRef)),
        (
            "Release",
            offset_of!(IMiniportWaveRTInputStreamVtbl, Release),
        ),
        (
            "GetReadPacket",
            offset_of!(IMiniportWaveRTInputStreamVtbl, GetReadPacket),
        ),
    ];
    for (i, (nom, offset)) in entree.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }
    let sortie = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, QueryInterface),
        ),
        (
            "AddRef",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, AddRef),
        ),
        (
            "Release",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, Release),
        ),
        (
            "SetWritePacket",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, SetWritePacket),
        ),
        (
            "GetOutputStreamPresentationPosition",
            offset_of!(
                IMiniportWaveRTOutputStreamVtbl,
                GetOutputStreamPresentationPosition
            ),
        ),
        (
            "GetPacketCount",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, GetPacketCount),
        ),
    ];
    for (i, (nom, offset)) in sortie.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }

    let e = &<Flux as PacketStreamVtbl>::INPUT_VTBL;
    assert!(
        e.QueryInterface.is_some()
            && e.AddRef.is_some()
            && e.Release.is_some()
            && e.GetReadPacket.is_some()
    );
    let s = &<Flux as PacketStreamVtbl>::OUTPUT_VTBL;
    assert!(
        s.QueryInterface.is_some()
            && s.AddRef.is_some()
            && s.Release.is_some()
            && s.SetWritePacket.is_some()
            && s.GetOutputStreamPresentationPosition.is_some()
            && s.GetPacketCount.is_some()
    );
    let p = &<Flux as PacketStreamVtbl>::VTBL;
    assert!(p.QueryInterface.is_some() && p.UnregisterNotificationEvent.is_some());

    // Deux objets du même type partagent leurs trois vtables (constantes promues).
    let (f1, _) = Flux::nouveau();
    let (f2, _) = Flux::nouveau();
    let o1 = new_packet_stream_object(f1, PacketInterfaces::Both);
    let o2 = try_new_packet_stream_object(f2, PacketInterfaces::Both).unwrap();
    assert!(ptr::eq(o1.object().vtbl(), o2.object().vtbl()));
    let t1 = qi_ok(o1.as_raw(), &iid_entree());
    let t2 = qi_ok(o2.as_raw(), &iid_entree());
    let v1 = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(t1) };
    let v2 = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(t2) };
    assert!(ptr::eq(v1, v2));
    release(t1);
    release(t2);
}

/// `QueryInterface` avec des arguments dégénérés, sur le principal comme sur un satellite.
#[test]
fn query_interface_arguments_degeneres() {
    let (flux, _dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let vt = unsafe { vtbl_de::<IMiniportWaveRTStreamNotificationVtbl>(this) };
    let iid = iid_entree();
    // `out` nul : refus, rien n'est écrit ni compté.
    assert_eq!(
        unsafe {
            vt.QueryInterface.unwrap()(
                this,
                ptr::from_ref(&iid).cast(),
                ptr::null_mut::<*mut core::ffi::c_void>().cast(),
            )
        },
        STATUS_INVALID_PARAMETER
    );
    // `iid` nul : refus, `*out` mis à nul.
    let mut out: *mut core::ffi::c_void = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    assert_eq!(
        unsafe { vt.QueryInterface.unwrap()(this, ptr::null(), (&raw mut out).cast()) },
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    assert_eq!(refcount(this), 2);

    // Mêmes réponses depuis un satellite.
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(entree) };
    let mut out: *mut core::ffi::c_void = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    assert_eq!(
        unsafe { vt.QueryInterface.unwrap()(entree, ptr::null(), (&raw mut out).cast()) },
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    assert_eq!(refcount(this), 2);

    release(entree);
    assert_eq!(release(this), 0);
}

// ---------------------------------------------------------------------------------
// Cycle de vie : trois pointeurs, un compteur.
// ---------------------------------------------------------------------------------

/// Comptabilité des `AddRef` et des `Release` **demandés par le test** à travers les
/// vtables — y compris celui qu'un `QueryInterface` réussi prend pour l'appelant.
///
/// L'invariant du cycle de vie en découle : sur un objet qui naît avec une référence et
/// meurt à zéro, `releases == add_refs + 1`.
#[derive(Default)]
struct Comptes {
    add_refs: u32,
    releases: u32,
}

impl Comptes {
    fn add_ref(&mut self, this: This) -> ULONG {
        self.add_refs += 1;
        add_ref(this)
    }

    fn release(&mut self, this: This) -> ULONG {
        self.releases += 1;
        release(this)
    }

    /// Compte courant, observé par un `AddRef`/`Release` équilibré.
    fn refcount(&mut self, this: This) -> ULONG {
        self.add_refs += 1;
        self.releases += 1;
        refcount(this)
    }

    /// `QueryInterface` : un succès rend un pointeur **déjà compté**, c'est donc un
    /// `AddRef` de plus au passif du test.
    fn qi(&mut self, this: This, iid: &Guid) -> (NtStatus, This) {
        let (status, out) = query_interface(this, iid);
        if status == STATUS_SUCCESS {
            self.add_refs += 1;
        }
        (status, out)
    }
}

/// Les six IID qu'un flux composite peut se voir demander : les trois qu'il porte
/// toujours, les deux du mode paquets, et un sixième qui n'est pas le sien.
fn iids_interrogeables(interfaces: PacketInterfaces) -> [(&'static str, Guid, bool); 6] {
    [
        ("IUnknown", IID_IUNKNOWN, true),
        (
            "IMiniportWaveRTStream",
            guid(&IID_IMiniportWaveRTStream),
            true,
        ),
        (
            "IMiniportWaveRTStreamNotification",
            guid(&IID_IMiniportWaveRTStreamNotification),
            true,
        ),
        (
            "IMiniportWaveRTInputStream",
            guid(&IID_IMiniportWaveRTInputStream),
            interfaces.has_input(),
        ),
        (
            "IMiniportWaveRTOutputStream",
            guid(&IID_IMiniportWaveRTOutputStream),
            interfaces.has_output(),
        ),
        ("IMiniportWaveRT", guid(&IID_IMiniportWaveRT), false),
    ]
}

/// Ouverture, `QueryInterface` de chaque IID, puis fermeture, pour les quatre
/// configurations d'interfaces : le compteur revient à **zéro**, `Drop` est appelé
/// **exactement une fois**, et le nombre de `Release` vaut celui des `AddRef` plus la
/// référence de construction.
///
/// C'est le test de non-régression du lot : une référence en trop laisserait le flux
/// vivant, donc l'emplacement du câble occupé, donc la **deuxième** ouverture en
/// `STATUS_DEVICE_BUSY` ; une référence en moins détruirait l'objet sous PortCls.
#[test]
fn cycle_de_vie_du_composite_revient_a_zero() {
    for interfaces in [
        PacketInterfaces::None,
        PacketInterfaces::Input,
        PacketInterfaces::Output,
        PacketInterfaces::Both,
    ] {
        let mut c = Comptes::default();
        let (flux, dropped) = Flux::nouveau();
        let this = new_packet_stream_object(flux, interfaces).into_raw();

        // Ouverture : une seule référence, celle de la construction. Elle ne vient
        // d'aucun `AddRef`, d'où le « + 1 » de l'invariant final.
        let mut attendu: ULONG = 1;
        assert_eq!(c.refcount(this), attendu, "{interfaces:?} à la création");
        assert_eq!(dropped.load(Ordering::SeqCst), 0);

        // PortCls interroge les IID un par un. Un succès prend exactement une référence,
        // un refus n'en prend aucune et laisse `*out` nul.
        let mut prises: Vec<(&'static str, This)> = Vec::new();
        for (nom, iid, expose) in iids_interrogeables(interfaces) {
            let (status, out) = c.qi(this, &iid);
            if expose {
                assert_eq!(status, STATUS_SUCCESS, "{nom} sur {interfaces:?}");
                assert!(!out.is_null(), "{nom} sur {interfaces:?}");
                attendu += 1;
                prises.push((nom, out));
            } else {
                assert_eq!(
                    status, STATUS_INVALID_PARAMETER,
                    "{nom} n'est pas exposée par {interfaces:?}"
                );
                assert!(out.is_null(), "{nom} sur {interfaces:?}");
            }
            assert_eq!(c.refcount(this), attendu, "après {nom} sur {interfaces:?}");
        }

        // Trois interfaces toujours, plus celles du mode paquets exposées : avec
        // `None`, l'objet répond exactement comme un flux ordinaire.
        let attendues =
            3 + usize::from(interfaces.has_input()) + usize::from(interfaces.has_output());
        assert_eq!(prises.len(), attendues, "{interfaces:?}");
        assert_eq!(interfaces.is_none(), attendues == 3, "{interfaces:?}");

        // Chaque pointeur obtenu est un `IUnknown` complet : `AddRef` puis `Release` s'y
        // équilibrent, quelle que soit la tête d'où l'appel part.
        for (nom, prise) in &prises {
            assert_eq!(c.add_ref(*prise), attendu + 1, "AddRef par {nom}");
            assert_eq!(c.release(*prise), attendu, "Release par {nom}");
        }

        // Fermeture : les références rendues dans l'ordre inverse de leur prise. Rien
        // n'est détruit tant qu'il en reste une.
        while let Some((nom, prise)) = prises.pop() {
            attendu -= 1;
            assert_eq!(c.release(prise), attendu, "Release de {nom}");
            assert_eq!(dropped.load(Ordering::SeqCst), 0, "vivant après {nom}");
        }

        // La dernière : le compteur tombe à zéro et le flux du pilote est détruit une
        // fois — c'est ce `Drop` qui libère l'emplacement du câble.
        assert_eq!(c.release(this), 0, "{interfaces:?}");
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "{interfaces:?}");
        assert_eq!(
            c.releases,
            c.add_refs + 1,
            "{interfaces:?} : {} Release pour {} AddRef et la référence de construction",
            c.releases,
            c.add_refs
        );
    }
}

/// Le dernier `Release` venu d'une tête satellite détruit l'objet une seule fois, et
/// l'ordre de rendu des références n'y change rien.
#[test]
fn le_drop_n_a_lieu_qu_une_fois_quel_que_soit_l_ordre() {
    // Ordres de fermeture : le principal en dernier, un satellite en dernier, l'autre.
    for ordre in [[0_usize, 1, 2], [2, 0, 1], [1, 2, 0]] {
        let mut c = Comptes::default();
        let (flux, dropped) = Flux::nouveau();
        let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
        let (_, entree) = c.qi(this, &iid_entree());
        let (_, sortie) = c.qi(this, &iid_sortie());
        let pointeurs = [this, entree, sortie];

        assert_eq!(c.refcount(this), 3, "ordre {ordre:?}");
        for (rang, i) in ordre.into_iter().enumerate() {
            let reste = 2 - rang as ULONG;
            assert_eq!(
                c.release(pointeurs[i]),
                reste,
                "ordre {ordre:?}, rang {rang}"
            );
            assert_eq!(
                dropped.load(Ordering::SeqCst),
                usize::from(reste == 0),
                "ordre {ordre:?}, rang {rang}"
            );
        }
        assert_eq!(c.releases, c.add_refs + 1, "ordre {ordre:?}");
    }
}

/// Avec [`PacketInterfaces::None`], le `QueryInterface` du composite répond **exactement**
/// comme celui d'un flux non composite : les deux IID du mode paquets sont refusés au même
/// titre qu'un IID étranger, et les têtes satellites — bien présentes dans l'allocation —
/// ne sont rendues à personne.
///
/// C'est la configuration que `conduit-kmd::wave::open_stream` construit : le composite y
/// doit être **indiscernable** de l'ancien objet de flux, faute de quoi le moteur audio
/// pourrait basculer sur un chemin que le pilote ne sert pas.
#[test]
fn sans_interface_exposee_le_composite_est_indiscernable_d_un_flux_ordinaire() {
    let (flux, dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::None);
    assert!(objet.interfaces().is_none());
    assert!(!objet.interfaces().has_input() && !objet.interfaces().has_output());
    let this = objet.into_raw();

    // Les deux IID du mode paquets sont refusés comme n'importe quel IID inconnu, et le
    // flux du pilote — qui les sert pourtant — n'est jamais atteint.
    for (nom, iid) in [
        ("IMiniportWaveRTInputStream", iid_entree()),
        ("IMiniportWaveRTOutputStream", iid_sortie()),
        ("IMiniportWaveRT", guid(&IID_IMiniportWaveRT)),
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{nom}");
        assert!(out.is_null(), "{nom}");
        assert_eq!(refcount(this), 1, "un refus ne compte rien ({nom})");
    }

    // Les trois interfaces d'un flux ordinaire répondent, elles, et rendent le principal.
    for iid in [
        IID_IUNKNOWN,
        guid(&IID_IMiniportWaveRTStream),
        guid(&IID_IMiniportWaveRTStreamNotification),
    ] {
        let out = qi_ok(this, &iid);
        assert_eq!(out, this, "{iid}");
        release(out);
    }

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Le composite se transfère à PortCls par [`StreamObject`] — le chemin exact de
/// `NewStream` — sans une référence perdue ni une de trop.
#[test]
fn le_composite_se_transfere_par_stream_object() {
    let (flux, dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::None);
    // `StreamObject::from` accepte la vtable du flux à notifications, dont la vtable
    // composite est une variante (seul le slot 0 change) : rien à changer dans `wavert`.
    let stream = StreamObject::from(objet);
    let this: This = stream.into_raw().cast();

    // L'objet cédé est un `IMiniportWaveRTStream` en règle, avec sa référence unique.
    assert_eq!(refcount(this), 1);
    let out = qi_ok(this, &guid(&IID_IMiniportWaveRTStream));
    assert_eq!(out, this);
    assert_eq!(release(out), 1);

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------------
// Le composite face aux gardes de vtable de `property` et d'`event`.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` n'est fait que de pointeurs, tous nuls ici et jamais
/// déréférencés : `Sync` déclaré à la main pour la `static`.
struct SyncDesc(PCFILTER_DESCRIPTOR);
// SAFETY: la valeur est entièrement nulle et personne ne la lit.
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Un miniport topologie quelconque : ce qui compte est que sa vtable soit celle
/// qu'attendent les deux gardes, et que ce ne soit **pas** celle d'un flux.
struct CibleTopo;

impl MiniportTopology for CibleTopo {
    fn init(&self, _: Option<ComRef<IUnknown>>, _: ResourceList, _: PortTopology) -> NtStatus {
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }
}

/// Gestionnaire tout en défauts : il rend `STATUS_NOT_SUPPORTED`, ce qui suffit à
/// distinguer « la garde a laissé passer » de « la garde a refusé ».
struct Muet;

impl PropertyHandler<CibleTopo> for Muet {}
impl EventHandler<CibleTopo> for Muet {}

struct SyncPropItem(PCPROPERTY_ITEM);
// SAFETY: un `&'static GUID` et un pointeur de fonction, tous deux `'static`.
unsafe impl Sync for SyncPropItem {}

struct SyncEventItem(PCEVENT_ITEM);
// SAFETY: idem `SyncPropItem`.
unsafe impl Sync for SyncEventItem {}

/// Un jeu quelconque : les deux thunks refusent sur la vtable bien avant de le lire.
static SET_TEST: GUID = KSPROPSETID_Jack;

static ITEM_PROP: SyncPropItem = SyncPropItem(property::item::<
    IMiniportTopologyVtbl,
    CibleTopo,
    Muet,
>(&SET_TEST, 0, KSPROPERTY_TYPE_GET));

static ITEM_EVENT: SyncEventItem = SyncEventItem(event::item::<
    IMiniportTopologyVtbl,
    CibleTopo,
    Muet,
>(&SET_TEST, 0, KSEVENT_TYPE_ENABLE));

/// Le `Handler` de l'entrée de propriété, appelé comme PortCls le fait, avec `this` pour
/// `MajorTarget`.
fn appeler_propriete(this: This) -> NTSTATUS {
    let mut req = PCPROPERTY_REQUEST {
        MajorTarget: this.cast(),
        MinorTarget: ptr::null_mut(),
        Node: ULONG::MAX,
        PropertyItem: &ITEM_PROP.0,
        Verb: KSPROPERTY_TYPE_GET,
        InstanceSize: 0,
        Instance: ptr::null_mut(),
        ValueSize: 0,
        Value: ptr::null_mut(),
        Irp: ptr::null_mut(),
    };
    let slot = ITEM_PROP.0.Handler.expect("Handler de property::item");
    unsafe { slot(&mut req) }
}

/// Idem pour l'entrée d'événement.
fn appeler_evenement(this: This) -> NTSTATUS {
    let mut req = PCEVENT_REQUEST {
        MajorTarget: this.cast(),
        MinorTarget: ptr::null_mut(),
        Node: ULONG::MAX,
        EventItem: &ITEM_EVENT.0,
        EventEntry: ptr::null_mut::<PKSEVENT_ENTRY>().cast(),
        Verb: PCEVENT_VERB_SUPPORT,
        Irp: ptr::null_mut(),
    };
    let slot = ITEM_EVENT.0.Handler.expect("Handler de event::item");
    unsafe { slot(&mut req) }
}

/// Les gardes de vtable de `property` et d'`event` comparent `MajorTarget`, qui est
/// toujours un **miniport** — jamais un flux. Un objet composite, principal comme
/// satellite, y est donc refusé (`STATUS_INVALID_DEVICE_REQUEST`) : le rapport de
/// conception s'en trouve vérifié plutôt que supposé.
///
/// Le repère importe autant que le refus : sur un vrai miniport topologie, les deux
/// thunks passent la garde et atteignent le gestionnaire (`STATUS_NOT_SUPPORTED`). Sans
/// lui, le test réussirait tout aussi bien si les gardes refusaient tout.
#[test]
fn un_flux_composite_n_est_ni_une_cible_de_propriete_ni_une_cible_d_evenement() {
    let topo = new_topology_object(CibleTopo).into_raw();
    assert_eq!(
        appeler_propriete(topo),
        STATUS_NOT_SUPPORTED,
        "repère : un vrai miniport passe la garde de property"
    );
    assert_eq!(
        appeler_evenement(topo),
        STATUS_NOT_SUPPORTED,
        "repère : un vrai miniport passe la garde d'event"
    );
    assert_eq!(release(topo), 0);

    for interfaces in [
        PacketInterfaces::None,
        PacketInterfaces::Input,
        PacketInterfaces::Output,
        PacketInterfaces::Both,
    ] {
        let (flux, dropped) = Flux::nouveau();
        let this = new_packet_stream_object(flux, interfaces).into_raw();

        // Le principal, puis chaque tête satellite exposée : aucune n'est prise pour un
        // miniport, et aucune ne fait fuir de référence au passage.
        let mut cibles = vec![("principal", this)];
        if interfaces.has_input() {
            cibles.push(("tête entrée", qi_ok(this, &iid_entree())));
        }
        if interfaces.has_output() {
            cibles.push(("tête sortie", qi_ok(this, &iid_sortie())));
        }
        let attendu = cibles.len() as ULONG;

        for (nom, cible) in &cibles {
            assert_eq!(
                appeler_propriete(*cible),
                STATUS_INVALID_DEVICE_REQUEST,
                "{nom} de {interfaces:?} n'est pas une cible de propriété"
            );
            assert_eq!(
                appeler_evenement(*cible),
                STATUS_INVALID_DEVICE_REQUEST,
                "{nom} de {interfaces:?} n'est pas une cible d'événement"
            );
        }
        assert_eq!(refcount(this), attendu, "{interfaces:?}");

        for (_, cible) in cibles.iter().skip(1) {
            release(*cible);
        }
        assert_eq!(release(this), 0, "{interfaces:?}");
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "{interfaces:?}");
    }
}

// ---------------------------------------------------------------------------------
// Les deux points d'observation du `QueryInterface` composite (lot 2).
// ---------------------------------------------------------------------------------

/// Flux qui ne sert rien du mode paquets mais **observe** les demandes d'IID.
///
/// Il est distinct de [`Flux`] et de [`FluxMuet`] à dessein : ce qui se vérifie ici est le
/// chemin d'observation, pas le service, et un flux qui ferait les deux ne dirait pas lequel
/// des deux a été emprunté.
struct FluxObservateur {
    /// `(demandes, rendues)` sur `IID_IMiniportWaveRTInputStream`.
    entree: AtomicU64,
    /// `(demandes, rendues)` sur `IID_IMiniportWaveRTOutputStream`.
    sortie: AtomicU64,
}

impl FluxObservateur {
    const fn nouveau() -> Self {
        Self {
            entree: AtomicU64::new(0),
            sortie: AtomicU64::new(0),
        }
    }

    /// Un compteur par mot de 32 bits : les demandes en poids faible, les réponses en poids
    /// fort. Un seul atomique par sens suffit et évite d'en désynchroniser deux.
    fn noter(compteur: &AtomicU64, rendu: bool) {
        let pas = if rendu { 1 | (1 << 32) } else { 1 };
        compteur.fetch_add(pas, Ordering::SeqCst);
    }

    fn lire(compteur: &AtomicU64) -> (u64, u64) {
        let brut = compteur.load(Ordering::SeqCst);
        (brut & 0xFFFF_FFFF, brut >> 32)
    }
}

impl MiniportWaveRTStream for FluxObservateur {
    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(0)
    }

    fn allocate_audio_buffer(&self, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for FluxObservateur {
    fn allocate_buffer_with_notification(&self, _: u32, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for FluxObservateur {
    fn note_input_query(&self, rendu: bool) {
        Self::noter(&self.entree, rendu);
    }
}

impl MiniportWaveRTOutputStream for FluxObservateur {
    fn note_output_query(&self, rendu: bool) {
        Self::noter(&self.sortie, rendu);
    }
}

/// Les deux points d'observation voient **toutes** les demandes d'IID de paquets, exposées
/// ou non, et rien d'autre.
///
/// C'est ce qui rend mesurable, sans rien exposer, la question du lot 2 : le moteur audio
/// demande-t-il le mode paquets ? Un compteur qui ne verrait que les demandes **satisfaites**
/// répondrait toujours « non » sur le pilote livré, c'est-à-dire ne répondrait rien.
#[test]
fn les_points_d_observation_voient_les_demandes_exposees_ou_non() {
    for interfaces in [
        PacketInterfaces::None,
        PacketInterfaces::Input,
        PacketInterfaces::Output,
        PacketInterfaces::Both,
    ] {
        let this = new_packet_stream_object(FluxObservateur::nouveau(), interfaces).into_raw();
        let flux = |this: This| unsafe {
            conduit_com::ComObject::<
                IMiniportWaveRTStreamNotificationVtbl,
                portcls::PacketStream<FluxObservateur>,
            >::inner(this)
        };

        // Les deux IID de paquets, demandés une fois chacun. Ceux qui sont exposés rendent
        // une tête satellite (référence à rendre), les autres sont refusés comme n'importe
        // quel IID inconnu — et les deux cas sont notifiés.
        for (iid, expose) in [
            (iid_entree(), interfaces.has_input()),
            (iid_sortie(), interfaces.has_output()),
        ] {
            let (status, out) = query_interface(this, &iid);
            if expose {
                assert_eq!(status, STATUS_SUCCESS, "{interfaces:?} {iid}");
                release(out);
            } else {
                assert_eq!(status, STATUS_INVALID_PARAMETER, "{interfaces:?} {iid}");
                assert!(out.is_null(), "{interfaces:?} {iid}");
            }
        }

        let (demandes_e, rendues_e) = FluxObservateur::lire(&flux(this).entree);
        let (demandes_s, rendues_s) = FluxObservateur::lire(&flux(this).sortie);
        assert_eq!(
            demandes_e, 1,
            "{interfaces:?} : l'IID d'entrée a été demandé"
        );
        assert_eq!(
            demandes_s, 1,
            "{interfaces:?} : l'IID de sortie a été demandé"
        );
        assert_eq!(
            rendues_e,
            u64::from(interfaces.has_input()),
            "{interfaces:?}"
        );
        assert_eq!(
            rendues_s,
            u64::from(interfaces.has_output()),
            "{interfaces:?}"
        );

        // Un IID qui n'est pas du mode paquets ne notifie rien : l'observation ne déforme
        // pas le `QueryInterface` générique.
        let base = qi_ok(this, &guid(&IID_IMiniportWaveRTStream));
        release(base);
        assert_eq!(FluxObservateur::lire(&flux(this).entree), (1, rendues_e));
        assert_eq!(FluxObservateur::lire(&flux(this).sortie), (1, rendues_s));

        assert_eq!(release(this), 0, "{interfaces:?}");
    }
}

/// Une demande relayée par une **tête satellite** est notifiée elle aussi.
///
/// Le `QueryInterface` d'un satellite délègue au principal (transitivité COM) : sans ce
/// test, un chemin de demande sur deux resterait invisible, et le relevé sous-compterait
/// exactement là où le moteur audio est déjà passé en mode paquets.
#[test]
fn une_demande_relayee_par_un_satellite_est_notifiee() {
    let this =
        new_packet_stream_object(FluxObservateur::nouveau(), PacketInterfaces::Both).into_raw();
    let flux = unsafe {
        conduit_com::ComObject::<
            IMiniportWaveRTStreamNotificationVtbl,
            portcls::PacketStream<FluxObservateur>,
        >::inner(this)
    };

    // Une première demande sur le principal, puis la même depuis la tête d'entrée.
    let tete = qi_ok(this, &iid_entree());
    assert_eq!(FluxObservateur::lire(&flux.entree), (1, 1));
    let depuis_satellite = qi_ok(tete, &iid_sortie());
    assert_eq!(
        FluxObservateur::lire(&flux.sortie),
        (1, 1),
        "le QueryInterface d'un satellite délègue au principal : la demande compte"
    );

    release(depuis_satellite);
    release(tete);
    assert_eq!(release(this), 0);
}
