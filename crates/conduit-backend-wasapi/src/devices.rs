//! Traduction d'un endpoint MMDevice en [`DeviceInfo`].
//!
//! Tout ici s'exécute sur le fil MMDevice (voir `mmdevice_thread`). Les fonctions
//! pures — analyse d'un `WAVEFORMATEX`, arrondi de la période, reconnaissance des
//! noms de câbles — sont séparées des appels COM pour être testables sans matériel.
//!
//! # Ce que décrit `DeviceInfo`
//!
//! `channels` et `sample_rate` sont ceux du **format de mixage** du moteur audio
//! (`IAudioClient::GetMixFormat`) : c'est ce qu'un flux en mode partagé délivre sans
//! conversion, et donc le chemin basse latence de `open` (tranché en M1b-31). Le
//! format du périphérique lui-même (`PKEY_AudioEngine_DeviceFormat`, par exemple un
//! micro mono que Windows mixe en stéréo) n'est qu'un repli si `GetMixFormat` échoue.
//! `sample_rates` liste 44,1/48/96 kHz : en mode partagé, Windows convertit
//! automatiquement (`AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`) tout format demandé qui
//! diffère du mixage. `default_block` est la période par défaut du moteur en trames.

use conduit_backend::{BackendError, CableId, DeviceDirection, DeviceId, DeviceInfo};
use conduit_core::types::SampleRate;
use conduit_kmd_core::config::{ConfigGuid, KSPROPSETID_CONDUIT, PID_MARQUE_CABLE};
use windows::core::HRESULT;
use windows::core::{Interface, GUID};
use windows::Win32::Devices::FunctionDiscovery::{
    PKEY_Device_DeviceDesc, PKEY_Device_FriendlyName,
};
use windows::Win32::Foundation::{ERROR_NOT_FOUND, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    IMMEndpoint, PKEY_AudioEngine_DeviceFormat, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::com::{platform_error, CoTaskMem, CoTaskString, PropVariant};
use crate::convert::SampleType;

/// Fréquences annoncées dans [`DeviceInfo::sample_rates`] pour tout périphérique
/// dont l'`IAudioClient` s'active : le mode partagé les accepte toutes, par
/// conversion automatique quand elles diffèrent du format de mixage.
pub const PROBED_RATES: [SampleRate; 3] = [
    SampleRate::HZ_44100,
    SampleRate::HZ_48000,
    SampleRate::HZ_96000,
];

/// Période par défaut du moteur audio Windows (10 ms), en unités de 100 ns : le
/// repli quand `IAudioClient::GetDevicePeriod` échoue.
pub(crate) const DEFAULT_PERIOD_HNS: i64 = 100_000;

/// Unités de 100 ns dans une seconde.
const HNS_PER_SECOND: i64 = 10_000_000;

/// `E_NOTFOUND` tel que le rend MMDevice : `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`,
/// pour « aucun périphérique par défaut » et « identifiant inconnu ».
const E_NOTFOUND: HRESULT = HRESULT::from_win32(ERROR_NOT_FOUND.0);

/// `WAVE_FORMAT_EXTENSIBLE` (mmreg.h) : le format porte un `SubFormat`.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `WAVE_FORMAT_IEEE_FLOAT` (mmreg.h) : flottants sans `SubFormat`.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;

/// `WAVE_FORMAT_PCM` (mmreg.h) : entiers signés sans `SubFormat`.
const WAVE_FORMAT_PCM: u16 = 0x0001;

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` (ksmedia.h) : échantillons flottants.
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// `KSDATAFORMAT_SUBTYPE_PCM` (ksmedia.h) : échantillons entiers signés — le
/// sous-format que le matériel accepte le plus souvent en mode exclusif.
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

/// `SPEAKER_FRONT_CENTER` / `SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT` (ksmedia.h).
const MASK_MONO: u32 = 0x4;
const MASK_STEREO: u32 = 0x3;

/// Taille de `WAVEFORMATEX` (champs `cbSize` compris).
pub(crate) const WAVEFORMATEX_SIZE: usize = 18;
/// Taille de `WAVEFORMATEXTENSIBLE`.
const WAVEFORMATEXTENSIBLE_SIZE: usize = 40;

/// Ce que l'on retient d'un `WAVEFORMATEX` : canaux, fréquence, disposition,
/// nature des échantillons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaveFormat {
    pub(crate) channels: u16,
    pub(crate) sample_rate: u32,
    /// Octets d'une trame (`nBlockAlign`) : sert à lire les positions que
    /// `IAudioClock` exprime en octets (module `clock`).
    pub(crate) block_align: u16,
    /// `dwChannelMask` si le format est extensible.
    pub(crate) channel_mask: Option<u32>,
    /// Bits du conteneur d'un échantillon (`wBitsPerSample`).
    pub(crate) bits: u16,
    /// Bits significatifs (`wValidBitsPerSample`) ; égal à `bits` hors format
    /// extensible.
    pub(crate) valid_bits: u16,
    /// Échantillons `f32` (tag `WAVE_FORMAT_IEEE_FLOAT` ou sous-format IEEE float,
    /// 32 bits) : le format que Conduit consomme tel quel.
    pub(crate) float32: bool,
    /// Échantillons entiers signés (tag `WAVE_FORMAT_PCM` ou sous-format PCM) : ce
    /// que le matériel propose le plus souvent en mode exclusif.
    pub(crate) pcm: bool,
}

impl WaveFormat {
    /// Masque de haut-parleurs à publier pour ce nombre de canaux : celui du
    /// format s'il en a un et si le nombre de canaux est le même, sinon les
    /// dispositions mono/stéréo standard, sinon « non spécifié » (0).
    pub(crate) fn mask_for(&self, channels: u16) -> u32 {
        match self.channel_mask {
            Some(mask) if channels == self.channels => mask,
            _ => default_mask(channels),
        }
    }

    /// Type d'échantillon correspondant, si Conduit sait le convertir ; `None`
    /// pour tout le reste (PCM 8 bits, 20 bits dans 24, float64…).
    pub(crate) fn sample_type(&self) -> Option<SampleType> {
        if self.float32 {
            return Some(SampleType::F32);
        }
        if !self.pcm {
            return None;
        }
        match (self.bits, self.valid_bits) {
            (32, 24) => Some(SampleType::Pcm24In32),
            (24, 24) => Some(SampleType::Pcm24),
            (16, 16) => Some(SampleType::Pcm16),
            _ => None,
        }
    }
}

/// Masque de haut-parleurs standard pour mono et stéréo ; 0 (« laisser Windows
/// choisir ») au-delà.
pub(crate) fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => MASK_MONO,
        2 => MASK_STEREO,
        _ => 0,
    }
}

/// Lit canaux, fréquence, masque et nature des échantillons dans l'image mémoire
/// d'un `WAVEFORMATEX` (éventuellement `WAVEFORMATEXTENSIBLE`), telle que la
/// publient `PKEY_AudioEngine_DeviceFormat` et `GetMixFormat`.
///
/// La structure est `packed(1)` : on lit les champs octet par octet plutôt que de
/// transtyper, ce qui évite tout problème d'alignement. `None` si le bloc est trop
/// court ou annonce zéro canal ou zéro hertz.
pub(crate) fn wave_format_from_bytes(bytes: &[u8]) -> Option<WaveFormat> {
    if bytes.len() < WAVEFORMATEX_SIZE {
        return None;
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let tag = u16_at(0);
    let channels = u16_at(2);
    let sample_rate = u32_at(4);
    let block_align = u16_at(12);
    let bits = u16_at(14);
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let extensible = tag == WAVE_FORMAT_EXTENSIBLE && bytes.len() >= WAVEFORMATEXTENSIBLE_SIZE;
    let channel_mask = extensible.then(|| u32_at(20));
    // `wValidBitsPerSample` occupe l'union `Samples`, juste après `cbSize`.
    let valid_bits = if extensible { u16_at(18) } else { bits };
    let sub_format = extensible.then(|| {
        let mut data4 = [0u8; 8];
        data4.copy_from_slice(&bytes[32..40]);
        GUID {
            data1: u32_at(24),
            data2: u16_at(28),
            data3: u16_at(30),
            data4,
        }
    });
    let float32 = bits == 32
        && match sub_format {
            Some(sub) => sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            None => tag == WAVE_FORMAT_IEEE_FLOAT,
        };
    let pcm = match sub_format {
        Some(sub) => sub == KSDATAFORMAT_SUBTYPE_PCM,
        None => tag == WAVE_FORMAT_PCM,
    };
    Some(WaveFormat {
        channels,
        sample_rate,
        block_align,
        channel_mask,
        bits,
        valid_bits: if valid_bits == 0 { bits } else { valid_bits },
        float32,
        pcm,
    })
}

/// Construit un `WAVEFORMATEXTENSIBLE` entrelacé au type d'échantillon voulu : le
/// format que Conduit demande à WASAPI (sondage en M1b-30, flux en mode partagé
/// depuis M1b-31, négociation exclusive depuis M1b-32).
///
/// Toujours extensible, même en mono ou stéréo : c'est la forme que le mode
/// exclusif attend, elle porte `wValidBitsPerSample` (indispensable au PCM 24 dans
/// un conteneur 32) et le masque de haut-parleurs.
pub(crate) fn hardware_format(
    sample: SampleType,
    channels: u16,
    rate: SampleRate,
    mask: u32,
) -> WAVEFORMATEXTENSIBLE {
    let block_align = u16::try_from(sample.frame_bytes(usize::from(channels))).unwrap_or(u16::MAX);
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: channels,
            nSamplesPerSec: rate.hz(),
            nAvgBytesPerSec: rate.hz().saturating_mul(u32::from(block_align)),
            nBlockAlign: block_align,
            wBitsPerSample: sample.bits(),
            cbSize: (WAVEFORMATEXTENSIBLE_SIZE - WAVEFORMATEX_SIZE) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: sample.valid_bits(),
        },
        dwChannelMask: mask,
        SubFormat: if sample.is_pcm() {
            KSDATAFORMAT_SUBTYPE_PCM
        } else {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        },
    }
}

/// Convertit une période en unités de 100 ns (`GetDevicePeriod`) en trames à la
/// fréquence donnée, arrondi au plus proche, au moins 1.
pub(crate) fn frames_from_period(period_hns: i64, rate: SampleRate) -> usize {
    let period = i128::from(period_hns.max(0));
    let frames = (period * i128::from(rate.hz()) + i128::from(HNS_PER_SECOND / 2))
        / i128::from(HNS_PER_SECOND);
    usize::try_from(frames).unwrap_or(usize::MAX).max(1)
}

/// Reconnaît le nom d'un côté de câble Conduit : exactement `Conduit <n>` avec `n`
/// décimal sans zéro de tête ni signe (`Conduit 1` → 1 ; `Conduit 01`, `conduit 1`,
/// `Conduit`, `Conduit 1 (2)` → `None`).
///
/// Cette sévérité est voulue : ce parseur reçoit la **description** d'un endpoint
/// (`PKEY_Device_DeviceDesc`), qui porte le nom d'endpoint seul. Le nom composé que
/// Windows affiche — « Conduit 1 (Conduit — câbles audio virtuels) » — n'est pas un
/// identifiant et n'a rien à faire ici : voir [`cable_id_from_endpoint`].
pub fn cable_id_from_name(name: &str) -> Option<CableId> {
    let digits = name.strip_prefix("Conduit ")?;
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0).map(CableId)
}

/// Traduit un [`ConfigGuid`] du contrat en `GUID` de `guiddef.h`.
///
/// Les deux structures ont la même disposition (`conduit_kmd_core` l'affirme par
/// assertion `const`) ; on recopie les quatre champs plutôt que de transtyper, ce qui
/// reste une `const fn` et n'engage aucun `unsafe`.
const fn guid_from_config(guid: &ConfigGuid) -> GUID {
    GUID {
        data1: guid.data1,
        data2: guid.data2,
        data3: guid.data3,
        data4: guid.data4,
    }
}

/// Le `PROPERTYKEY` de la **marque de câble** que le service écrit sur un endpoint
/// avant de le renommer : `{3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b},1`.
///
/// Le `fmtid` est [`KSPROPSETID_CONDUIT`] et le `pid` [`PID_MARQUE_CABLE`], tous deux
/// **pris dans `conduit-kmd-core`** et non recopiés : le service d'assistance écrit
/// exactement cette valeur dans le registre (`conduit_helper::registre`), et deux
/// écritures en dur du même GUID finiraient par diverger en silence — la marque
/// deviendrait introuvable, sans le moindre message.
///
/// Elle se lit dans le magasin de propriétés de l'endpoint, comme les autres clés : ce
/// crate n'a pas à connaître le chemin `HKLM` sous lequel Windows range ce magasin.
pub(crate) const MARQUE_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: guid_from_config(&KSPROPSETID_CONDUIT),
    pid: PID_MARQUE_CABLE,
};

/// Identité de câble d'un endpoint, à partir de ce que sa clé porte : notre **marque**
/// d'abord, la description ensuite, le nom en dernier recours.
///
/// # Pourquoi la marque prime
///
/// Windows **compose** le nom convivial (`PKEY_Device_FriendlyName`) : le nom de
/// l'endpoint suivi de celui du périphérique qui le porte. Relevé dans la machine
/// virtuelle sur les deux côtés du câble 1, avant tout renommage :
///
/// ```text
/// PKEY_Device_FriendlyName : Conduit 1 (Conduit — câbles audio virtuels)
/// PKEY_Device_DeviceDesc   : Conduit 1
/// ```
///
/// La description identifiait donc le câble — **tant que personne ne renomme**. Or
/// renommer un câble écrit précisément dans cette description : après
/// `conduit-helper renommer 1 Musique`, elle vaut « Musique » et le pont est rompu. Le
/// service écrit pour cette raison, dans la même clé et **avant** la description, une
/// valeur qui lui appartient et qui dit ce que la description disait ([`MARQUE_KEY`],
/// « Conduit 1 »).
///
/// C'est la **même famille de défaut** que le nom composé : un identifiant déduit d'un
/// texte d'affichage. Le texte d'affichage change ; l'identité, non. La marque est donc
/// consultée en premier, et un endpoint renommé « Conduit 5 » alors qu'il est le câble 3
/// se reconnaît comme le **3** — c'est sa marque qui compte, pas ce qu'il affiche.
///
/// # Ce qui ne retombe pas sur le repli
///
/// Une marque **présente mais illisible** ne fait pas relire la description : quelqu'un a
/// écrit dans notre valeur, et deviner à sa place vaudrait moins que ne rien faire.
/// C'est la règle exacte de `conduit_helper::registre::cable_designe`, du côté qui écrit ;
/// le test `nos_lectures_de_marque_coincident` la vérifie des deux côtés à la fois.
///
/// Sans marque **ni** description, on retombe sur le nom : sur un endpoint dont Windows
/// ne compose pas le nom, il suffit ; sur un nom composé, [`cable_id_from_name`] le
/// refuse, ce qui est le bon résultat — mieux vaut « pas de câble » qu'un câble deviné.
pub fn cable_id_from_endpoint(
    marque: Option<&str>,
    description: Option<&str>,
    name: &str,
) -> Option<CableId> {
    match marque {
        Some(marque) => cable_id_from_name(marque),
        None => cable_id_from_name(description.unwrap_or(name)),
    }
}

/// Le nom d'un câble, tel que ses deux endpoints le portent.
///
/// Le nom **affiché** d'un câble n'est connu que de Windows : le service ne transporte
/// que des numéros, et `CableControl::list` rend donc le nom canonique « Conduit *N* ».
/// C'est le dorsal qui lit les deux descriptions et tranche — voir
/// [`WasapiBackend::resoudre_endpoints`](crate::WasapiBackend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableName<'a> {
    /// Aucun des deux sens n'a publié de nom : on garde celui du service.
    Inconnu,
    /// Les deux sens s'accordent, ou un seul est publié.
    Accord(&'a str),
    /// Les deux sens portent des noms **différents** : un renommage à moitié fait, que
    /// `registre::appliquer` ne produit pas (il exige les deux côtés avant d'écrire) mais
    /// qu'une interruption ou une modification manuelle peut laisser.
    Divergent {
        /// Celui qu'on retient : le rendu.
        retenu: &'a str,
        /// L'autre, celui du côté capture, journalisé et non retenu.
        ecarte: &'a str,
    },
}

impl<'a> CableName<'a> {
    /// Le nom à publier, s'il y en a un.
    #[must_use]
    pub const fn retenu(self) -> Option<&'a str> {
        match self {
            Self::Inconnu => None,
            Self::Accord(nom) | Self::Divergent { retenu: nom, .. } => Some(nom),
        }
    }
}

