//! Ouverture d'un flux WASAPI : négociation du format et initialisation de
//! l'`IAudioClient`, sur le fil MMDevice.
//!
//! Le format demandé est **honoré** (comme avec PipeWire) : le rappel reçoit
//! exactement `format.channels` canaux entrelacés `f32` à `format.sample_rate`.
//! Trois chemins y mènent :
//!
//! 0. **Exclusif** — seulement si la politique du backend le demande
//!    ([`ExclusivePolicy`], module `exclusive`, jamais par défaut) : le flux prend
//!    le périphérique pour lui seul, au format que le matériel accepte (souvent de
//!    l'entier : la conversion vers le `f32` du rappel est alors à notre charge,
//!    module `convert`), avec la période **minimale** du pilote. Un refus retombe
//!    en partagé ([`ExclusivePolicy::Preferred`]) ou devient une erreur
//!    ([`ExclusivePolicy::Required`]).
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
//! 3. **Écho** — sur demande explicite ([`WasapiBackend::open_loopback`], module
//!    `loopback`) : l'endpoint est un endpoint de **rendu**, mais le flux prélève
//!    le mélange du moteur au lieu de l'alimenter
//!    (`AUDCLNT_STREAMFLAGS_LOOPBACK`, mode partagé obligatoire). C'est un
//!    `IAudioCaptureClient` qui en sort, et le rappel reçoit des trames d'entrée.
//!
//! [`WasapiBackend::open_loopback`]: crate::WasapiBackend::open_loopback
//!
//! L'horloge du flux (`IAudioClock`, module `clock`) est obtenue ici aussi :
//! `GetService(IAudioClock)` puis `GetFrequency`, classée par rapport au format de
//! mixage. Son absence n'empêche pas l'ouverture : le flux se replie sur le
//! compteur de trames ([`ClockSource::Counter`]).
//!
//! Les objets COM créés ici (`IAudioClient`, `IAudioRenderClient` ou
//! `IAudioCaptureClient`, `IAudioClock`) partent ensuite vers le fil du flux
//! (`stream`) sous forme d'[`AgileReference`] : les interfaces du crate `windows`
//! ne sont pas `Send`, la référence agile l'est, et sa résolution depuis un autre
//! fil de l'appartement multi-fil rend le pointeur direct (les objets WASAPI sont
//! libres de fil).

use std::time::Duration;

use conduit_backend::{BackendError, DeviceDirection, DeviceId, DeviceInfo, StreamFormat};
use windows::core::{AgileReference, Interface};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IAudioClient3, IAudioClock, IAudioRenderClient,
    IMMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};

use crate::clock::{ClockScale, ClockSource};
use crate::com::{platform_error, Event};
use crate::convert::SampleType;
use crate::devices::{
    activate_client, describe, device_period, find_active, frames_from_period, hardware_format,
    mix_format, Defaults, DEFAULT_PERIOD_HNS,
};
use crate::exclusive::{required_error, try_exclusive, ExclusivePolicy, ShareMode};

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
    /// `IAudioClient::Initialize` en **capture d'écho** sur un endpoint de rendu
    /// (`AUDCLNT_STREAMFLAGS_LOOPBACK`, module `loopback`), période par défaut du
    /// moteur. Toujours partagé, toujours du float32.
    Loopback {
        /// Vrai si le format demandé n'était pas celui du mixage et que
        /// `AUTOCONVERTPCM` a été demandé en plus.
        converted: bool,
    },
    /// `IAudioClient::Initialize` en mode **exclusif** (M1b-32) : le périphérique
    /// n'est plus partagé, la période est la minimale du pilote, et le format
    /// matériel n'est pas forcément celui du rappel.
    Exclusive {
        /// Format d'échantillon que le matériel a accepté. Différent de
        /// [`SampleType::F32`] = Conduit convertit lui-même dans le fil du flux.
        sample: SampleType,
        /// Période obtenue, en unités de 100 ns.
        period_hns: i64,
        /// Période obtenue, en trames.
        period_frames: u32,
        /// Vrai si `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` a imposé une reprise avec
        /// la taille de tampon que le pilote a lui-même choisie.
        realigned: bool,
    },
}

