//! Ouverture d'un flux WASAPI en mode partagé : négociation du format et
//! initialisation de l'`IAudioClient`, sur le fil MMDevice.
//!
//! Le format demandé est **honoré** (comme avec PipeWire) : le rappel reçoit
//! exactement `format.channels` canaux entrelacés `f32` à `format.sample_rate`. Deux
//! chemins y mènent :
//!
//! 1. **Basse latence** — le format demandé est le format de mixage du moteur (et
//!    celui-ci est float32) : `IAudioClient3::InitializeSharedAudioStream` avec la
//!    plus petite période prise en charge ≥ `block_frames`
//!    ([`choose_period`]). Aucune conversion, période au choix entre le minimum du
//!    pilote et le maximum du moteur.
//! 2. **Conversion** — sinon : `IAudioClient::Initialize` en mode partagé avec
//!    `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | SRC_DEFAULT_QUALITY` et un
//!    `WAVEFORMATEXTENSIBLE` float32 aux valeurs demandées ; Windows convertit
//!    (fréquence, canaux, type d'échantillon). La période est celle par défaut du
//!    moteur, `block_frames` effectif = cette période en trames à la fréquence
//!    demandée. On y retombe aussi si le chemin 1 échoue (période verrouillée par
//!    un autre client, pilote sans `IAudioClient3`).
//!
//! Les objets COM créés ici (`IAudioClient`, `IAudioRenderClient` ou
//! `IAudioCaptureClient`) partent ensuite vers le fil du flux (`stream`) sous forme
//! d'[`AgileReference`] : les interfaces du crate `windows` ne sont pas `Send`, la
//! référence agile l'est, et sa résolution depuis un autre fil de l'appartement
//! multi-fil rend le pointeur direct (les objets WASAPI sont libres de fil).

use std::time::Duration;

use conduit_backend::{BackendError, DeviceDirection, DeviceId, DeviceInfo, StreamFormat};
use windows::core::{AgileReference, Interface};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IAudioClient3, IAudioRenderClient, IMMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};

use crate::com::{platform_error, Event};
use crate::devices::{
    activate_client, describe, device_period, find_active, float32_format, frames_from_period,
    mix_format, Defaults, DEFAULT_PERIOD_HNS,
};

/// Périodes du moteur audio pour un format donné, en trames
/// (`IAudioClient3::GetSharedModeEnginePeriod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnginePeriods {
    /// Période par défaut (celle du chemin par conversion).
    pub default: u32,
    /// Période fondamentale : toute période acceptée en est un multiple.
    pub fundamental: u32,
    /// Plus petite période acceptée (pilote).
    pub min: u32,
    /// Plus grande période acceptée (moteur).
    pub max: u32,
}

/// Plus petite période acceptée ≥ `requested` : le multiple de la fondamentale
/// immédiatement supérieur ou égal, borné à `[min, max]`.
pub fn choose_period(requested: usize, periods: EnginePeriods) -> u32 {
    let fundamental = u64::from(periods.fundamental.max(1));
    let requested = u64::try_from(requested).unwrap_or(u64::MAX).max(1);
    let multiple = requested.div_ceil(fundamental).saturating_mul(fundamental);
    let clamped = multiple.clamp(
        u64::from(periods.min),
        u64::from(periods.max.max(periods.min)),
    );
    u32::try_from(clamped).unwrap_or(u32::MAX)
}

/// Comment le flux a été initialisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitPath {
    /// `IAudioClient3::InitializeSharedAudioStream` au format de mixage, avec la
    /// période choisie (en trames).
    LowLatency {
        /// Période obtenue.
        period_frames: u32,
        /// Périodes annoncées par le moteur.
        periods: EnginePeriods,
    },
    /// `IAudioClient::Initialize` avec conversion automatique, période par défaut.
    Converted,
}

/// Latence et tailles d'un flux ouvert (hors trait `DeviceHandle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLatency {
    /// Taille du tampon partagé avec le moteur, en trames (`GetBufferSize`).
    pub buffer_frames: usize,
    /// Trames par réveil attendues (`block_frames` effectif).
    pub period_frames: usize,
    /// Latence du flux rapportée par WASAPI (`GetStreamLatency`), hors tampon.
    /// Certains pilotes rendent 0 en mode partagé (observé sur une sortie HDMI
    /// NVIDIA) : la latence utile se déduit alors de `buffer_frames`.
    pub stream_latency: Duration,
    /// Chemin d'initialisation.
    pub path: InitPath,
}

/// Service WASAPI selon le sens.
pub(crate) enum Service {
    Render(AgileReference<IAudioRenderClient>),
    Capture(AgileReference<IAudioCaptureClient>),
}

