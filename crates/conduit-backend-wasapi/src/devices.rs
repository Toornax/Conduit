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
use windows::core::HRESULT;
use windows::core::{Interface, GUID};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    IMMEndpoint, PKEY_AudioEngine_DeviceFormat, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::com::{platform_error, CoTaskMem, CoTaskString, PropVariant};

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

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` (ksmedia.h) : échantillons flottants.
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

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
    /// Échantillons `f32` (tag `WAVE_FORMAT_IEEE_FLOAT` ou sous-format IEEE float,
    /// 32 bits) : le format que Conduit consomme tel quel.
    pub(crate) float32: bool,
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
    let float32 = bits == 32
        && if extensible {
            let mut data4 = [0u8; 8];
            data4.copy_from_slice(&bytes[32..40]);
            GUID {
                data1: u32_at(24),
                data2: u16_at(28),
                data3: u16_at(30),
                data4,
            } == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            tag == WAVE_FORMAT_IEEE_FLOAT
        };
    Some(WaveFormat {
        channels,
        sample_rate,
        block_align,
        channel_mask,
        float32,
    })
}

/// Construit un `WAVEFORMATEXTENSIBLE` float32 entrelacé : le format que Conduit
/// demande à WASAPI (sondage en M1b-30, flux en mode partagé depuis M1b-31).
pub(crate) fn float32_format(channels: u16, rate: SampleRate, mask: u32) -> WAVEFORMATEXTENSIBLE {
    let block_align = channels * 4;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: channels,
            nSamplesPerSec: rate.hz(),
            nAvgBytesPerSec: rate.hz() * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: (WAVEFORMATEXTENSIBLE_SIZE - WAVEFORMATEX_SIZE) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: 32,
        },
        dwChannelMask: mask,
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
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

/// Énumère les endpoints actifs des deux sens.
pub(crate) fn enumerate(enumerator: &IMMDeviceEnumerator) -> Result<Vec<DeviceInfo>, BackendError> {
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
) -> Result<Option<DeviceInfo>, BackendError> {
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

/// Traduit un endpoint actif en [`DeviceInfo`].
pub(crate) fn describe(
    device: &IMMDevice,
    defaults: &Defaults,
) -> Result<DeviceInfo, BackendError> {
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
    let name = property(
        &store,
        &PKEY_Device_FriendlyName,
        "PKEY_Device_FriendlyName",
    )?
    .as_wide_string()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or_else(|| id.clone());

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

    Ok(DeviceInfo {
        is_default: defaults.id_for(direction) == Some(id.as_str()),
        cable: cable_id_from_name(&name),
        id: DeviceId::new(id),
        name,
        direction,
        channels,
        sample_rate,
        sample_rates,
        default_block: frames_from_period(period_hns, sample_rate),
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

    /// Image mémoire d'un `WAVEFORMATEXTENSIBLE` float32.
    fn extensible_bytes(channels: u16, rate: u32, mask: u32) -> Vec<u8> {
        let format = float32_format(
            channels,
            SampleRate::new(rate).unwrap_or(SampleRate::HZ_48000),
            mask,
        );
        let mut bytes = Vec::with_capacity(WAVEFORMATEXTENSIBLE_SIZE);
        bytes.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels * 4)).to_le_bytes());
        bytes.extend_from_slice(&(channels * 4).to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&22u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&mask.to_le_bytes());
        let sub = format.SubFormat;
        bytes.extend_from_slice(&sub.data1.to_le_bytes());
        bytes.extend_from_slice(&sub.data2.to_le_bytes());
        bytes.extend_from_slice(&sub.data3.to_le_bytes());
        bytes.extend_from_slice(&sub.data4);
        assert_eq!(bytes.len(), WAVEFORMATEXTENSIBLE_SIZE);
        bytes
    }

    #[test]
    fn float32_format_is_consistent() {
        let format = float32_format(2, SampleRate::HZ_48000, MASK_STEREO);
        let f = format.Format;
        assert_eq!({ f.nChannels }, 2);
        assert_eq!({ f.nSamplesPerSec }, 48_000);
        assert_eq!({ f.nBlockAlign }, 8);
        assert_eq!({ f.nAvgBytesPerSec }, 384_000);
        assert_eq!({ f.wBitsPerSample }, 32);
        assert_eq!({ f.cbSize }, 22);
        assert_eq!({ format.dwChannelMask }, MASK_STEREO);
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
                float32: true,
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
                float32: true,
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
                float32: false,
            })
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
