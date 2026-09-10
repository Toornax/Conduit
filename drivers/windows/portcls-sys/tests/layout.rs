//! Disposition des bindings contre l'oracle C (`layout.golden`, produit par `cl.exe` via
//! `drivers/windows/tools/regen-layout.ps1` sur `tools/sizeof-probe.c`).
//!
//! Deux compilateurs sur les mêmes en-têtes du WDK : clang (bindgen, `build.rs`) et
//! cl.exe (le probe). Chaque ligne du golden donne une assertion nommée : `sizeof` d'une
//! structure ou d'une vtable (nombre de slots × 8), `offset` d'un champ, ou valeur d'un
//! GUID. Le test échoue aussi si le golden et les tables ci-dessous ne couvrent pas
//! exactement les mêmes noms, pour qu'un ajout d'un côté ne soit pas oublié de l'autre.
//!
//! Les décalages comptent autant que les tailles pour les structures de propriétés KS :
//! M1b les sérialise à la main, champ par champ, dans les tampons venus du mode
//! utilisateur (écrire à travers un pointeur de structure potentiellement désaligné
//! serait un comportement indéfini en Rust), donc chaque décalage y devient du code.

// Tests en mode utilisateur : une panique est un échec de test, pas un écran bleu.
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::{
    collections::BTreeSet,
    mem::{offset_of, size_of},
};

use portcls_sys::*;

const GOLDEN: &str = include_str!("layout.golden");

/// Table `nom → size_of::<type>()`, dans l'ordre du probe.
macro_rules! tailles {
    ($($nom:ident => $type:ty),* $(,)?) => {
        const NOMS_TAILLES: &[&str] = &[$(stringify!($nom)),*];
        fn taille(nom: &str) -> Option<usize> {
            match nom {
                $(stringify!($nom) => Some(size_of::<$type>()),)*
                _ => None,
            }
        }
    };
}

/// Table `« Type.Champ » → offset_of!(Type, Champ)`, dans l'ordre du probe.
macro_rules! decalages {
    ($($type:ident { $($champ:ident),* $(,)? }),* $(,)?) => {
        const NOMS_DECALAGES: &[&str] = &[
            $($(concat!(stringify!($type), ".", stringify!($champ))),*),*
        ];
        fn decalage(nom: &str) -> Option<usize> {
            match nom {
                $($(concat!(stringify!($type), ".", stringify!($champ))
                    => Some(offset_of!($type, $champ)),)*)*
                _ => None,
            }
        }
    };
}

/// Table `nom → GUID généré`.
macro_rules! guids {
    ($($nom:ident),* $(,)?) => {
        const NOMS_GUIDS: &[&str] = &[$(stringify!($nom)),*];
        fn guid(nom: &str) -> Option<GUID> {
            match nom {
                $(stringify!($nom) => Some($nom),)*
                _ => None,
            }
        }
    };
}

