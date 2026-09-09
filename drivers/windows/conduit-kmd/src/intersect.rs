//! `IMiniport::DataRangeIntersection` des miniports WaveRT : notre gestionnaire
//! d'intersection, celui qui remplace le gestionnaire par défaut de PortCls.
//!
//! Le module ne fait que **traduire** : il lit les deux `KSDATARANGE` que PortCls remet,
//! confie la négociation à `conduit_kmd_core::wavefmt` — qui est pur, testé et fuzzable —
//! puis recopie le format retenu dans le tampon de sortie sous la forme
//! `KSDATAFORMAT_WAVEFORMATEX` ou `KSDATAFORMAT_WAVEFORMATEXTENSIBLE`. La négociation de
//! taille (`ResultantFormatLength`, `STATUS_BUFFER_OVERFLOW`, `STATUS_BUFFER_TOO_SMALL`)
//! est **déjà faite** par le thunk de `portcls::miniport` : il suffit de rendre la taille
//! requise et de n'écrire que si elle tient.
//!
//! # Pourquoi ne plus s'en remettre à PortCls
//!
//! Le défaut du trait rend `STATUS_NOT_IMPLEMENTED`, ce que la documentation décrit comme
//! « *defers data-intersection handling to the port driver's default data-intersection
//! handler* ». Ce gestionnaire-là est limité, et la documentation le dit sans détour
//! (« Default Data-Intersection Handlers ») : *only PCM data formats*, *only mono and
//! stereo audio streams*, et aucun format contenant un `WAVEFORMATEXTENSIBLE` — « *which is
//! needed, for example, to specify the channel mask for a format with more than two
//! channels* ». Un câble à trois canaux ou plus ne pouvait donc obtenir aucun format, et
//! nos plages flottantes n'étaient jamais retenues.
//!
//! # Ce que ce gestionnaire refuse, et pourquoi il le dit
//!
//! Un refus est un `STATUS_NO_MATCH`, et un `STATUS_NO_MATCH` ne se voit **nulle part** :
//! l'endpoint apparaît quand même, sans format, et rien n'est écrit dans le journal. C'est
//! exactement la panne muette que le dépôt refuse. D'où le tri de
//! [`NoMatch::est_numerique`] :
//!
//! - un refus de **sous-type** est le fonctionnement normal — PortCls propose nos trois
//!   plages l'une après l'autre à un client qui n'en veut qu'une famille — et ne part
//!   qu'au débogueur ;
//! - un refus **numérique** (canaux, profondeur, fréquence) dit que Windows a demandé un
//!   format que le câble ne sert pas, et part au **journal d'événements**, une fois par
//!   miniport et par démarrage ([`Negotiation::reported`]).
//!
//! C'est cette entrée-là qui répond, sans débogueur, à la question « qu'est-ce que Windows
//! demande, au juste ? » quand un endpoint n'a aucun format.

use core::mem::{align_of, size_of};
use core::ptr;
use core::slice;
use core::sync::atomic::{AtomicBool, Ordering};

use conduit_kmd_core::config::CableFormat;
use conduit_kmd_core::wavefmt::{AudioRange, NoMatch, WaveFormat};
use conduit_kmd_core::{
    KSDATAFORMAT_WAVEFORMATEX_BYTES, KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES, SampleKind,
    WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM, cable_formats, intersect,
    validate,
};
use portcls::STATUS_NO_MATCH;
use portcls::conduit_com::NtStatus;
use portcls_sys::{
    GUID, KSDATAFORMAT, KSDATAFORMAT__bindgen_ty_1, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM, KSDATAFORMAT_TYPE_AUDIO,
    KSDATAFORMAT_WAVEFORMATEX, KSDATAFORMAT_WAVEFORMATEXTENSIBLE, KSDATARANGE, KSDATARANGE_AUDIO,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE__bindgen_ty_1,
};

use crate::eventlog::{EventLog, kmd_event};
use crate::registry;