/// Le nom d'un câble d'après les descriptions de ses deux endpoints.
///
/// Le **rendu** l'emporte quand les deux divergent : c'est le côté où les applications
/// jouent, celui que l'utilisateur voit en premier dans les réglages Son de Windows, et
/// il fallait trancher de façon reproductible plutôt que selon l'ordre d'énumération.
/// L'appelant journalise la divergence — la taire ferait passer un renommage à moitié
/// fait pour un état normal.
#[must_use]
pub fn cable_name<'a>(render: Option<&'a str>, capture: Option<&'a str>) -> CableName<'a> {
    match (render, capture) {
        (Some(rendu), Some(capture)) if rendu != capture => CableName::Divergent {
            retenu: rendu,
            ecarte: capture,
        },
        (Some(nom), _) | (None, Some(nom)) => CableName::Accord(nom),
        (None, None) => CableName::Inconnu,
    }
}

/// Identifiants des périphériques par défaut (`eConsole`) au moment de
/// l'énumération.
#[derive(Debug, Default, Clone)]
pub(crate) struct Defaults {
    pub(crate) render: Option<String>,
    pub(crate) capture: Option<String>,
}

impl Defaults {
    /// Interroge l'énumérateur pour les deux sens.
    pub(crate) fn query(enumerator: &IMMDeviceEnumerator) -> Result<Self, BackendError> {
        Ok(Self {
            render: default_endpoint_id(enumerator, DeviceDirection::Render)?,
            capture: default_endpoint_id(enumerator, DeviceDirection::Capture)?,
        })
    }