tailles! {
    GUID => GUID,
    KSDATAFORMAT => KSDATAFORMAT,
    KSDATARANGE => KSDATARANGE,
    KSDATARANGE_AUDIO => KSDATARANGE_AUDIO,
    WAVEFORMATEX => WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE => WAVEFORMATEXTENSIBLE,
    KSDATAFORMAT_WAVEFORMATEX => KSDATAFORMAT_WAVEFORMATEX,
    KSDATAFORMAT_WAVEFORMATEXTENSIBLE => KSDATAFORMAT_WAVEFORMATEXTENSIBLE,
    KSPIN_DESCRIPTOR => KSPIN_DESCRIPTOR,
    PCPIN_DESCRIPTOR => PCPIN_DESCRIPTOR,
    PCNODE_DESCRIPTOR => PCNODE_DESCRIPTOR,
    PCCONNECTION_DESCRIPTOR => PCCONNECTION_DESCRIPTOR,
    PCFILTER_DESCRIPTOR => PCFILTER_DESCRIPTOR,
    PCPROPERTY_ITEM => PCPROPERTY_ITEM,
    PCPROPERTY_REQUEST => PCPROPERTY_REQUEST,
    PCEVENT_ITEM => PCEVENT_ITEM,
    PCEVENT_REQUEST => PCEVENT_REQUEST,
    PCAUTOMATION_TABLE => PCAUTOMATION_TABLE,
    KSJACK_DESCRIPTION => KSJACK_DESCRIPTION,
    KSRTAUDIO_BUFFER => KSRTAUDIO_BUFFER,
    KSRTAUDIO_BUFFER_PROPERTY => KSRTAUDIO_BUFFER_PROPERTY,
    KSRTAUDIO_HWLATENCY => KSRTAUDIO_HWLATENCY,
    KSRTAUDIO_NOTIFICATION_EVENT_PROPERTY => KSRTAUDIO_NOTIFICATION_EVENT_PROPERTY,
    KSAUDIO_POSITION => KSAUDIO_POSITION,
    KSSTATE => KSSTATE::Type,
    DEVICE_OBJECT => DEVICE_OBJECT,
    IRP => IRP,
    UNICODE_STRING => UNICODE_STRING,
    KSPROPERTY_DESCRIPTION => KSPROPERTY_DESCRIPTION,
    KSPROPERTY_MEMBERSHEADER => KSPROPERTY_MEMBERSHEADER,
    KSPROPERTY_STEPPING_LONG => KSPROPERTY_STEPPING_LONG,
    KSNODEPROPERTY => KSNODEPROPERTY,
    KSNODEPROPERTY_AUDIO_CHANNEL => KSNODEPROPERTY_AUDIO_CHANNEL,
    KSMULTIPLE_ITEM => KSMULTIPLE_ITEM,
    KSP_PIN => KSP_PIN,
    KSEVENTDATA => KSEVENTDATA,
    KSEVENT_ENTRY => KSEVENT_ENTRY,
    KSATTRIBUTE => KSATTRIBUTE,
    KSATTRIBUTE_LIST => KSATTRIBUTE_LIST,
    KSATTRIBUTE_AUDIOSIGNALPROCESSING_MODE => KSATTRIBUTE_AUDIOSIGNALPROCESSING_MODE,
    IUnknownVtbl => IUnknownVtbl,
    IMiniportVtbl => IMiniportVtbl,
    IMiniportWaveRTVtbl => IMiniportWaveRTVtbl,
    IMiniportWaveRTStreamVtbl => IMiniportWaveRTStreamVtbl,
    IMiniportWaveRTStreamNotificationVtbl => IMiniportWaveRTStreamNotificationVtbl,
    IMiniportTopologyVtbl => IMiniportTopologyVtbl,
    IAdapterPowerManagementVtbl => IAdapterPowerManagementVtbl,
    IPortVtbl => IPortVtbl,
    IPortWaveRTVtbl => IPortWaveRTVtbl,
    IPortTopologyVtbl => IPortTopologyVtbl,
    IPortWaveRTStreamVtbl => IPortWaveRTStreamVtbl,
    IResourceListVtbl => IResourceListVtbl,
    IRegistryKeyVtbl => IRegistryKeyVtbl,
    IPortEventsVtbl => IPortEventsVtbl,
    IPortClsVersionVtbl => IPortClsVersionVtbl,
}

decalages! {
    KSPROPERTY_DESCRIPTION {
        AccessFlags,
        DescriptionSize,
        PropTypeSet,
        MembersListCount,
        Reserved,
    },
    KSPROPERTY_MEMBERSHEADER {
        MembersFlags,
        MembersSize,
        MembersCount,
        Flags,
    },
    KSPROPERTY_STEPPING_LONG {
        SteppingDelta,
        Reserved,
        Bounds,
    },
    KSNODEPROPERTY {
        Property,
        NodeId,
        Reserved,
    },
    KSNODEPROPERTY_AUDIO_CHANNEL {
        NodeProperty,
        Channel,
        Reserved,
    },
    KSMULTIPLE_ITEM {
        Size,
        Count,
    },
    KSJACK_DESCRIPTION {
        ChannelMapping,
        Color,
        ConnectionType,
        GeoLocation,
        GenLocation,
        PortConnection,
        IsConnected,
    },
    KSP_PIN {
        Property,
        PinId,
    },
    KSATTRIBUTE {
        Size,
        Flags,
        Attribute,
    },
    KSATTRIBUTE_LIST {
        Count,
        Attributes,
    },
    PCNODE_DESCRIPTOR {
        Flags,
        AutomationTable,
        Type,
        Name,
    },
    PCPROPERTY_ITEM {
        Set,
        Id,
        Flags,
        Handler,
    },
    PCEVENT_ITEM {
        Set,
        Id,
        Flags,
        Handler,
    },
    PCEVENT_REQUEST {
        MajorTarget,
        MinorTarget,
        Node,
        EventItem,
        EventEntry,
        Verb,
        Irp,
    },
    PCAUTOMATION_TABLE {
        PropertyItemSize,
        PropertyCount,
        Properties,
        MethodItemSize,
        MethodCount,
        Methods,
        EventItemSize,
        EventCount,
        Events,
        Reserved,
    },
    KSEVENTDATA {
        NotificationType,
    },
}