/// Les deux formes de descripteur, telles que les bindings les rendent, et ce que le crate
/// portable en dit : une divergence de taille casserait la compilation ici plutôt que de
/// produire un `FormatSize` qui ment.
///
/// Les égalités « somme des membres == taille » sont ce qui autorise [`as_bytes`] : une
/// structure `packed` dont les membres remplissent exactement la taille n'a aucun octet de
/// remplissage, donc aucun octet non initialisé à recopier dans le tampon de PortCls.
const _: () = {
    assert!(size_of::<KSDATAFORMAT>() == 64);
    assert!(size_of::<KSDATAFORMAT__bindgen_ty_1>() == size_of::<KSDATAFORMAT>());
    assert!(size_of::<WAVEFORMATEX>() == 18);
    assert!(size_of::<WAVEFORMATEXTENSIBLE__bindgen_ty_1>() == 2);
    // 18 (`Format`) + 2 (`Samples`) + 4 (`dwChannelMask`) + 16 (`SubFormat`).
    assert!(size_of::<WAVEFORMATEXTENSIBLE>() == 40);
    assert!(
        size_of::<WAVEFORMATEXTENSIBLE>()
            == size_of::<WAVEFORMATEX>()
                + size_of::<WAVEFORMATEXTENSIBLE__bindgen_ty_1>()
                + size_of::<u32>()
                + size_of::<GUID>()
    );
    assert!(size_of::<KSDATAFORMAT_WAVEFORMATEX>() == KSDATAFORMAT_WAVEFORMATEX_BYTES as usize);
    assert!(
        size_of::<KSDATAFORMAT_WAVEFORMATEXTENSIBLE>()
            == KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES as usize
    );
    assert!(
        size_of::<KSDATAFORMAT_WAVEFORMATEX>()
            == size_of::<KSDATAFORMAT>() + size_of::<WAVEFORMATEX>()
    );
    assert!(
        size_of::<KSDATAFORMAT_WAVEFORMATEXTENSIBLE>()
            == size_of::<KSDATAFORMAT>() + size_of::<WAVEFORMATEXTENSIBLE>()
    );
    // `packed` : aucune contrainte d'alignement, donc aucun trou entre les membres.
    assert!(align_of::<KSDATAFORMAT_WAVEFORMATEX>() == 1);
    assert!(align_of::<KSDATAFORMAT_WAVEFORMATEXTENSIBLE>() == 1);
    // Et l'élargissement `&KSDATARANGE` → `&KSDATARANGE_AUDIO` des deux lecteurs ne change
    // pas l'alignement requis : sans cette égalité, une plage bien alignée pour le premier
    // type ne le serait pas forcément pour le second.
    assert!(align_of::<KSDATARANGE_AUDIO>() == align_of::<KSDATARANGE>());
    assert!(size_of::<KSDATARANGE_AUDIO>() == KSDATARANGE_AUDIO_SIZE as usize);
    // Les étiquettes `mmreg.h` recopiées dans le crate portable sont bien celles du WDK.
    assert!(WAVE_FORMAT_PCM == portcls_sys::WAVE_FORMAT_PCM);
    assert!(WAVE_FORMAT_IEEE_FLOAT == portcls_sys::WAVE_FORMAT_IEEE_FLOAT);
    assert!(WAVE_FORMAT_EXTENSIBLE == portcls_sys::WAVE_FORMAT_EXTENSIBLE);
};

/// Taille d'une `KSDATARANGE_AUDIO` : au-dessous, la plage n'a pas ses champs audio.
const KSDATARANGE_AUDIO_SIZE: u32 = size_of::<KSDATARANGE_AUDIO>() as u32;

/// Égalité de deux `GUID` (le type généré n'implémente pas `PartialEq`).
pub(crate) fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 && a.Data4 == b.Data4
}