    fn id_for(&self, direction: DeviceDirection) -> Option<&str> {
        match direction {
            DeviceDirection::Render => self.render.as_deref(),
            DeviceDirection::Capture => self.capture.as_deref(),
        }
    }
}

fn data_flow(direction: DeviceDirection) -> EDataFlow {
    match direction {
        DeviceDirection::Render => eRender,
        DeviceDirection::Capture => eCapture,
    }
}

/// Sens d'un `EDataFlow` (`None` pour `eAll`, qui ne désigne pas un endpoint).
pub(crate) fn direction_from_flow(flow: EDataFlow) -> Option<DeviceDirection> {
    if flow == eRender {
        Some(DeviceDirection::Render)
    } else if flow == eCapture {
        Some(DeviceDirection::Capture)
    } else {
        None
    }
}

/// Identifiant de l'endpoint par défaut (`eConsole`) d'un sens ; `None` s'il n'y a
/// aucun périphérique de ce sens.
pub(crate) fn default_endpoint_id(
    enumerator: &IMMDeviceEnumerator,
    direction: DeviceDirection,
) -> Result<Option<String>, BackendError> {
    // SAFETY: l'énumérateur est une interface COM valide, créée sur ce fil.
    let device = match unsafe { enumerator.GetDefaultAudioEndpoint(data_flow(direction), eConsole) }
    {
        Ok(device) => device,
        // E_NOTFOUND : aucun périphérique de ce sens n'est actif.
        Err(e) if e.code() == E_NOTFOUND => return Ok(None),
        Err(e) => {
            return Err(platform_error(
                "IMMDeviceEnumerator::GetDefaultAudioEndpoint",
                &e,
            ))
        }
    };
    endpoint_id(&device).map(Some)
}

/// Un endpoint tel que ce module l'a lu : ce que le trait [`Backend`] publie, et la
/// **description** que Windows range à part.
///
/// [`DeviceInfo::name`] est le nom **composé** que Windows affiche (« Musique (Conduit —
/// câbles audio virtuels) ») ; `description` est le nom d'endpoint seul (« Musique »),
/// c'est-à-dire celui que le renommage de M1b-21 écrit et celui qu'un câble doit
/// remonter dans `CableInfo::name`. Le trait `Backend` est portable et n'a pas à gagner
/// cette notion Windows, d'où un type interne au crate plutôt qu'un champ de plus sur
/// [`DeviceInfo`].
///
/// [`Backend`]: conduit_backend::Backend
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndpointInfo {
    /// Ce que le dorsal publie.
    pub(crate) info: DeviceInfo,
    /// `PKEY_Device_DeviceDesc` : le nom d'endpoint seul, `None` si Windows n'en range
    /// pas (l'endpoint n'a alors que son nom composé).
    pub(crate) description: Option<String>,
}

