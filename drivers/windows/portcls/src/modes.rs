//! `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` (jeu `KSPROPSETID_AudioSignalProcessing`) :
//! trait [`SignalModes`] que le miniport WaveRT implémente, et le [`PropertyHandler`]
//! [`SignalProcessingModes`] posé sur la brique de [`crate::property`].
//!
//! **Ce module ne stocke aucun état**, comme [`crate::jack`] : il décode le numéro de
//! broche, écrit un `KSMULTIPLE_ITEM` suivi de zéro ou un GUID de mode, et demande au
//! miniport quelle broche est celle du flux.
//!
//! # Ce que « ne supporter que DEFAULT » demande de déclarer
//!
//! Deux choses, et la documentation Microsoft les sépare nettement :
//!
//! 1. **Un attribut sur les plages de la broche** (« Audio Signal Processing Modes ») :
//!    « *KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE is used by mode aware drivers with a
//!    KSDATARANGE structure which contain a KSATTRIBUTE_LIST. This list has a single
//!    element in it, which is a KSATTRIBUTE.* » L'attribut ne nomme **pas** le mode : il
//!    dit seulement « cette broche sait qu'un mode existe ». Cette moitié-là est dans
//!    `conduit_kmd::descriptors`, sur les plages elles-mêmes (flag `KSDATARANGE_ATTRIBUTES`
//!    et l'entrée de liste qui suit chaque plage) ;
//! 2. **Cette propriété-ci**, qui énumère les modes réellement servis. C'est elle, et elle
//!    seule, qui dit `AUDIO_SIGNALPROCESSINGMODE_DEFAULT`.
//!
//! Un pilote qui ne sert que le mode par défaut déclare donc **les deux** : l'attribut sans
//! valeur, et la liste à un élément. C'est exactement la construction de SYSVAD
//! (`SpeakerPinDataRangesStream` + `SpeakerHostPinSupportedDeviceModes`).
//!
//! # Une propriété du **filtre** qui parle d'une **broche**
//!
//! Même géométrie que `KSPROPERTY_JACK_DESCRIPTION`, et pour une fois la table
//! d'utilisation de la documentation le dit sans ambiguïté : « *Target : Pin factory (via
//! Filter instance)* », « *Property descriptor type : KSP_PIN* ». L'entrée
//! [`signal_processing_modes_item`] se pose donc dans la `PCAUTOMATION_TABLE` du **filtre
//! wave**, et le `PinId` arrive dans la queue de `KSP_PIN` que PortCls laisse dans
//! `Instance` — le décodage est celui de [`crate::jack::Pin::decode`], réemployé tel quel
//! plutôt que recopié : un second jeu de décalages qui divergerait d'un octet est la panne
//! silencieuse classique de ce dépôt.
//!
//! # La forme de la réponse
//!
//! Un `KSMULTIPLE_ITEM` suivi de `Count` GUID de mode. La documentation prescrit le tri
//! broche par broche : « *should advertise support only on non-loopback streaming pins*
//! […] *For loopback or bridge pins the audio driver should still support the property, but
//! return a KSMULTIPLE_ITEM structure with its Count parameter set to zero* ».
//!
//! | Broche | `Count` | `Size` | Contenu |
//! |---|---|---|---|
//! | broche de flux ([`SignalModes::streaming_pin`]) | 1 | 24 | `AUDIO_SIGNALPROCESSINGMODE_DEFAULT` |
//! | autre broche existante (bridge) | 0 | 8 | rien |
//! | broche inexistante | — | — | `STATUS_INVALID_PARAMETER` |
//!
//! # Lecture seule, et `BASICSUPPORT` obligatoire
//!
//! `Flags = GET | BASICSUPPORT` ([`MODES_ACCESS_FLAGS`], 513) : la table d'utilisation
//! donne « Get: Yes, Set: No ». Comme partout ailleurs, déclarer `BASICSUPPORT` fait de
//! nous le seul répondant à ce verbe, et la réponse est en paliers — 4 octets
//! (`AccessFlags`) puis 40 (`KSPROPERTY_DESCRIPTION`). La valeur étant de **taille
//! variable** (zéro ou un GUID), aucun membre n'est décrit et le `PropTypeSet` est nul,
//! comme pour le jack.