/// Vrai pour `GUID_NULL`, qui est la valeur des trois jokers de `ks.h`
/// (`KSDATAFORMAT_TYPE_WILDCARD`, `KSDATAFORMAT_SUBTYPE_WILDCARD`,
/// `KSDATAFORMAT_SPECIFIER_WILDCARD` sont tous `#define`s vers `GUID_NULL`).
fn is_wildcard(guid: &GUID) -> bool {
    guid.Data1 == 0 && guid.Data2 == 0 && guid.Data3 == 0 && guid.Data4 == [0; 8]
}

/// La famille d'échantillons d'un sous-format ; `None` pour le joker,
/// `Some(SampleKind::Other)` pour un sous-format que le pilote ne sert pas (que
/// `wavefmt::intersect` refusera).
fn subtype_kind(subformat: &GUID) -> Option<SampleKind> {
    if is_wildcard(subformat) {
        None
    } else if guid_eq(subformat, &KSDATAFORMAT_SUBTYPE_PCM) {
        Some(SampleKind::Pcm)
    } else if guid_eq(subformat, &KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) {
        Some(SampleKind::Float)
    } else {
        Some(SampleKind::Other)
    }
}

/// Le `SubFormat` à écrire pour une famille.
fn subtype_guid(kind: SampleKind) -> GUID {
    match kind {
        SampleKind::Float => KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        SampleKind::Pcm | SampleKind::Other => KSDATAFORMAT_SUBTYPE_PCM,
    }
}

/// L'en-tête d'une plage, lu par son membre nommé.
///
/// # Safety
///
/// `range` pointe une `KSDATARANGE` lisible (contrat du thunk de `portcls::miniport`).
unsafe fn header(range: &KSDATARANGE) -> KSDATAFORMAT__bindgen_ty_1 {
    // SAFETY: lecture du membre nommé de l'union `KSDATAFORMAT` ; l'autre membre,
    // `Alignment`, ne sert qu'à l'alignement et toute plage bien formée écrit celui-ci.
    unsafe { range.__bindgen_anon_1 }
}