impl InitPath {
    /// Mode de partage de ce chemin.
    pub fn share_mode(self) -> ShareMode {
        match self {
            Self::LowLatency { .. } | Self::Converted | Self::Loopback { .. } => ShareMode::Shared,
            Self::Exclusive { .. } => ShareMode::Exclusive,
        }
    }

    /// Format d'échantillon du tampon WASAPI : `f32` sauf en exclusif sur un
    /// matériel entier.
    pub fn sample_type(self) -> SampleType {
        match self {
            Self::LowLatency { .. } | Self::Converted | Self::Loopback { .. } => SampleType::F32,
            Self::Exclusive { sample, .. } => sample,
        }
    }

    /// Vrai si le flux est une capture d'écho sur un endpoint de rendu.
    pub fn is_loopback(self) -> bool {
        matches!(self, Self::Loopback { .. })
    }
}

/// Latence et tailles d'un flux ouvert (hors trait `DeviceHandle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLatency {
    /// Taille du tampon (partagé avec le moteur, ou celui du pilote en exclusif),
    /// en trames (`GetBufferSize`).
    pub buffer_frames: usize,
    /// Trames par réveil attendues (`block_frames` effectif).
    pub period_frames: usize,
    /// Durée d'une période, telle qu'elle découle de `period_frames` à la fréquence
    /// demandée : la latence « par réveil » du flux, comparable d'un mode à l'autre.
    pub period: Duration,
    /// Latence du flux rapportée par WASAPI (`GetStreamLatency`), hors tampon.
    /// Certains pilotes rendent 0 en mode partagé (observé sur une sortie HDMI
    /// NVIDIA) : la latence utile se déduit alors de `buffer_frames`.
    pub stream_latency: Duration,
    /// Chemin d'initialisation.
    pub path: InitPath,
    /// Latence estimée par l'horloge du flux, en trames du format livré, mise à
    /// jour à chaque rappel : en rendu, trames écrites depuis `start()` moins la
    /// position de lecture du matériel (`IAudioClock::GetPosition`) ; en capture,
    /// position d'écriture du matériel moins trames livrées. Zéro avant le premier
    /// rappel et avec la source [`ClockSource::Counter`].
    pub write_ahead_frames: u64,
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
    /// Format d'échantillon du tampon WASAPI ; hors [`SampleType::F32`], le fil du
    /// flux convertit lui-même (module `convert`).
    pub(crate) sample: SampleType,
    /// Octets d'une trame du tampon WASAPI (`nBlockAlign`).
    pub(crate) frame_bytes: usize,
    /// Horloge du flux, si le pilote en fournit une.
    pub(crate) clock: Option<AgileReference<IAudioClock>>,
    /// Conversion des positions de `clock` en trames du format livré.
    pub(crate) clock_scale: ClockScale,
    /// Ce que `clock` est, pour le diagnostic.
    pub(crate) clock_source: ClockSource,
}

/// Résultat d'une ouverture.
pub(crate) struct Opened {
    pub(crate) info: DeviceInfo,
    /// Format effectif : fréquence et canaux demandés, `block_frames` obtenu.
    pub(crate) format: StreamFormat,
    pub(crate) objects: StreamObjects,
    /// Pourquoi le mode exclusif a été refusé, en [`ExclusivePolicy::Preferred`]
    /// seulement (le flux rendu est alors partagé).
    pub(crate) exclusive_refusal: Option<String>,
}