/// Ce que le fil MMDevice crée et que le fil du flux consomme.
pub(crate) struct StreamObjects {
    pub(crate) client: AgileReference<IAudioClient>,
    pub(crate) service: Service,
    /// Signalé par le moteur à chaque période (`SetEventHandle`).
    pub(crate) event: Event,
    pub(crate) latency: StreamLatency,
}

/// Résultat d'une ouverture.
pub(crate) struct Opened {
    pub(crate) info: DeviceInfo,
    /// Format effectif : fréquence et canaux demandés, `block_frames` obtenu.
    pub(crate) format: StreamFormat,
    pub(crate) objects: StreamObjects,
}

/// Ouvre un flux partagé sur l'endpoint `id`. À appeler depuis le fil MMDevice.
pub(crate) fn open(
    enumerator: &IMMDeviceEnumerator,
    id: &DeviceId,
    format: StreamFormat,
) -> Result<Opened, BackendError> {
    let device =
        find_active(enumerator, id.as_str())?.ok_or_else(|| BackendError::NotFound(id.clone()))?;
    let channels = match u16::try_from(format.channels) {
        Ok(channels) if channels > 0 => channels,
        _ => {
            return Err(BackendError::UnsupportedFormat {
                device: id.clone(),
                reason: format!(
                    "un flux doit avoir entre 1 et {} canaux, pas {}",
                    u16::MAX,
                    format.channels
                ),
            })
        }
    };
    let info = describe(&device, &Defaults::query(enumerator)?)?;
    let event = Event::new(false)?;

    let client = activate_client(&device)?;
    let mix = mix_format(&client)?;
    let matches_mix = mix.parsed.float32
        && mix.parsed.channels == channels
        && mix.parsed.sample_rate == format.sample_rate.hz();

    let (client, path, block_frames) =
        match matches_mix.then(|| low_latency(&client, mix.as_ptr(), format.block_frames)) {
            Some(Ok((period_frames, periods))) => (
                client,
                InitPath::LowLatency {
                    period_frames,
                    periods,
                },
                period_frames as usize,
            ),
            // Chemin 1 refusé (ou format différent du mixage) : un client neuf, car un
            // `IAudioClient` dont l'initialisation a échoué n'est pas réutilisable.
            _ => {
                let client = activate_client(&device)?;
                let period_hns = device_period(&client)
                    .map(|p| p.default)
                    .unwrap_or(DEFAULT_PERIOD_HNS);
                let mask = mix.parsed.mask_for(channels);
                let wanted = float32_format(channels, format.sample_rate, mask);
                // SAFETY: `wanted` vit pendant l'appel et commence par un `WAVEFORMATEX` ;
                // aucun GUID de session (session par défaut du processus).
                unsafe {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                        0,
                        0,
                        core::ptr::addr_of!(wanted).cast::<WAVEFORMATEX>(),
                        None,
                    )
                }
                .map_err(|e| unsupported_or_platform(id, "IAudioClient::Initialize", &e))?;
                (
                    client,
                    InitPath::Converted,
                    frames_from_period(period_hns, format.sample_rate),
                )
            }
        };

    // SAFETY: client initialisé ; l'événement vit aussi longtemps que le flux
    // (`StreamObjects::event`), plus longtemps que le client.
    unsafe { client.SetEventHandle(event.handle()) }
        .map_err(|e| platform_error("IAudioClient::SetEventHandle", &e))?;
    // SAFETY: client initialisé.
    let buffer_frames = unsafe { client.GetBufferSize() }
        .map_err(|e| platform_error("IAudioClient::GetBufferSize", &e))?;
    // SAFETY: client initialisé.
    let latency_hns = unsafe { client.GetStreamLatency() }
        .map_err(|e| platform_error("IAudioClient::GetStreamLatency", &e))?;

    let service = match info.direction {
        DeviceDirection::Render => {
            // SAFETY: client initialisé en rendu.
            let render: IAudioRenderClient = unsafe { client.GetService() }
                .map_err(|e| platform_error("IAudioClient::GetService(IAudioRenderClient)", &e))?;
            Service::Render(agile(&render, "IAudioRenderClient")?)
        }
        DeviceDirection::Capture => {
            // SAFETY: client initialisé en capture.
            let capture: IAudioCaptureClient = unsafe { client.GetService() }
                .map_err(|e| platform_error("IAudioClient::GetService(IAudioCaptureClient)", &e))?;
            Service::Capture(agile(&capture, "IAudioCaptureClient")?)
        }
    };

    let latency = StreamLatency {
        buffer_frames: buffer_frames as usize,
        period_frames: block_frames.max(1),
        stream_latency: Duration::from_nanos(u64::try_from(latency_hns.max(0) * 100).unwrap_or(0)),
        path,
    };
    Ok(Opened {
        info,
        format: StreamFormat {
            sample_rate: format.sample_rate,
            channels: format.channels,
            block_frames: latency.period_frames,
        },
        objects: StreamObjects {
            client: agile(&client, "IAudioClient")?,
            service,
            event,
            latency,
        },
    })
}