/// Traduit la plage **du client** en [`AudioRange`].
///
/// Le type majeur et le spécificateur doivent être ceux de l'audio `WAVEFORMATEX` ou des
/// jokers ; le sous-format donne la famille (joker compris). Les bornes numériques ne sont
/// lues que si `FormatSize` couvre réellement une `KSDATARANGE_AUDIO` — sinon la plage est
/// une `KSDATARANGE` nue de 64 octets, qui n'impose rien ([`AudioRange::WILDCARD`]), et
/// lire au-delà serait une lecture hors objet.
///
/// # Safety
///
/// `client` pointe une `KSDATARANGE` suivie d'au moins `FormatSize` octets lisibles
/// (contrat de `IMiniport::DataRangeIntersection`, relayé par le thunk).
unsafe fn read_client_range(client: &KSDATARANGE) -> Option<AudioRange> {
    // SAFETY: contrat relayé.
    let head = unsafe { header(client) };
    if !is_wildcard(&head.MajorFormat) && !guid_eq(&head.MajorFormat, &KSDATAFORMAT_TYPE_AUDIO) {
        return None;
    }
    if !is_wildcard(&head.Specifier)
        && !guid_eq(&head.Specifier, &KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
    {
        return None;
    }
    let kind = subtype_kind(&head.SubFormat);
    if head.FormatSize < KSDATARANGE_AUDIO_SIZE {
        return Some(AudioRange {
            kind,
            ..AudioRange::WILDCARD
        });
    }
    // SAFETY: `FormatSize` annonce au moins les 88 octets d'une `KSDATARANGE_AUDIO`, et le
    // contrat garantit que ces octets sont lisibles ; `KSDATARANGE_AUDIO` commence par sa
    // `KSDATARANGE`, l'élargissement du type ne dépasse donc pas l'objet. Les deux types
    // ont le même alignement (8, celui de la `KSDATAFORMAT` qu'ils commencent tous les
    // deux) : la référence d'entrée étant valide, celle-ci l'est aussi.
    let audio = unsafe { &*ptr::from_ref(client).cast::<KSDATARANGE_AUDIO>() };
    Some(AudioRange {
        kind,
        max_channels: audio.MaximumChannels,
        min_bits: audio.MinimumBitsPerSample,
        max_bits: audio.MaximumBitsPerSample,
        min_rate: audio.MinimumSampleFrequency,
        max_rate: audio.MaximumSampleFrequency,
    })
}

/// Traduit **notre** plage en [`AudioRange`].
///
/// Bien plus strict que [`read_client_range`] : celle-ci vient de
/// `descriptors::SYSTEM_RANGE_VALUES`, elle doit donc être une `KSDATARANGE_AUDIO` audio,
/// `WAVEFORMATEX`, de sous-format PCM ou flottant. `None` signale soit la plage analogique
/// d'une broche bridge — auquel cas PortCls nous interroge sur une broche qui n'a pas de
/// format —, soit une table cassée.
///
/// # Safety
///
/// Même contrat que [`read_client_range`].
unsafe fn read_our_range(my: &KSDATARANGE) -> Option<AudioRange> {
    // SAFETY: contrat relayé.
    let head = unsafe { header(my) };
    if head.FormatSize != KSDATARANGE_AUDIO_SIZE
        || !guid_eq(&head.MajorFormat, &KSDATAFORMAT_TYPE_AUDIO)
        || !guid_eq(&head.Specifier, &KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
    {
        return None;
    }
    let kind = match subtype_kind(&head.SubFormat) {
        Some(SampleKind::Pcm) => SampleKind::Pcm,
        Some(SampleKind::Float) => SampleKind::Float,
        _ => return None,
    };
    // SAFETY: `FormatSize` vaut exactement les 88 octets d'une `KSDATARANGE_AUDIO` ; voir
    // `read_client_range` pour l'élargissement.
    let audio = unsafe { &*ptr::from_ref(my).cast::<KSDATARANGE_AUDIO>() };
    Some(AudioRange {
        kind: Some(kind),
        max_channels: audio.MaximumChannels,
        min_bits: audio.MinimumBitsPerSample,
        max_bits: audio.MaximumBitsPerSample,
        min_rate: audio.MinimumSampleFrequency,
        max_rate: audio.MaximumSampleFrequency,
    })
}

/// Les octets d'une structure `packed` sans rembourrage.
///
/// # Safety
///
/// `T` est `repr(C, packed)` (alignement 1) et la somme de la taille de ses membres vaut
/// `size_of::<T>()` : aucun octet de remplissage, donc aucun octet non initialisé dans la
/// tranche rendue. Les assertions `const` en tête de module l'établissent pour les deux
/// seuls types passés ici.
unsafe fn as_bytes<T>(value: &T) -> &[u8] {
    // SAFETY: `value` est vivant et initialisé en entier (contrat) ; `u8` n'a pas de
    // contrainte d'alignement et la tranche ne dépasse pas l'objet.
    unsafe { slice::from_raw_parts(ptr::from_ref(value).cast::<u8>(), size_of::<T>()) }
}

/// L'en-tête `KSDATAFORMAT` du format retenu.
///
/// `SampleSize` porte les octets d'une trame, comme SYSVAD le renseigne ; `Flags` et
/// `Reserved` sont nuls.
fn ks_header(format: WaveFormat) -> KSDATAFORMAT {
    KSDATAFORMAT {
        __bindgen_anon_1: KSDATAFORMAT__bindgen_ty_1 {
            FormatSize: format.format_size(),
            Flags: 0,
            SampleSize: format.sample_size(),
            Reserved: 0,
            MajorFormat: KSDATAFORMAT_TYPE_AUDIO,
            SubFormat: subtype_guid(format.kind()),
            Specifier: KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
        },
    }
}

/// Le `WAVEFORMATEX` du format retenu (`cbSize` vaut 22 sous la forme étendue, 0 sinon).
fn wave_format_ex(format: WaveFormat) -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: format.format_tag(),
        nChannels: format.channels(),
        nSamplesPerSec: format.sample_rate(),
        nAvgBytesPerSec: format.avg_bytes_per_sec(),
        nBlockAlign: format.block_align(),
        wBitsPerSample: format.bits_per_sample(),
        cbSize: format.cb_size(),
    }
}

