//! Traduction d'un endpoint MMDevice en [`DeviceInfo`].
//!
//! Tout ici s'exécute sur le fil MMDevice (voir `mmdevice_thread`). Les fonctions
//! pures — analyse d'un `WAVEFORMATEX`, arrondi de la période, reconnaissance des
//! noms de câbles — sont séparées des appels COM pour être testables sans matériel.

use conduit_backend::{BackendError, CableId, DeviceDirection, DeviceId, DeviceInfo};
use conduit_core::types::SampleRate;
use windows::core::HRESULT;
use windows::core::{Interface, GUID};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, PROPERTYKEY, S_OK};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    IMMEndpoint, PKEY_AudioEngine_DeviceFormat, AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE_ACTIVE,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::com::{platform_error, CoTaskMem, CoTaskString, PropVariant};

/// Fréquences sondées auprès de chaque périphérique pour remplir
/// [`DeviceInfo::sample_rates`].
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

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` (ksmedia.h) : échantillons flottants.
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// `SPEAKER_FRONT_CENTER` / `SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT` (ksmedia.h).
const MASK_MONO: u32 = 0x4;
const MASK_STEREO: u32 = 0x3;

/// Taille de `WAVEFORMATEX` (champs `cbSize` compris).
const WAVEFORMATEX_SIZE: usize = 18;
/// Taille de `WAVEFORMATEXTENSIBLE`.
const WAVEFORMATEXTENSIBLE_SIZE: usize = 40;

/// Ce que l'on retient d'un `WAVEFORMATEX` : canaux, fréquence, disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaveFormat {
    pub(crate) channels: u16,
    pub(crate) sample_rate: u32,
    /// `dwChannelMask` si le format est extensible.
    pub(crate) channel_mask: Option<u32>,
}

/// Lit canaux, fréquence et masque dans l'image mémoire d'un `WAVEFORMATEX`
/// (éventuellement `WAVEFORMATEXTENSIBLE`), telle que la publie
/// `PKEY_AudioEngine_DeviceFormat`.
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
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let channel_mask = (tag == WAVE_FORMAT_EXTENSIBLE && bytes.len() >= WAVEFORMATEXTENSIBLE_SIZE)
        .then(|| u32_at(20));
    Some(WaveFormat {
        channels,
        sample_rate,
        channel_mask,
    })
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

/// Décrit un seul endpoint par son identifiant ; `None` s'il n'existe pas ou n'est
/// pas actif.
pub(crate) fn describe_id(
    enumerator: &IMMDeviceEnumerator,
    id: &str,
) -> Result<Option<DeviceInfo>, BackendError> {
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
    if state != DEVICE_STATE_ACTIVE {
        return Ok(None);
    }
    let defaults = Defaults::query(enumerator)?;
    describe(&device, &defaults).map(Some)
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

    // `IAudioClient` sert au repli du format, aux fréquences acceptées et à la
    // période. Un échec d'activation n'empêche pas de décrire le périphérique.
    // SAFETY: interface valide ; aucun paramètre d'activation.
    let client: Option<IAudioClient> =
        unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }.ok();

    let format = property(
        &store,
        &PKEY_AudioEngine_DeviceFormat,
        "PKEY_AudioEngine_DeviceFormat",
    )
    .ok()
    .and_then(|value| value.as_blob().and_then(wave_format_from_bytes));
    let format = match (format, &client) {
        (Some(format), _) => format,
        (None, Some(client)) => mix_format(client)?,
        (None, None) => {
            return Err(BackendError::Platform(format!(
                "format de {id} illisible : PKEY_AudioEngine_DeviceFormat absent et \
                 IMMDevice::Activate(IAudioClient) a échoué"
            )))
        }
    };
    let sample_rate = SampleRate::new(format.sample_rate).ok_or_else(|| {
        BackendError::Platform(format!(
            "fréquence native de {id} hors plage : {} Hz",
            format.sample_rate
        ))
    })?;
    let channels = usize::from(format.channels);

    let sample_rates = match &client {
        Some(client) => supported_rates(client, &format),
        None => vec![sample_rate],
    };
    let period_hns = client
        .as_ref()
        .and_then(|client| device_period(client).ok())
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

/// Format de mixage du moteur (`IAudioClient::GetMixFormat`).
fn mix_format(client: &IAudioClient) -> Result<WaveFormat, BackendError> {
    // SAFETY: client valide ; le bloc rendu est alloué par COM et nous revient.
    let raw = unsafe { client.GetMixFormat() }
        .map_err(|e| platform_error("IAudioClient::GetMixFormat", &e))?;
    // SAFETY: `GetMixFormat` a réussi : `raw` est un bloc `CoTaskMemAlloc`.
    let owned = unsafe { CoTaskMem::from_raw(raw) };
    let ptr = owned.as_ptr();
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
    wave_format_from_bytes(bytes).ok_or_else(|| {
        BackendError::Platform("IAudioClient::GetMixFormat a rendu un format vide".into())
    })
}

/// Fréquences de [`PROBED_RATES`] acceptées en mode partagé, en float32 avec le
/// nombre de canaux natif.
fn supported_rates(client: &IAudioClient, native: &WaveFormat) -> Vec<SampleRate> {
    PROBED_RATES
        .iter()
        .copied()
        .filter(|rate| is_rate_supported(client, native, *rate))
        .collect()
}

fn is_rate_supported(client: &IAudioClient, native: &WaveFormat, rate: SampleRate) -> bool {
    let channels = native.channels;
    let block_align = channels * 4;
    let mask = native.channel_mask.unwrap_or(match channels {
        1 => MASK_MONO,
        2 => MASK_STEREO,
        _ => 0,
    });
    let format = WAVEFORMATEXTENSIBLE {
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
    };
    let mut closest: CoTaskMem<WAVEFORMATEX> = CoTaskMem::null();
    // SAFETY: `format` vit pendant l'appel et commence par un `WAVEFORMATEX` ;
    // `closest` reçoit un bloc `CoTaskMemAlloc` (ou rien) que la garde libère.
    let hr = unsafe {
        client.IsFormatSupported(
            AUDCLNT_SHAREMODE_SHARED,
            core::ptr::addr_of!(format).cast::<WAVEFORMATEX>(),
            Some(closest.slot()),
        )
    };
    // `S_FALSE` (avec un format « le plus proche ») signifie « pas tel quel ».
    hr == S_OK
}

/// Période par défaut du moteur pour ce périphérique, en unités de 100 ns.
fn device_period(client: &IAudioClient) -> Result<i64, BackendError> {
    let mut default_period = 0i64;
    let mut minimum_period = 0i64;
    // SAFETY: client valide ; les deux sorties sont des locales vivantes.
    unsafe {
        client.GetDevicePeriod(
            Some(core::ptr::addr_of_mut!(default_period)),
            Some(core::ptr::addr_of_mut!(minimum_period)),
        )
    }
    .map_err(|e| platform_error("IAudioClient::GetDevicePeriod", &e))?;
    Ok(default_period)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Image mémoire d'un `WAVEFORMATEXTENSIBLE` float32.
    fn extensible_bytes(channels: u16, rate: u32, mask: u32) -> Vec<u8> {
        let block_align = channels * 4;
        let mut bytes = Vec::with_capacity(WAVEFORMATEXTENSIBLE_SIZE);
        bytes.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&22u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&mask.to_le_bytes());
        // Le `SubFormat` n'est pas lu : seize octets quelconques suffisent.
        bytes.extend_from_slice(&[0u8; 16]);
        assert_eq!(bytes.len(), WAVEFORMATEXTENSIBLE_SIZE);
        bytes
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
                channel_mask: Some(MASK_STEREO),
            })
        );
        let bytes = extensible_bytes(6, 96_000, 0x3F);
        assert_eq!(
            wave_format_from_bytes(&bytes),
            Some(WaveFormat {
                channels: 6,
                sample_rate: 96_000,
                channel_mask: Some(0x3F),
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
                channel_mask: None,
            })
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