use conduit_com::NtStatus;
use portcls_sys::{
    AUDIO_SIGNALPROCESSINGMODE_DEFAULT, GUID, GUID_NULL, KSPROPERTY_AUDIOSIGNALPROCESSING,
    KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET, KSPROPSETID_AudioSignalProcessing,
    PCPROPERTY_ITEM,
};

use crate::jack;
use crate::property::{
    self, Champs, D_ACCESSFLAGS, PropertyHandler, Request, TAILLE_ACCESSFLAGS, TAILLE_DESCRIPTION,
    TargetVtbl, ecrire_description,
};
use crate::status::STATUS_BUFFER_TOO_SMALL;

/// `Flags` du `PCPROPERTY_ITEM` des modes : `GET | BASICSUPPORT` (513), **sans `SET`**.
///
/// La table d'utilisation de `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` donne « Get: Yes,
/// Set: No » : les modes sont une propriété du pilote, pas un réglage.
pub const MODES_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

const _: () = assert!(MODES_ACCESS_FLAGS == 513);

// ---------------------------------------------------------------------------------
// Décalages recopiés de portcls-sys/tests/layout.golden (oracle cl.exe), en absolu depuis
// le début du tampon `Value` : le `KSMULTIPLE_ITEM` d'abord, le GUID de mode ensuite.
// ---------------------------------------------------------------------------------

/// `KSMULTIPLE_ITEM::Size` (0) : taille **totale** de la réponse, en-tête compris.
const MI_SIZE: usize = 0;
/// `KSMULTIPLE_ITEM::Count` (4) : le nombre de modes.
const MI_COUNT: usize = 4;
/// `sizeof(KSMULTIPLE_ITEM)` (golden).
const TAILLE_MULTIPLE_ITEM: usize = 8;
/// `sizeof(GUID)` (golden).
const TAILLE_GUID: usize = 16;
/// Le GUID du mode, à la suite du `KSMULTIPLE_ITEM`.
const M_MODE: usize = TAILLE_MULTIPLE_ITEM;
/// Réponse complète d'une broche de flux : `KSMULTIPLE_ITEM` + un GUID.
const TAILLE_REPONSE_MODE: usize = TAILLE_MULTIPLE_ITEM + TAILLE_GUID;

const _: () = assert!(TAILLE_MULTIPLE_ITEM == 8 && TAILLE_GUID == 16);
const _: () = assert!(TAILLE_REPONSE_MODE == 24 && M_MODE == 8);

// ---------------------------------------------------------------------------------
// La broche visée.
// ---------------------------------------------------------------------------------

/// Ce que le `PinId` de la requête désigne, une fois décodé et validé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModePin {
    /// La broche de **flux** ([`SignalModes::streaming_pin`]) : la liste des modes.
    Flux,
    /// Une autre broche **existante** du filtre — chez nous la broche bridge : réponse
    /// vide (`KSMULTIPLE_ITEM` seul, `Count = 0`), comme le prescrit la documentation.
    Sans,
}

impl ModePin {
    /// Décode le `PinId: ULONG` en tête de `instance`, la queue de `KSP_PIN` que PortCls
    /// nous laisse.
    ///
    /// Délègue à [`jack::Pin::decode`] : c'est le **même** descripteur `KSP_PIN`, donc le
    /// même décalage, les mêmes tailles hostiles et les mêmes deux statuts d'erreur
    /// (`STATUS_INVALID_DEVICE_REQUEST` pour une instance tronquée,
    /// `STATUS_INVALID_PARAMETER` pour une broche inexistante). Seuls les **noms** changent
    /// : ce qui est « la broche à prise » pour le jack est « la broche de flux » ici.
    pub fn decode(instance: &[u8], pin_count: u32, streaming_pin: u32) -> Result<Self, NtStatus> {
        match jack::Pin::decode(instance, pin_count, streaming_pin)? {
            jack::Pin::Jack => Ok(Self::Flux),
            jack::Pin::Sans => Ok(Self::Sans),
        }
    }