/// Chemin 1 : `IAudioClient3` au format de mixage, période choisie. `Err` signifie
/// « retomber sur le chemin 2 », sans autre conséquence.
fn low_latency(
    client: &IAudioClient,
    mix: *const WAVEFORMATEX,
    block_frames: usize,
) -> Result<(u32, EnginePeriods), BackendError> {
    let client3: IAudioClient3 = client
        .cast()
        .map_err(|e| platform_error("IAudioClient::QueryInterface(IAudioClient3)", &e))?;
    let periods = engine_periods(&client3, mix)?;
    let period = choose_period(block_frames, periods);
    // SAFETY: `mix` pointe le bloc rendu par `GetMixFormat`, vivant pendant l'appel ;
    // aucun GUID de session.
    unsafe {
        client3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, mix, None)
    }
    .map_err(|e| platform_error("IAudioClient3::InitializeSharedAudioStream", &e))?;
    Ok((period, periods))
}

/// Périodes du moteur pour un format (`GetSharedModeEnginePeriod`).
fn engine_periods(
    client3: &IAudioClient3,
    format: *const WAVEFORMATEX,
) -> Result<EnginePeriods, BackendError> {
    let mut periods = EnginePeriods {
        default: 0,
        fundamental: 0,
        min: 0,
        max: 0,
    };
    // SAFETY: `format` est un `WAVEFORMATEX` valide pendant l'appel ; les quatre
    // sorties sont des champs d'une locale vivante.
    unsafe {
        client3.GetSharedModeEnginePeriod(
            format,
            &mut periods.default,
            &mut periods.fundamental,
            &mut periods.min,
            &mut periods.max,
        )
    }
    .map_err(|e| platform_error("IAudioClient3::GetSharedModeEnginePeriod", &e))?;
    if periods.fundamental == 0 || periods.max == 0 {
        return Err(BackendError::Platform(
            "IAudioClient3::GetSharedModeEnginePeriod a rendu des périodes nulles".into(),
        ));
    }
    Ok(periods)
}

/// Référence agile vers une interface, pour la faire voyager vers le fil du flux.
fn agile<T: Interface>(interface: &T, what: &str) -> Result<AgileReference<T>, BackendError> {
    AgileReference::new(interface)
        .map_err(|e| platform_error(&format!("RoGetAgileReference({what})"), &e))
}

/// `AUDCLNT_E_UNSUPPORTED_FORMAT` devient [`BackendError::UnsupportedFormat`], le
/// reste [`BackendError::Platform`].
fn unsupported_or_platform(
    id: &DeviceId,
    call: &str,
    error: &windows::core::Error,
) -> BackendError {
    use windows::Win32::Media::Audio::AUDCLNT_E_UNSUPPORTED_FORMAT;
    if error.code() == AUDCLNT_E_UNSUPPORTED_FORMAT {
        BackendError::UnsupportedFormat {
            device: id.clone(),
            reason: format!("{call} : format refusé par le moteur audio"),
        }
    } else {
        platform_error(call, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAXWELL_LIKE: EnginePeriods = EnginePeriods {
        default: 480,
        fundamental: 48,
        min: 144,
        max: 960,
    };

    #[test]
    fn period_is_the_next_multiple_of_the_fundamental() {
        assert_eq!(choose_period(480, MAXWELL_LIKE), 480);
        assert_eq!(choose_period(481, MAXWELL_LIKE), 528);
        assert_eq!(choose_period(256, MAXWELL_LIKE), 288);
        assert_eq!(choose_period(144, MAXWELL_LIKE), 144);
        assert_eq!(choose_period(145, MAXWELL_LIKE), 192);
    }

    #[test]
    fn period_is_clamped_to_the_engine_range() {
        assert_eq!(choose_period(1, MAXWELL_LIKE), 144);
        assert_eq!(choose_period(0, MAXWELL_LIKE), 144);
        assert_eq!(choose_period(100_000, MAXWELL_LIKE), 960);
        assert_eq!(choose_period(usize::MAX, MAXWELL_LIKE), 960);
    }

    #[test]
    fn degenerate_periods_do_not_panic() {
        let zero = EnginePeriods {
            default: 0,
            fundamental: 0,
            min: 0,
            max: 0,
        };
        assert_eq!(choose_period(480, zero), 0);
        let inverted = EnginePeriods {
            default: 480,
            fundamental: 32,
            min: 512,
            max: 128,
        };
        assert_eq!(choose_period(480, inverted), 512);
    }
}