/// Énumère les endpoints actifs des deux sens.
pub(crate) fn enumerate(
    enumerator: &IMMDeviceEnumerator,
) -> Result<Vec<EndpointInfo>, BackendError> {
    let defaults = Defaults::query(enumerator)?;
    let mut devices = Vec::new();
    for direction in [DeviceDirection::Render, DeviceDirection::Capture] {
        // SAFETY: l'énumérateur est une interface COM valide, créée sur ce fil.
        let collection =
            unsafe { enumerator.EnumAudioEndpoints(data_flow(direction), DEVICE_STATE_ACTIVE) }
                .map_err(|e| platform_error("IMMDeviceEnumerator::EnumAudioEndpoints", &e))?;
        // SAFETY: collection valide, rendue par l'appel précédent.
        let count = unsafe { collection.GetCount() }
            .map_err(|e| platform_error("IMMDeviceCollection::GetCount", &e))?;
        for index in 0..count {
            // SAFETY: `index < count`, la collection est figée à sa création.
            let device = unsafe { collection.Item(index) }
                .map_err(|e| platform_error("IMMDeviceCollection::Item", &e))?;
            devices.push(describe(&device, &defaults)?);
        }
    }
    Ok(devices)
}

/// Retrouve un endpoint **actif** par son identifiant ; `None` s'il n'existe pas ou
/// n'est pas actif.
pub(crate) fn find_active(
    enumerator: &IMMDeviceEnumerator,
    id: &str,
) -> Result<Option<IMMDevice>, BackendError> {
    let wide: Vec<u16> = id.encode_utf16().chain(core::iter::once(0)).collect();
    // SAFETY: `wide` est terminé par NUL et vit pendant tout l'appel.
    let device = match unsafe { enumerator.GetDevice(windows::core::PCWSTR(wide.as_ptr())) } {
        Ok(device) => device,
        Err(e) if e.code() == E_NOTFOUND => return Ok(None),
        Err(e) => return Err(platform_error("IMMDeviceEnumerator::GetDevice", &e)),
    };
    // SAFETY: interface valide rendue par `GetDevice`.
    let state =
        unsafe { device.GetState() }.map_err(|e| platform_error("IMMDevice::GetState", &e))?;
    Ok((state == DEVICE_STATE_ACTIVE).then_some(device))
}

/// Décrit un seul endpoint par son identifiant ; `None` s'il n'existe pas ou n'est
/// pas actif.
pub(crate) fn describe_id(
    enumerator: &IMMDeviceEnumerator,
    id: &str,
) -> Result<Option<EndpointInfo>, BackendError> {
    let Some(device) = find_active(enumerator, id)? else {
        return Ok(None);
    };
    let defaults = Defaults::query(enumerator)?;
    describe(&device, &defaults).map(Some)
}

/// Active l'`IAudioClient` d'un endpoint.
pub(crate) fn activate_client(device: &IMMDevice) -> Result<IAudioClient, BackendError> {
    // SAFETY: interface valide ; aucun paramètre d'activation.
    unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }
        .map_err(|e| platform_error("IMMDevice::Activate(IAudioClient)", &e))
}

/// Identifiant d'endpoint (`IMMDevice::GetId`), copié en `String`.
fn endpoint_id(device: &IMMDevice) -> Result<String, BackendError> {
    // SAFETY: interface COM valide ; la chaîne rendue est allouée par COM et sa
    // propriété nous revient, `CoTaskString` la libère.
    let raw = unsafe { device.GetId() }.map_err(|e| platform_error("IMMDevice::GetId", &e))?;
    // SAFETY: `GetId` a réussi : `raw` est une chaîne `CoTaskMemAlloc` terminée par NUL.
    let id = unsafe { CoTaskString::from_raw(raw) };
    id.to_string().ok_or_else(|| {
        BackendError::Platform("IMMDevice::GetId a rendu une chaîne vide ou invalide".into())
    })
}

/// Lit une propriété de l'endpoint.
fn property(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    what: &str,
) -> Result<PropVariant, BackendError> {
    // SAFETY: magasin de propriétés valide ; la valeur rendue nous appartient et
    // `PropVariant` la nettoie.
    let value = unsafe { store.GetValue(key) }
        .map_err(|e| platform_error(&format!("IPropertyStore::GetValue({what})"), &e))?;
    Ok(PropVariant::from_owned(value))
}

/// Lit une propriété **texte** de l'endpoint ; `None` si la lecture échoue, si la
/// valeur n'est pas une chaîne ou si elle est vide. Une propriété manquante n'est
/// pas une panne : l'appelant a toujours un repli.
fn text_property(store: &IPropertyStore, key: &PROPERTYKEY, what: &str) -> Option<String> {
    property(store, key, what)
        .ok()?
        .as_wide_string()
        .filter(|s| !s.trim().is_empty())
}