/// Ouvre un flux sur l'endpoint `id`. À appeler depuis le fil MMDevice.
///
/// `policy` décide du mode de partage : partagé par défaut, exclusif tenté ou exigé
/// selon [`ExclusivePolicy`]. `loopback` demande une **capture d'écho** sur un
/// endpoint de rendu (module `loopback`) : partagé obligatoire, service de capture
/// au lieu du service de rendu.
pub(crate) fn open(
    enumerator: &IMMDeviceEnumerator,
    id: &DeviceId,
    format: StreamFormat,
    policy: ExclusivePolicy,
    loopback: bool,
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
    // Seul le `DeviceInfo` sert ici : la description d'endpoint ne concerne que le
    // rattachement d'un câble à ses endpoints (`backend::resoudre_endpoints`), pas
    // l'ouverture d'un flux.
    let info = describe(&device, &Defaults::query(enumerator)?)?.info;
    if loopback {
        // Refusés avant tout appel COM : un écho ne se prend que sur un endpoint de
        // rendu, et seulement en mode partagé (module `loopback`).
        crate::loopback::check_render(id, info.direction)?;
        crate::loopback::check_shared(id, policy)?;
    }
    let event = Event::new(false)?;

    let client = activate_client(&device)?;
    let mix = mix_format(&client)?;
    let mask = mix.parsed.mask_for(channels);

    let (client, path, block_frames, exclusive_refusal) = if loopback {
        // Chemin 3 : écho. `check_shared` a déjà écarté l'exclusif, il n'y a rien à
        // négocier — le mélange du moteur, tel quel.
        let (client, path, block_frames) =
            crate::loopback::initialize(&device, id, &client, &mix, channels, format, mask)?;
        (client, path, block_frames, None)
    } else {
        // Chemin 0 : mode exclusif, uniquement si la politique le demande.
        let exclusive = policy
            .tries_exclusive()
            .then(|| try_exclusive(&device, channels, format.sample_rate, mask));
        match exclusive {
            Some(Ok(init)) => {
                let path = InitPath::Exclusive {
                    sample: init.sample,
                    period_hns: init.period_hns,
                    period_frames: init.period_frames,
                    realigned: init.realigned,
                };
                (init.client, path, init.period_frames as usize, None)
            }
            // `Required` : plutôt une erreur explicite qu'un flux partagé qu'on n'a
            // pas demandé.
            Some(Err(reason)) if policy == ExclusivePolicy::Required => {
                return Err(required_error(id, &reason))
            }
            // `Preferred` : repli en partagé, la raison voyage jusqu'à la poignée.
            Some(Err(reason)) => shared(&device, id, &client, &mix, channels, format, mask)
                .map(|(client, path, block)| (client, path, block, Some(reason)))?,
            None => shared(&device, id, &client, &mix, channels, format, mask)
                .map(|(client, path, block)| (client, path, block, None))?,
        }
    };
    let sample = path.sample_type();
    let frame_bytes = sample.frame_bytes(usize::from(channels));

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

    // En écho, l'endpoint est un endpoint de **rendu** mais le flux se comporte en
    // capture : c'est `IAudioCaptureClient` qu'il faut, pas `IAudioRenderClient`.
    let capture_side = loopback || info.direction == DeviceDirection::Capture;
    let service = if capture_side {
        // SAFETY: client initialisé en capture (ou en écho).
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| platform_error("IAudioClient::GetService(IAudioCaptureClient)", &e))?;
        Service::Capture(agile(&capture, "IAudioCaptureClient")?)
    } else {
        // SAFETY: client initialisé en rendu.
        let render: IAudioRenderClient = unsafe { client.GetService() }
            .map_err(|e| platform_error("IAudioClient::GetService(IAudioRenderClient)", &e))?;
        Service::Render(agile(&render, "IAudioRenderClient")?)
    };

    let (clock, clock_scale, clock_source) = audio_clock(
        &client,
        format.sample_rate.hz(),
        channels,
        mix.parsed.sample_rate,
        mix.parsed.block_align,
        u16::try_from(frame_bytes).unwrap_or(u16::MAX),
    )?;

    let period_frames = block_frames.max(1);
    let latency = StreamLatency {
        buffer_frames: buffer_frames as usize,
        period_frames,
        period: Duration::from_nanos(
            (period_frames as u64).saturating_mul(1_000_000_000)
                / u64::from(format.sample_rate.hz().max(1)),
        ),
        stream_latency: Duration::from_nanos(u64::try_from(latency_hns.max(0) * 100).unwrap_or(0)),
        path,
        write_ahead_frames: 0,
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
            sample,
            frame_bytes,
            clock,
            clock_scale,
            clock_source,
        },
        exclusive_refusal,
    })
}