/// Écrit le descripteur de `format` au début de `out` ; `false` si `out` est trop court.
///
/// La forme est celle que [`WaveFormat::is_extensible`] commande : `WAVEFORMATEXTENSIBLE`
/// dès qu'il y a plus de deux canaux ou plus de seize bits — les deux seuls cas où la
/// documentation dit `WAVEFORMATEX` insuffisant —, `WAVEFORMATEX` simple sinon.
fn write_format(format: WaveFormat, out: &mut [u8]) -> bool {
    let Ok(taille) = usize::try_from(format.format_size()) else {
        return false;
    };
    let Some(cible) = out.get_mut(..taille) else {
        return false;
    };
    if format.is_extensible() {
        let valeur = KSDATAFORMAT_WAVEFORMATEXTENSIBLE {
            DataFormat: ks_header(format),
            WaveFormatExt: WAVEFORMATEXTENSIBLE {
                Format: wave_format_ex(format),
                Samples: WAVEFORMATEXTENSIBLE__bindgen_ty_1 {
                    wValidBitsPerSample: format.valid_bits(),
                },
                dwChannelMask: format.channel_mask(),
                SubFormat: subtype_guid(format.kind()),
            },
        };
        // SAFETY: `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` est `packed`, d'alignement 1, et la
        // somme de ses membres vaut sa taille (assertions `const` en tête de module) : la
        // valeur qu'on vient de construire est initialisée octet pour octet.
        cible.copy_from_slice(unsafe { as_bytes(&valeur) });
    } else {
        let valeur = KSDATAFORMAT_WAVEFORMATEX {
            DataFormat: ks_header(format),
            WaveFormatEx: wave_format_ex(format),
        };
        // SAFETY: idem pour `KSDATAFORMAT_WAVEFORMATEX`.
        cible.copy_from_slice(unsafe { as_bytes(&valeur) });
    }
    true
}

/// Le contexte d'une négociation : de quel filtre elle vient, quel format le câble sert, et
/// où consigner un refus numérique.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Negotiation<'a> {
    /// Nom du filtre, pour le journal (`"WaveRender"` ou `"WaveCapture"`).
    pub(crate) name: &'static str,
    /// Numéro du câble.
    pub(crate) n: u32,
    /// La seule broche de ce filtre qui porte des plages audio.
    pub(crate) system_pin: u32,
    /// Le format configuré du câble : ce que `NewStream` acceptera, et donc la seule chose
    /// que ce gestionnaire a le droit de proposer.
    pub(crate) format: CableFormat,
    /// Journal d'événements de l'adaptateur.
    pub(crate) log: EventLog,
    /// Faux tant qu'aucun refus numérique n'a été consigné pour ce miniport : un seul par
    /// démarrage, PortCls appelant l'intersection une fois par couple de plages.
    pub(crate) reported: &'a AtomicBool,
}