/// Traduit un endpoint actif en [`DeviceInfo`].
pub(crate) fn describe(
    device: &IMMDevice,
    defaults: &Defaults,
) -> Result<EndpointInfo, BackendError> {
    let id = endpoint_id(device)?;
    let endpoint: IMMEndpoint = device
        .cast()
        .map_err(|e| platform_error("IMMDevice::QueryInterface(IMMEndpoint)", &e))?;
    // SAFETY: interface valide obtenue par `QueryInterface`.
    let flow = unsafe { endpoint.GetDataFlow() }
        .map_err(|e| platform_error("IMMEndpoint::GetDataFlow", &e))?;
    let direction = direction_from_flow(flow).ok_or_else(|| {
        BackendError::Platform(format!("IMMEndpoint::GetDataFlow a rendu eAll pour {id}"))
    })?;

    // SAFETY: interface valide ; lecture seule du magasin.
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }
        .map_err(|e| platform_error("IMMDevice::OpenPropertyStore", &e))?;
    // Trois valeurs, trois rôles. Le nom convivial est la composition que Windows
    // affiche (« Conduit 1 (Conduit — câbles audio virtuels) ») : c'est celui qu'on
    // publie, celui que l'utilisateur retrouve dans les réglages Son. La
    // description est le nom d'endpoint seul (« Conduit 1 », « Musique » après un
    // renommage) : c'est le nom que le câble remonte. La marque est notre valeur,
    // celle qui **survit** au renommage et qui seule identifie le câble. Voir
    // `cable_id_from_endpoint`.
    let name = text_property(
        &store,
        &PKEY_Device_FriendlyName,
        "PKEY_Device_FriendlyName",
    )
    .unwrap_or_else(|| id.clone());
    let description = text_property(&store, &PKEY_Device_DeviceDesc, "PKEY_Device_DeviceDesc");
    let marque = text_property(&store, &MARQUE_KEY, "marque de câble Conduit");

    // `IAudioClient` donne le format de mixage et la période. Un échec d'activation
    // n'empêche pas de décrire le périphérique : on retombe sur le format du
    // périphérique et la période par défaut du moteur.
    let client: Option<IAudioClient> = activate_client(device).ok();

    // Format de mixage d'abord (ce qu'un flux partagé délivre sans conversion),
    // format du périphérique en repli.
    let format = client
        .as_ref()
        .and_then(|client| mix_format(client).ok())
        .map(|mix| mix.parsed)
        .or_else(|| {
            property(
                &store,
                &PKEY_AudioEngine_DeviceFormat,
                "PKEY_AudioEngine_DeviceFormat",
            )
            .ok()
            .and_then(|value| value.as_blob().and_then(wave_format_from_bytes))
        })
        .ok_or_else(|| {
            BackendError::Platform(format!(
                "format de {id} illisible : IAudioClient::GetMixFormat et \
                 PKEY_AudioEngine_DeviceFormat ont tous deux échoué"
            ))
        })?;
    let sample_rate = SampleRate::new(format.sample_rate).ok_or_else(|| {
        BackendError::Platform(format!(
            "fréquence de mixage de {id} hors plage : {} Hz",
            format.sample_rate
        ))
    })?;
    let channels = usize::from(format.channels);

    // Sans `IAudioClient`, aucun flux ne s'ouvrira : on n'annonce que le natif.
    let sample_rates = match &client {
        Some(_) => PROBED_RATES.to_vec(),
        None => vec![sample_rate],
    };
    let period_hns = client
        .as_ref()
        .and_then(|client| device_period(client).ok().map(|p| p.default))
        .unwrap_or(DEFAULT_PERIOD_HNS);

    Ok(EndpointInfo {
        info: DeviceInfo {
            is_default: defaults.id_for(direction) == Some(id.as_str()),
            cable: cable_id_from_endpoint(marque.as_deref(), description.as_deref(), &name),
            id: DeviceId::new(id),
            name,
            direction,
            channels,
            sample_rate,
            sample_rates,
            default_block: frames_from_period(period_hns, sample_rate),
        },
        description,
    })
}

/// Format de mixage du moteur (`IAudioClient::GetMixFormat`) : le bloc tel que COM
/// l'a rendu (à passer tel quel à `InitializeSharedAudioStream`) et sa lecture.
pub(crate) struct MixFormat {
    /// Le `WAVEFORMATEX` (ou `WAVEFORMATEXTENSIBLE`) possédé.
    pub(crate) raw: CoTaskMem<WAVEFORMATEX>,
    /// Ce qu'on en retient.
    pub(crate) parsed: WaveFormat,
}

impl MixFormat {
    /// Pointeur à passer aux appels WASAPI qui prennent un `*const WAVEFORMATEX`.
    pub(crate) fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.raw.as_ptr()
    }
}

/// Format de mixage du moteur (`IAudioClient::GetMixFormat`).
pub(crate) fn mix_format(client: &IAudioClient) -> Result<MixFormat, BackendError> {
    // SAFETY: client valide ; le bloc rendu est alloué par COM et nous revient.
    let raw = unsafe { client.GetMixFormat() }
        .map_err(|e| platform_error("IAudioClient::GetMixFormat", &e))?;
    // SAFETY: `GetMixFormat` a réussi : `raw` est un bloc `CoTaskMemAlloc`.
    let raw = unsafe { CoTaskMem::from_raw(raw) };
    let ptr = raw.as_ptr();
    if ptr.is_null() {
        return Err(BackendError::Platform(
            "IAudioClient::GetMixFormat a rendu un pointeur nul".into(),
        ));
    }
    // SAFETY: `ptr` pointe un `WAVEFORMATEX` complet ; la structure est `packed(1)`,
    // on lit `cbSize` par `read_unaligned` puis les `18 + cbSize` octets qui suivent.
    let bytes = unsafe {
        let cb_size = usize::from(core::ptr::addr_of!((*ptr).cbSize).read_unaligned());
        core::slice::from_raw_parts(ptr.cast::<u8>(), WAVEFORMATEX_SIZE + cb_size)
    };
    let parsed = wave_format_from_bytes(bytes).ok_or_else(|| {
        BackendError::Platform("IAudioClient::GetMixFormat a rendu un format vide".into())
    })?;
    Ok(MixFormat { raw, parsed })
}

/// Périodes du moteur pour ce périphérique (`IAudioClient::GetDevicePeriod`), en
/// unités de 100 ns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DevicePeriod {
    /// Période par défaut, celle du mode partagé classique (10 ms en général).
    pub(crate) default: i64,
    /// Période minimale, celle du mode exclusif (M1b-32).
    pub(crate) minimum: i64,
}