/// Chemins partagés 1 (basse latence) et 2 (conversion). Rend le client
/// **initialisé**, le chemin retenu et `block_frames` effectif.
///
/// `probe` est le client déjà activé qui a servi à lire le format de mixage : il
/// est réutilisé si `InitializeSharedAudioStream` réussit, abandonné sinon (un
/// `IAudioClient` dont l'initialisation a échoué n'est pas réutilisable).
fn shared(
    device: &windows::Win32::Media::Audio::IMMDevice,
    id: &DeviceId,
    probe: &IAudioClient,
    mix: &crate::devices::MixFormat,
    channels: u16,
    format: StreamFormat,
    mask: u32,
) -> Result<(IAudioClient, InitPath, usize), BackendError> {
    let matches_mix = mix.parsed.float32
        && mix.parsed.channels == channels
        && mix.parsed.sample_rate == format.sample_rate.hz();
    if matches_mix {
        if let Ok((period_frames, periods)) = low_latency(probe, mix.as_ptr(), format.block_frames)
        {
            return Ok((
                probe.clone(),
                InitPath::LowLatency {
                    period_frames,
                    periods,
                },
                period_frames as usize,
            ));
        }
    }
    // Chemin 1 refusé (ou format différent du mixage) : un client neuf.
    let client = activate_client(device)?;
    let period_hns = device_period(&client)
        .map(|p| p.default)
        .unwrap_or(DEFAULT_PERIOD_HNS);
    let wanted = hardware_format(SampleType::F32, channels, format.sample_rate, mask);
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
    Ok((
        client,
        InitPath::Converted,
        frames_from_period(period_hns, format.sample_rate),
    ))
}

/// `IAudioClock` du flux et sa fréquence, classée par rapport au format de mixage
/// et au format livré (voir le module `clock`). Un pilote sans horloge, ou dont
/// `GetFrequency` échoue ou rend zéro, donne le repli [`ClockSource::Counter`] :
/// ce n'est pas une erreur d'ouverture, l'appelant le voit dans
/// [`WasapiHandle::clock_source`].
///
/// [`WasapiHandle::clock_source`]: crate::WasapiHandle::clock_source
fn audio_clock(
    client: &IAudioClient,
    sample_rate: u32,
    channels: u16,
    mix_rate: u32,
    mix_block_align: u16,
    device_block_align: u16,
) -> Result<(Option<AgileReference<IAudioClock>>, ClockScale, ClockSource), BackendError> {
    // SAFETY: client initialisé.
    let clock: Option<IAudioClock> = unsafe { client.GetService() }.ok();
    // SAFETY: horloge rendue par le client initialisé.
    let frequency = clock
        .as_ref()
        .and_then(|c| unsafe { c.GetFrequency() }.ok())
        .filter(|&f| f > 0);
    match (clock, frequency) {
        (Some(clock), Some(frequency)) => {
            let (scale, units) = ClockScale::new(
                frequency,
                sample_rate,
                channels,
                mix_rate,
                mix_block_align,
                device_block_align,
            );
            Ok((
                Some(agile(&clock, "IAudioClock")?),
                scale,
                ClockSource::AudioClock { frequency, units },
            ))
        }
        _ => Ok((
            None,
            ClockScale::identity(sample_rate),
            ClockSource::Counter,
        )),
    }
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
pub(crate) fn unsupported_or_platform(
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