impl Negotiation<'_> {
    /// `IMiniport::DataRangeIntersection` : rend la taille du format retenu, après l'avoir
    /// écrit dans `out` si le tampon la contient.
    ///
    /// L'appelant est le thunk de `portcls::miniport`, qui traduit `Ok(taille)` en
    /// `STATUS_SUCCESS`, `STATUS_BUFFER_OVERFLOW` (interrogation de taille) ou
    /// `STATUS_BUFFER_TOO_SMALL`. Il n'y a donc **rien** à faire ici de la négociation de
    /// taille, sinon rendre la bonne valeur et n'écrire que ce qui tient.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Erreurs
    ///
    /// `STATUS_NO_MATCH` si les deux plages ne se croisent pas, si la broche n'est pas la
    /// broche système, ou si l'une des deux plages n'est pas une plage audio exploitable.
    ///
    /// # Safety
    ///
    /// `client` et `my` pointent des `KSDATARANGE` suivies d'au moins `FormatSize` octets
    /// lisibles (contrat de `IMiniport::DataRangeIntersection`).
    pub(crate) unsafe fn resolve(
        &self,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        let (name, n) = (self.name, self.n);
        if pin_id != self.system_pin {
            // Les broches bridge sont analogiques : elles n'ont pas de format à négocier.
            kmd_log!("{name}{n}::DataRangeIntersection : broche {pin_id} sans format");
            return Err(STATUS_NO_MATCH);
        }
        // SAFETY: contrat relayé.
        let Some(notre) = (unsafe { read_our_range(my) }) else {
            kmd_log!(
                "{name}{n}::DataRangeIntersection : notre plage n'est pas une KSDATARANGE_AUDIO"
            );
            return Err(STATUS_NO_MATCH);
        };
        // SAFETY: contrat relayé.
        let Some(demande) = (unsafe { read_client_range(client) }) else {
            kmd_log!("{name}{n}::DataRangeIntersection : plage client non audio WAVEFORMATEX");
            return Err(STATUS_NO_MATCH);
        };

        let format = match intersect(&demande, &notre) {
            Ok(format) => format,
            Err(refus) => return Err(self.refuse(refus, demande)),
        };

        // Le garde-fou qui compte : ne **jamais** proposer au moteur audio ce que
        // `NewStream` refuserait ensuite. Les deux passent par la même liste — celle des
        // trois profondeurs du câble, à sa fréquence et sur ses canaux.
        let supportes = cable_formats(self.format.sample_rate, self.format.channels);
        if validate(&format.requested(), &supportes).is_err() {
            kmd_log!(
                "{name}{n}::DataRangeIntersection : {format:?} négocié mais hors des formats du \
                 câble — refusé plutôt que proposé"
            );
            return Err(self.refuse(NoMatch::HorsCatalogue, demande));
        }

        let taille = format.format_size();
        if let Some(out) = out {
            if !write_format(format, out) {
                // Tampon trop court : le thunk en fera un `STATUS_BUFFER_TOO_SMALL` à partir
                // de la taille rendue. C'est le protocole documenté, pas un échec.
                kmd_log!(
                    "{name}{n}::DataRangeIntersection : tampon de {} octets < {taille}",
                    out.len()
                );
                return Ok(taille);
            }
        }
        kmd_log!(
            "{name}{n}::DataRangeIntersection broche {pin_id} : {} canaux, {} Hz, {} bits{} \
             ({taille} octets)",
            format.channels(),
            format.sample_rate(),
            format.bits_per_sample(),
            if format.is_extensible() {
                " (WAVEFORMATEXTENSIBLE)"
            } else {
                ""
            }
        );
        Ok(taille)
    }

    /// Consigne un refus et rend `STATUS_NO_MATCH`.
    ///
    /// Seuls les refus **numériques** atteignent le journal d'événements, et une seule fois
    /// par miniport : voir l'en-tête de module sur ce tri, qui est ce qui sépare un
    /// fonctionnement normal d'un endpoint sans format.
    fn refuse(&self, refus: NoMatch, demande: AudioRange) -> NtStatus {
        let (name, n) = (self.name, self.n);
        kmd_log!("{name}{n}::DataRangeIntersection refusé : {refus} ; client {demande:?}");
        if refus.est_numerique() && !self.reported.swap(true, Ordering::Relaxed) {
            kmd_event!(
                self.log,
                registry::code::INTERSECTION
                    .saturating_add(registry::RANG_FORMAT)
                    .saturating_add(n),
                "{name}{n} : {refus}"
            );
        }
        STATUS_NO_MATCH
    }
}