guids! {
    IID_IUnknown,
    IID_IMiniportWaveRT,
    IID_IMiniportTopology,
    IID_IAdapterPowerManagement,
    IID_IPortWaveRT,
    IID_IPortTopology,
    IID_IPortEvents,
    KSCATEGORY_AUDIO,
    KSDATAFORMAT_SUBTYPE_PCM,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    KSNODETYPE_SPEAKER,
    KSNODETYPE_VOLUME,
    KSNODETYPE_MUTE,
    KSPROPSETID_Jack,
    KSEVENTSETID_PinCapsChange,
    KSPROPSETID_Audio,
    KSPROPTYPESETID_General,
    KSPROPSETID_AudioSignalProcessing,
    KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE,
    AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
    AUDIO_SIGNALPROCESSINGMODE_RAW,
}

/// Même format que le probe : `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` en majuscules.
fn guid_texte(g: &GUID) -> String {
    let d = g.Data4;
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.Data1, g.Data2, g.Data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

fn lignes_golden() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    GOLDEN
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let champs: Vec<&str> = l.split('\t').collect();
            assert_eq!(champs.len(), 3, "ligne du golden mal formée : {l:?}");
            (champs[0], champs[1], champs[2])
        })
}

#[test]
fn tailles_et_guids_conformes_au_golden() {
    let mut nb = 0;
    for (genre, nom, attendu) in lignes_golden() {
        nb += 1;
        match genre {
            "sizeof" => {
                let attendu: usize = attendu.parse().expect("taille entière dans le golden");
                let obtenu =
                    taille(nom).unwrap_or_else(|| panic!("{nom} absent de la table `tailles!`"));
                assert_eq!(
                    obtenu, attendu,
                    "size_of::<{nom}>() : Rust {obtenu}, cl.exe {attendu}"
                );
            }
            "offset" => {
                let attendu: usize = attendu.parse().expect("décalage entier dans le golden");
                let obtenu = decalage(nom)
                    .unwrap_or_else(|| panic!("{nom} absent de la table `decalages!`"));
                assert_eq!(
                    obtenu, attendu,
                    "offset_of!({nom}) : Rust {obtenu}, cl.exe {attendu}"
                );
            }
            "guid" => {
                let obtenu =
                    guid(nom).unwrap_or_else(|| panic!("{nom} absent de la table `guids!`"));
                let obtenu = guid_texte(&obtenu);
                assert_eq!(
                    obtenu, attendu,
                    "GUID {nom} : Rust {obtenu}, cl.exe {attendu}"
                );
            }
            autre => panic!("genre de mesure inconnu dans le golden : {autre:?}"),
        }
    }
    assert!(nb >= 50, "golden trop court ({nb} mesures)");
}

#[test]
fn golden_et_tables_couvrent_les_memes_noms() {
    let golden_tailles: BTreeSet<&str> = lignes_golden()
        .filter(|(g, _, _)| *g == "sizeof")
        .map(|(_, n, _)| n)
        .collect();
    let golden_decalages: BTreeSet<&str> = lignes_golden()
        .filter(|(g, _, _)| *g == "offset")
        .map(|(_, n, _)| n)
        .collect();
    let golden_guids: BTreeSet<&str> = lignes_golden()
        .filter(|(g, _, _)| *g == "guid")
        .map(|(_, n, _)| n)
        .collect();
    let tables_tailles: BTreeSet<&str> = NOMS_TAILLES.iter().copied().collect();
    let tables_decalages: BTreeSet<&str> = NOMS_DECALAGES.iter().copied().collect();
    let tables_guids: BTreeSet<&str> = NOMS_GUIDS.iter().copied().collect();
    assert_eq!(
        golden_tailles, tables_tailles,
        "sizeof : golden ≠ table `tailles!`"
    );
    assert_eq!(
        golden_decalages, tables_decalages,
        "offset : golden ≠ table `decalages!`"
    );
    assert_eq!(golden_guids, tables_guids, "guid : golden ≠ table `guids!`");
}

/// Les vtables sont des tableaux de pointeurs de fonction : taille multiple de 8, jamais
/// vide ; et la version corrigée de `IPortClsVersion` compte bien IUnknown + GetVersion.
#[test]
fn vtables_multiples_de_huit() {
    for (genre, nom, attendu) in lignes_golden() {
        if genre == "sizeof" && nom.ends_with("Vtbl") {
            let octets: usize = attendu.parse().expect("taille entière");
            assert!(octets > 0 && octets % 8 == 0, "{nom} : {octets} octets");
        }
    }
    assert_eq!(size_of::<IPortClsVersionVtbl>(), 4 * 8);
}