    /// Nombre de GUID de mode que la réponse porte : 1 pour la broche de flux, 0 pour les
    /// autres.
    const fn modes(self) -> u32 {
        match self {
            Self::Flux => 1,
            Self::Sans => 0,
        }
    }

    /// Taille totale de la réponse, `KSMULTIPLE_ITEM` compris.
    const fn taille(self) -> usize {
        match self {
            Self::Flux => TAILLE_REPONSE_MODE,
            Self::Sans => TAILLE_MULTIPLE_ITEM,
        }
    }
}

// ---------------------------------------------------------------------------------
// Trace.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire vient de faire d'une requête de modes, passé à
/// [`SignalModes::trace`].
///
/// Même rôle que [`crate::jack::JackTrace`] : `instance` porte les octets **bruts** de la
/// queue de `KSP_PIN`, `pin` ce qu'on en a tiré. Attendu sur un filtre wave Conduit : la
/// broche système en [`ModePin::Flux`], la broche bridge en [`ModePin::Sans`].
#[derive(Debug)]
pub struct ModesTrace<'a> {
    /// Le verbe : `"GET"` ou `"BASICSUPPORT"`.
    pub verb: &'static str,
    /// Les octets d'`Instance` tels que PortCls les a laissés : la queue de `KSP_PIN`,
    /// normalement 8 octets, `PinId` d'abord.
    pub instance: &'a [u8],
    /// La broche décodée de ces octets, ou le statut d'erreur rendu à l'appelant.
    pub pin: Result<ModePin, NtStatus>,
}

/// Nom de verbe des traces de lecture.
const VERBE_GET: &str = "GET";
/// Nom de verbe des traces de description.
const VERBE_BASICSUPPORT: &str = "BASICSUPPORT";

// ---------------------------------------------------------------------------------
// Le trait métier.
// ---------------------------------------------------------------------------------

/// Les modes de traitement du signal d'un miniport wave, vus par le gestionnaire de
/// propriété.
///
/// Il n'y a **rien à choisir** : Conduit ne sert que `AUDIO_SIGNALPROCESSINGMODE_DEFAULT`,
/// et le trait ne demande donc pas la liste — seulement de quelles broches elle parle. Le
/// jour où un second mode existerait, c'est ici qu'il s'ajouterait.
///
/// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne dans le contexte du fil
/// appelant). Appels concurrents possibles depuis plusieurs fils.
pub trait SignalModes: Send + Sync + 'static {
    /// Nombre de broches du filtre, tel que son `PCFILTER_DESCRIPTOR` le déclare.
    ///
    /// Sert à distinguer « broche existante sans mode » (réponse vide, `STATUS_SUCCESS`)
    /// de « broche inexistante » (`STATUS_INVALID_PARAMETER`). Doit s'accorder avec
    /// `PCFILTER_DESCRIPTOR::PinCount`.
    fn pin_count(&self) -> u32;

    /// Numéro de la broche de **flux** : celle que le moteur audio ouvre, la seule qui
    /// porte des plages de format, donc la seule qui ait un mode.
    ///
    /// Chez nous : `WAVE_RENDER_PIN_SYSTEM` au rendu, `WAVE_CAPTURE_PIN_SYSTEM` à la
    /// capture. Se tromper de broche ici ne casse rien de visible : la propriété répond,
    /// mais annonce zéro mode sur la broche que le moteur interroge — et le pilote reste
    /// exactement aussi peu « mode aware » qu'avant.
    fn streaming_pin(&self) -> u32;

    /// Point de trace, appelé une fois par requête, après coup.
    ///
    /// Défaut : ne fait rien. Ne doit ni allouer ni bloquer.
    fn trace(&self, trace: &ModesTrace<'_>) {
        let _ = trace;
    }
}

// ---------------------------------------------------------------------------------
// Sérialisation.
// ---------------------------------------------------------------------------------