/// Périodes du moteur pour ce périphérique.
pub(crate) fn device_period(client: &IAudioClient) -> Result<DevicePeriod, BackendError> {
    let mut default = 0i64;
    let mut minimum = 0i64;
    // SAFETY: client valide ; les deux sorties sont des locales vivantes.
    unsafe {
        client.GetDevicePeriod(
            Some(core::ptr::addr_of_mut!(default)),
            Some(core::ptr::addr_of_mut!(minimum)),
        )
    }
    .map_err(|e| platform_error("IAudioClient::GetDevicePeriod", &e))?;
    Ok(DevicePeriod { default, minimum })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Image mémoire d'un `WAVEFORMATEXTENSIBLE`, telle que la publient
    /// `GetMixFormat`, `PKEY_AudioEngine_DeviceFormat` et `IsFormatSupported`.
    fn sample_bytes(sample: SampleType, channels: u16, rate: u32, mask: u32) -> Vec<u8> {
        let format = hardware_format(
            sample,
            channels,
            SampleRate::new(rate).unwrap_or(SampleRate::HZ_48000),
            mask,
        );
        let block_align = channels * sample.bytes() as u16;
        let mut bytes = Vec::with_capacity(WAVEFORMATEXTENSIBLE_SIZE);
        bytes.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&sample.bits().to_le_bytes());
        bytes.extend_from_slice(&22u16.to_le_bytes());
        bytes.extend_from_slice(&sample.valid_bits().to_le_bytes());
        bytes.extend_from_slice(&mask.to_le_bytes());
        let sub = format.SubFormat;
        bytes.extend_from_slice(&sub.data1.to_le_bytes());
        bytes.extend_from_slice(&sub.data2.to_le_bytes());
        bytes.extend_from_slice(&sub.data3.to_le_bytes());
        bytes.extend_from_slice(&sub.data4);
        assert_eq!(bytes.len(), WAVEFORMATEXTENSIBLE_SIZE);
        bytes
    }

    /// Image mémoire d'un `WAVEFORMATEXTENSIBLE` float32 (le cas courant).
    fn extensible_bytes(channels: u16, rate: u32, mask: u32) -> Vec<u8> {
        sample_bytes(SampleType::F32, channels, rate, mask)
    }

    #[test]
    fn float32_format_is_consistent() {
        let format = hardware_format(SampleType::F32, 2, SampleRate::HZ_48000, MASK_STEREO);
        let f = format.Format;
        assert_eq!({ f.nChannels }, 2);
        assert_eq!({ f.nSamplesPerSec }, 48_000);
        assert_eq!({ f.nBlockAlign }, 8);
        assert_eq!({ f.nAvgBytesPerSec }, 384_000);
        assert_eq!({ f.wBitsPerSample }, 32);
        assert_eq!({ f.cbSize }, 22);
        assert_eq!({ format.dwChannelMask }, MASK_STEREO);
        assert_eq!({ format.SubFormat }, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        // SAFETY: `Samples` est une union de `u16` : toutes les variantes sont
        // valides pour n'importe quelle valeur.
        assert_eq!(unsafe { format.Samples.wValidBitsPerSample }, 32);
        // Le même format, sérialisé octet par octet, se relit comme float32.
        let bytes = extensible_bytes(2, 48_000, MASK_STEREO);
        assert!(wave_format_from_bytes(&bytes).unwrap().float32);
        // Sous-format PCM : pas float32.
        let mut pcm = bytes.clone();
        pcm[24] = 1;
        assert!(!wave_format_from_bytes(&pcm).unwrap().float32);
        // Extensible mais 24 bits : pas float32.
        let mut bits24 = bytes;
        bits24[14] = 24;
        assert!(!wave_format_from_bytes(&bits24).unwrap().float32);
    }

    /// M1b-32 : les trois formats entiers proposés au matériel en mode exclusif.
    #[test]
    fn pcm_formats_carry_their_container_and_valid_bits() {
        for (sample, bits, valid, block_align, avg) in [
            (SampleType::Pcm24In32, 32u16, 24u16, 8u16, 384_000u32),
            (SampleType::Pcm24, 24, 24, 6, 288_000),
            (SampleType::Pcm16, 16, 16, 4, 192_000),
        ] {
            let format = hardware_format(sample, 2, SampleRate::HZ_48000, MASK_STEREO);
            let f = format.Format;
            assert_eq!({ f.wFormatTag }, WAVE_FORMAT_EXTENSIBLE, "{sample}");
            assert_eq!({ f.nChannels }, 2, "{sample}");
            assert_eq!({ f.nSamplesPerSec }, 48_000, "{sample}");
            assert_eq!({ f.wBitsPerSample }, bits, "{sample}");
            assert_eq!({ f.nBlockAlign }, block_align, "{sample}");
            assert_eq!({ f.nAvgBytesPerSec }, avg, "{sample}");
            assert_eq!({ f.cbSize }, 22, "{sample}");
            assert_eq!({ format.dwChannelMask }, MASK_STEREO, "{sample}");
            assert_eq!({ format.SubFormat }, KSDATAFORMAT_SUBTYPE_PCM, "{sample}");
            // SAFETY: `Samples` est une union de `u16` : toutes les variantes sont
            // valides pour n'importe quelle valeur.
            let got = unsafe { format.Samples.wValidBitsPerSample };
            assert_eq!(got, valid, "{sample}");
            // Sérialisé puis relu, le format se reconnaît lui-même : c'est ainsi
            // qu'une proposition `S_FALSE` du pilote est traduite.
            let parsed = wave_format_from_bytes(&sample_bytes(sample, 2, 48_000, MASK_STEREO))
                .expect("format lisible");
            assert!(parsed.pcm && !parsed.float32, "{sample}");
            assert_eq!(parsed.bits, bits, "{sample}");
            assert_eq!(parsed.valid_bits, valid, "{sample}");
            assert_eq!(parsed.block_align, block_align, "{sample}");
            assert_eq!(parsed.sample_type(), Some(sample), "{sample}");
        }
        // Un mono 6 canaux : `nBlockAlign` suit le nombre de canaux.
        let f = hardware_format(SampleType::Pcm24, 6, SampleRate::HZ_96000, 0x3F).Format;
        assert_eq!({ f.nBlockAlign }, 18);
        assert_eq!({ f.nAvgBytesPerSec }, 96_000 * 18);
    }

    /// Les formats que Conduit ne sait pas convertir n'ont pas de type.
    #[test]
    fn unknown_sample_layouts_have_no_type() {
        let float = wave_format_from_bytes(&extensible_bytes(2, 48_000, MASK_STEREO)).unwrap();
        assert_eq!(float.sample_type(), Some(SampleType::F32));
        // PCM 20 bits dans un conteneur 24 : reconnu comme PCM, mais pas converti.
        let mut bytes = sample_bytes(SampleType::Pcm24, 2, 48_000, MASK_STEREO);
        bytes[18] = 20;
        let odd = wave_format_from_bytes(&bytes).unwrap();
        assert!(odd.pcm);
        assert_eq!(odd.valid_bits, 20);
        assert_eq!(odd.sample_type(), None);
        // Ni float ni PCM (sous-format inconnu) : pas de type.
        let mut bytes = extensible_bytes(2, 48_000, MASK_STEREO);
        bytes[24] = 9;
        let alien = wave_format_from_bytes(&bytes).unwrap();
        assert!(!alien.pcm && !alien.float32);
        assert_eq!(alien.sample_type(), None);
    }

    #[test]
    fn masks_follow_the_channel_count() {
        assert_eq!(default_mask(1), MASK_MONO);
        assert_eq!(default_mask(2), MASK_STEREO);
        assert_eq!(default_mask(6), 0);
        let stereo = wave_format_from_bytes(&extensible_bytes(2, 48_000, MASK_STEREO)).unwrap();
        assert_eq!(stereo.mask_for(2), MASK_STEREO);
        assert_eq!(stereo.mask_for(1), MASK_MONO);
        assert_eq!(stereo.mask_for(8), 0);
        let surround = wave_format_from_bytes(&extensible_bytes(6, 48_000, 0x3F)).unwrap();
        assert_eq!(surround.mask_for(6), 0x3F);
        assert_eq!(surround.mask_for(2), MASK_STEREO);
    }

    #[test]
    fn cable_names_are_matched_exactly() {
        assert_eq!(cable_id_from_name("Conduit 1"), Some(CableId(1)));
        assert_eq!(cable_id_from_name("Conduit 16"), Some(CableId(16)));
        assert_eq!(cable_id_from_name("Conduit 01"), None);
        assert_eq!(cable_id_from_name("conduit 1"), None);
        assert_eq!(cable_id_from_name("Conduit"), None);
        assert_eq!(cable_id_from_name("Conduit "), None);
        assert_eq!(cable_id_from_name("Conduit 1 (2)"), None);
        // Les câbles sont numérotés à partir de 1.
        assert_eq!(cable_id_from_name("Conduit 0"), None);
        assert_eq!(cable_id_from_name("Conduit 99999999999"), None);
        assert_eq!(cable_id_from_name("Casque USB"), None);
    }

    /// Le nom composé que Windows affiche n'est **pas** un identifiant de câble.
    ///
    /// C'est la mesure faite dans la machine virtuelle : les deux côtés du câble 1
    /// portent « Conduit 1 (Conduit — câbles audio virtuels) » en
    /// `PKEY_Device_FriendlyName` et « Conduit 1 » en `PKEY_Device_DeviceDesc`.
    /// Relâcher `cable_id_from_name` pour accepter la composition reviendrait à
    /// deviner : « Conduit 1 (2) », que Windows fabrique quand deux périphériques
    /// portent le même nom, désigne un **autre** endpoint. On corrige la source, pas
    /// le parseur.
    #[test]
    fn le_nom_compose_n_identifie_pas_le_cable() {
        const COMPOSE: &str = "Conduit 1 (Conduit — câbles audio virtuels)";
        assert_eq!(cable_id_from_name(COMPOSE), None);
        // Ni marque ni description : le repli sur le nom composé ne devine rien.
        assert_eq!(cable_id_from_endpoint(None, None, COMPOSE), None);
    }

    /// Description et nom composé jouent des rôles différents : l'une identifie le
    /// câble, l'autre s'affiche.
    #[test]
    fn la_description_identifie_le_cable_pas_le_nom_affiche() {
        const COMPOSE: &str = "Conduit 3 (Conduit — câbles audio virtuels)";
        assert_eq!(
            cable_id_from_endpoint(None, Some("Conduit 3"), COMPOSE),
            Some(CableId(3))
        );
        // Sans description, on retombe sur le nom : suffisant s'il n'est pas composé.
        assert_eq!(
            cable_id_from_endpoint(None, None, "Conduit 3"),
            Some(CableId(3))
        );
        // Une description vide n'arrive pas jusqu'ici (`text_property` la filtre),
        // mais une description qui n'est pas un câble n'en invente pas un, même si
        // le nom affiché commence par « Conduit ».
        assert_eq!(
            cable_id_from_endpoint(None, Some("Casque USB"), COMPOSE),
            None
        );
        // Un endpoint qui n'est pas un câble Conduit n'a pas d'identité de câble.
        assert_eq!(
            cable_id_from_endpoint(None, Some("Haut-parleurs"), "Haut-parleurs (Realtek Audio)"),
            None
        );
    }

    /// **La marque prime sur la description** : c'est elle qui fait survivre le lien
    /// entre un câble et ses endpoints au renommage.
    ///
    /// La table est celle du défaut mesuré dans la machine virtuelle : après
    /// `conduit-helper renommer 1 Musique`, les deux endpoints du câble 1 portent
    /// `description = « Musique »` et `marque = « Conduit 1 »`, et `cable list` rendait
    /// des jetons de repli parce que le rattachement ne lisait que la description.
    #[test]
    fn la_marque_prime_sur_la_description() {
        /// Le nom composé d'un endpoint renommé, tel que Windows le fabrique.
        fn compose(affiche: &str) -> String {
            format!("{affiche} (Conduit — câbles audio virtuels)")
        }
        /// Un cas de la table : marque, description, nom affiché, câble attendu.
        type Cas<'a> = (Option<&'a str>, Option<&'a str>, &'a str, Option<CableId>);
        let cas: [Cas<'_>; 10] = [
            // Le cas mesuré : renommé « Musique », marqué « Conduit 1 ».
            (
                Some("Conduit 1"),
                Some("Musique"),
                "Musique",
                Some(CableId(1)),
            ),
            // **Le cas piège** : un endpoint renommé du nom d'un *autre* câble. C'est sa
            // marque qui compte — il est le 3, pas le 5.
            (
                Some("Conduit 3"),
                Some("Conduit 5"),
                "Conduit 5",
                Some(CableId(3)),
            ),
            // Jamais renommé : pas de marque, la description suffit.
            (None, Some("Conduit 2"), "Conduit 2", Some(CableId(2))),
            // Renommé puis rendu à son nom d'origine : la marque a été supprimée, la
            // description est redevenue « Conduit 4 ». Les deux disent la même chose.
            (None, Some("Conduit 4"), "Conduit 4", Some(CableId(4))),
            // Marque et description d'accord : le cas d'un renommage vers le même nom.
            (
                Some("Conduit 6"),
                Some("Conduit 6"),
                "Conduit 6",
                Some(CableId(6)),
            ),
            // Une marque illisible ne retombe **pas** sur la description : quelqu'un a
            // écrit dans notre valeur, et deviner à sa place vaudrait moins que rien.
            (Some("n'importe quoi"), Some("Conduit 7"), "Conduit 7", None),
            // Le nom **composé** n'est pas davantage une marque valable.
            (
                Some("Conduit 8 (Conduit — câbles audio virtuels)"),
                Some("Musique"),
                "Musique",
                None,
            ),
            // Un endpoint qui n'est ni marqué ni nommé « Conduit N » n'a pas de câble,
            // même renommé « Conduit 9 » par l'utilisateur dans mmsys.cpl : sans notre
            // marque, rien ne le rattache — et c'est bien un câble qu'il n'est pas.
            (None, Some("Casque USB"), "Casque USB (Realtek Audio)", None),
            // Ni marque ni description : le nom composé ne devine rien.
            (
                None,
                None,
                "Conduit 1 (Conduit — câbles audio virtuels)",
                None,
            ),
            // Ni marque ni description, nom non composé : le dernier recours joue.
            (None, None, "Conduit 16", Some(CableId(16))),
        ];
        for (marque, description, nom, attendu) in cas {
            assert_eq!(
                cable_id_from_endpoint(marque, description, nom),
                attendu,
                "marque {marque:?}, description {description:?}, nom « {nom} »"
            );
        }
        // Sur les seize câbles, un renommage quelconque ne change rien à l'identité.
        for numero in 1..=16u32 {
            let marque = format!("Conduit {numero}");
            let affiche = compose("Musique");
            assert_eq!(
                cable_id_from_endpoint(Some(&marque), Some("Musique"), &affiche),
                Some(CableId(numero)),
                "câble {numero} renommé"
            );
        }
    }

    /// La clé de la marque est **exactement** celle que le service écrit.
    ///
    /// Le service range `{3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b},1` dans la clé
    /// `Properties` de l'endpoint (`conduit_helper::registre::valeur_marque`). Les deux
    /// bouts prennent leur GUID et leur `pid` dans `conduit-kmd-core` ; ce test le
    /// vérifie champ par champ, parce qu'une clé fausse ne se manifesterait par aucun
    /// message — la marque serait simplement introuvable et le défaut reviendrait tel
    /// quel.
    #[test]
    fn la_cle_de_la_marque_est_celle_que_le_service_ecrit() {
        assert_eq!(MARQUE_KEY.pid, PID_MARQUE_CABLE);
        assert_eq!(MARQUE_KEY.fmtid.data1, KSPROPSETID_CONDUIT.data1);
        assert_eq!(MARQUE_KEY.fmtid.data2, KSPROPSETID_CONDUIT.data2);
        assert_eq!(MARQUE_KEY.fmtid.data3, KSPROPSETID_CONDUIT.data3);
        assert_eq!(MARQUE_KEY.fmtid.data4, KSPROPSETID_CONDUIT.data4);
        // La valeur littérale, celle que `regedit` montre : elle attrape un GUID modifié
        // par accident, que la comparaison ci-dessus ne verrait pas.
        assert_eq!(
            format!("{:?}", MARQUE_KEY.fmtid).to_ascii_lowercase(),
            "3f1b27a4-8c6e-4d02-9b75-e4a0d61c8f3b"
        );
        // Ce n'est aucune des clés du système que ce module lit par ailleurs.
        assert_ne!(MARQUE_KEY.fmtid, PKEY_Device_DeviceDesc.fmtid);
        assert_ne!(MARQUE_KEY.fmtid, PKEY_Device_FriendlyName.fmtid);
    }

    /// Le nom d'un câble se lit sur ses deux endpoints, et une divergence se voit.
    #[test]
    fn le_nom_du_cable_vient_des_deux_sens() {
        // Les deux sens d'accord — le cas normal, renommé ou non.
        assert_eq!(
            cable_name(Some("Musique"), Some("Musique")),
            CableName::Accord("Musique")
        );
        assert_eq!(
            cable_name(Some("Conduit 1"), Some("Conduit 1")),
            CableName::Accord("Conduit 1")
        );
        // Un seul côté publié : le câble est peut-être à moitié apparu (77 ms mesurées
        // entre l'écriture et la publication des endpoints). Ce n'est pas une divergence.
        assert_eq!(
            cable_name(Some("Musique"), None),
            CableName::Accord("Musique")
        );
        assert_eq!(
            cable_name(None, Some("Musique")),
            CableName::Accord("Musique")
        );
        // Aucun côté : on garde le nom du service.
        assert_eq!(cable_name(None, None), CableName::Inconnu);
        assert_eq!(cable_name(None, None).retenu(), None);
        // Divergence : le rendu l'emporte, et l'écarté part au journal.
        assert_eq!(
            cable_name(Some("Musique"), Some("Conduit 1")),
            CableName::Divergent {
                retenu: "Musique",
                ecarte: "Conduit 1",
            }
        );
        assert_eq!(
            cable_name(Some("Musique"), Some("Conduit 1")).retenu(),
            Some("Musique")
        );
        // Le choix ne dépend pas de l'ordre des arguments : inverser les deux côtés
        // change le retenu, ce qui est bien la preuve que c'est le **rendu** qui décide.
        assert_eq!(
            cable_name(Some("Conduit 1"), Some("Musique")).retenu(),
            Some("Conduit 1")
        );
    }

    #[test]
    fn extensible_format_yields_channels_rate_and_mask() {
        let bytes = extensible_bytes(2, 48_000, MASK_STEREO);
        assert_eq!(
            wave_format_from_bytes(&bytes),
            Some(WaveFormat {
                channels: 2,
                sample_rate: 48_000,
                block_align: 8,
                channel_mask: Some(MASK_STEREO),
                bits: 32,
                valid_bits: 32,
                float32: true,
                pcm: false,
            })
        );
        let bytes = extensible_bytes(6, 96_000, 0x3F);
        assert_eq!(
            wave_format_from_bytes(&bytes),
            Some(WaveFormat {
                channels: 6,
                sample_rate: 96_000,
                block_align: 24,
                channel_mask: Some(0x3F),
                bits: 32,
                valid_bits: 32,
                float32: true,
                pcm: false,
            })
        );
    }

    #[test]
    fn plain_waveformatex_has_no_mask() {
        // WAVE_FORMAT_PCM, 1 canal, 44,1 kHz, 16 bits.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&88_200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            wave_format_from_bytes(&bytes),
            Some(WaveFormat {
                channels: 1,
                sample_rate: 44_100,
                block_align: 2,
                channel_mask: None,
                bits: 16,
                valid_bits: 16,
                float32: false,
                pcm: true,
            })
        );
        // Un `WAVE_FORMAT_PCM` simple se convertit comme du PCM 16 bits.
        assert_eq!(
            wave_format_from_bytes(&bytes).and_then(|f| f.sample_type()),
            Some(SampleType::Pcm16)
        );
        // `WAVE_FORMAT_IEEE_FLOAT` 32 bits sans `SubFormat` est bien du float32.
        let mut float = bytes.clone();
        float[0] = WAVE_FORMAT_IEEE_FLOAT as u8;
        float[14] = 32;
        assert_eq!(
            wave_format_from_bytes(&float).map(|f| f.float32),
            Some(true)
        );
        // Un `WAVEFORMATEX` tronqué à 16 octets (sans `cbSize`) est refusé.
        assert_eq!(wave_format_from_bytes(&bytes[..16]), None);
        // Un format extensible tronqué garde canaux et fréquence, sans masque.
        let truncated = &extensible_bytes(2, 48_000, MASK_STEREO)[..24];
        assert_eq!(
            wave_format_from_bytes(truncated).map(|f| f.channel_mask),
            Some(None)
        );
    }

    #[test]
    fn degenerate_formats_are_rejected() {
        assert_eq!(wave_format_from_bytes(&[]), None);
        assert_eq!(
            wave_format_from_bytes(&extensible_bytes(0, 48_000, 0)),
            None
        );
        assert_eq!(wave_format_from_bytes(&extensible_bytes(2, 0, 0)), None);
    }

    #[test]
    fn period_rounds_to_nearest_frame() {
        // 10 ms.
        assert_eq!(frames_from_period(100_000, SampleRate::HZ_48000), 480);
        assert_eq!(frames_from_period(100_000, SampleRate::HZ_44100), 441);
        assert_eq!(frames_from_period(100_000, SampleRate::HZ_96000), 960);
        // 3 ms (période minimale courante).
        assert_eq!(frames_from_period(30_000, SampleRate::HZ_48000), 144);
        // 2,9 ms à 44,1 kHz = 127,89 trames → 128.
        assert_eq!(frames_from_period(29_000, SampleRate::HZ_44100), 128);
        // Jamais zéro, jamais négatif.
        assert_eq!(frames_from_period(0, SampleRate::HZ_48000), 1);
        assert_eq!(frames_from_period(-5, SampleRate::HZ_48000), 1);
        assert_eq!(
            frames_from_period(DEFAULT_PERIOD_HNS, SampleRate::HZ_48000),
            480
        );
    }

    #[test]
    fn flow_to_direction() {
        assert_eq!(direction_from_flow(eRender), Some(DeviceDirection::Render));
        assert_eq!(
            direction_from_flow(eCapture),
            Some(DeviceDirection::Capture)
        );
        assert_eq!(
            direction_from_flow(windows::Win32::Media::Audio::eAll),
            None
        );
    }
}