/// Écrit le `KSMULTIPLE_ITEM` puis, si `pin` est la broche de flux, le GUID du mode.
///
/// **Tout ou rien**, comme [`crate::jack`] : si `value` est plus court que la réponse, rien
/// n'est écrit et le thunk rendra `STATUS_BUFFER_TOO_SMALL` avec la taille requise. Une
/// réponse à demi écrite serait pire qu'aucune : son `KSMULTIPLE_ITEM` annoncerait un mode
/// absent du tampon.
fn ecrire_reponse(value: &mut [u8], pin: ModePin) {
    if value.len() < pin.taille() {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(MI_SIZE, pin.taille() as u32);
    champs.u32(MI_COUNT, pin.modes());
    if pin == ModePin::Sans {
        return;
    }
    champs.guid(M_MODE, &AUDIO_SIGNALPROCESSINGMODE_DEFAULT);
}

/// Réponse à `KSPROPERTY_TYPE_BASICSUPPORT`, en paliers (voir la documentation du module).
///
/// Renvoie les octets **écrits**, jamais une taille requise : c'est l'asymétrie documentée
/// sur [`PropertyHandler`].
fn basic_support_ks(value: &mut [u8]) -> Result<u32, NtStatus> {
    if value.len() < TAILLE_ACCESSFLAGS {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let mut champs = Champs { dest: value };
    if champs.dest.len() < TAILLE_DESCRIPTION {
        // Premier appel de KS : `sizeof(ULONG)`, les seuls `AccessFlags`.
        champs.u32(D_ACCESSFLAGS, MODES_ACCESS_FLAGS);
        return Ok(TAILLE_ACCESSFLAGS as u32);
    }

    // Valeur de taille variable (zéro ou un GUID) : aucun membre, `PropTypeSet` nul —
    // l'équivalent du `VT_ILLEGAL` de SYSVAD, exactement comme pour le jack.
    ecrire_description(
        &mut champs,
        MODES_ACCESS_FLAGS,
        TAILLE_DESCRIPTION as u32,
        &GUID_NULL,
        0,
        0,
    );
    Ok(TAILLE_DESCRIPTION as u32)
}

// ---------------------------------------------------------------------------------
// Le gestionnaire.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` sur la table d'automatisation d'un filtre
/// wave : les modes de traitement du signal de la broche désignée par `Instance`.
#[derive(Debug)]
pub struct SignalProcessingModes;

impl<T: SignalModes> PropertyHandler<T> for SignalProcessingModes {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let broche = ModePin::decode(
            req.instance,
            req.target.pin_count(),
            req.target.streaming_pin(),
        );
        if let Ok(pin) = broche {
            ecrire_reponse(value, pin);
        }
        req.target.trace(&ModesTrace {
            verb: VERBE_GET,
            instance: req.instance,
            pin: broche,
        });
        // Taille **requise**, écrite ou non : contrat de `get`.
        broche.map(|pin| pin.taille() as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Le numéro de broche est validé **avant** le verbe, comme pour le jack : la
        // documentation veut que le gestionnaire de support de base reçoive lui aussi un
        // `KSP_PIN`.
        let broche = ModePin::decode(
            req.instance,
            req.target.pin_count(),
            req.target.streaming_pin(),
        );
        let ecrits = broche.and_then(|_| basic_support_ks(value));
        req.target.trace(&ModesTrace {
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            pin: broche,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Entrée de table prête à poser.
// ---------------------------------------------------------------------------------

/// `KSPROPSETID_AudioSignalProcessing` en `static` : `PCPROPERTY_ITEM::Set` veut une
/// adresse `'static`, et les GUID de `portcls-sys` sont des `const`.
static SET_SIGNAL_PROCESSING: GUID = KSPROPSETID_AudioSignalProcessing;

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** wave :
/// `KSPROPSETID_AudioSignalProcessing`, `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES`,
/// `GET | BASICSUPPORT`.
///
/// À poser dans `PCFILTER_DESCRIPTOR::AutomationTable`, pas sur une broche (voir la
/// documentation du module). `V` est la vtable du miniport qui porte la table
/// (`IMiniportWaveRTVtbl`), `T` son type — c'est ce couple que la garde de vtable de
/// [`crate::property::handler`] vérifie.
pub const fn signal_processing_modes_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: SignalModes,
{
    property::item::<V, T, SignalProcessingModes>(
        &SET_SIGNAL_PROCESSING,
        KSPROPERTY_AUDIOSIGNALPROCESSING::KSPROPERTY_AUDIOSIGNALPROCESSING_MODES as u32,
        MODES_ACCESS_FLAGS,
    )
}

#[cfg(test)]
mod tests {
    // Tests en mode utilisateur : les lints anti-panique du noyau y sont sans objet, une
    // assertion fausse doit arrêter le test.
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used
    )]

    use super::*;
    use crate::status::STATUS_INVALID_DEVICE_REQUEST;
    use conduit_com::STATUS_INVALID_PARAMETER;

    /// Les tailles sont bien celles du golden, et la réponse complète en est la somme.
    #[test]
    fn les_tailles_sont_celles_du_golden() {
        assert_eq!(TAILLE_MULTIPLE_ITEM, 8);
        assert_eq!(TAILLE_GUID, core::mem::size_of::<GUID>());
        assert_eq!(TAILLE_REPONSE_MODE, TAILLE_MULTIPLE_ITEM + TAILLE_GUID);
        assert_eq!(TAILLE_DESCRIPTION, 40);
    }

    /// `GET | BASICSUPPORT`, et surtout **pas** `SET` (« Get: Yes, Set: No »).
    #[test]
    fn la_propriete_est_en_lecture_seule() {
        assert_eq!(MODES_ACCESS_FLAGS, 513);
        assert_eq!(MODES_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET, 0);
    }

    /// Décodage de la broche : les quatre cas, aux deux sens.
    #[test]
    fn decodage_de_la_broche() {
        let instance = |pin: u32| pin.to_ne_bytes();
        // Filtre wave rendu : la broche de flux est l'entrée (0).
        assert_eq!(ModePin::decode(&instance(0), 2, 0), Ok(ModePin::Flux));
        assert_eq!(ModePin::decode(&instance(1), 2, 0), Ok(ModePin::Sans));
        // Filtre wave capture : la broche de flux est la sortie (1).
        assert_eq!(ModePin::decode(&instance(1), 2, 1), Ok(ModePin::Flux));
        assert_eq!(ModePin::decode(&instance(0), 2, 1), Ok(ModePin::Sans));
        // Broche inexistante.
        assert_eq!(
            ModePin::decode(&instance(2), 2, 0),
            Err(STATUS_INVALID_PARAMETER)
        );
        // Instance absente ou tronquée : requête mal formée, statut différent.
        assert_eq!(
            ModePin::decode(&[], 2, 0),
            Err(STATUS_INVALID_DEVICE_REQUEST)
        );
        assert_eq!(
            ModePin::decode(&[0, 0, 0], 2, 0),
            Err(STATUS_INVALID_DEVICE_REQUEST)
        );
    }

    /// Le compte annoncé et la taille annoncée se déduisent l'un de l'autre.
    #[test]
    fn le_compte_et_la_taille_s_accordent() {
        assert_eq!(ModePin::Flux.modes(), 1);
        assert_eq!(ModePin::Sans.modes(), 0);
        assert_eq!(
            ModePin::Flux.taille(),
            TAILLE_MULTIPLE_ITEM + ModePin::Flux.modes() as usize * TAILLE_GUID
        );
        assert_eq!(
            ModePin::Sans.taille(),
            TAILLE_MULTIPLE_ITEM + ModePin::Sans.modes() as usize * TAILLE_GUID
        );
    }

    /// Le GUID écrit est bien celui du mode par défaut de `ksmedia.h`, et pas celui du
    /// mode brut — les deux se ressemblent à la relecture et rien d'autre ne les
    /// distinguerait.
    #[test]
    fn le_mode_ecrit_est_default_et_pas_raw() {
        let mut tampon = [0u8; TAILLE_REPONSE_MODE];
        ecrire_reponse(&mut tampon, ModePin::Flux);
        let attendu = AUDIO_SIGNALPROCESSINGMODE_DEFAULT;
        assert_eq!(
            u32::from_ne_bytes(tampon[M_MODE..M_MODE + 4].try_into().unwrap()),
            attendu.Data1
        );
        assert_ne!(
            attendu.Data1,
            portcls_sys::AUDIO_SIGNALPROCESSINGMODE_RAW.Data1
        );
        assert_eq!(&tampon[M_MODE + 8..M_MODE + 16], &attendu.Data4);
    }
}
