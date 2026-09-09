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
//!    (`lowlat::choose_period`). Aucune conversion, période au choix entre le
//!    minimum du pilote et le maximum du moteur. `SharedPeriod` (module `lowlat`)
//!    permet d'**exiger** ce chemin et d'y imposer la période minimale, ou une
//!    période précise : le refus devient alors une erreur d'ouverture au lieu d'un
//!    repli sur le chemin 2.
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
    IAudioCaptureClient, IAudioClient, IAudioClock, IAudioRenderClient, IMMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
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
use crate::lowlat::{self, EnginePeriods, SharedPeriod};

/// Comment le flux a été initialisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitPath {
    /// `IAudioClient3::InitializeSharedAudioStream` au format de mixage, avec la
    /// période choisie (en trames). Voir le module `lowlat` et [`SharedPeriod`].
    LowLatency {
        /// Période **demandée** à `InitializeSharedAudioStream`.
        period_frames: u32,
        /// Période que le moteur dit servir après l'initialisation
        /// (`GetCurrentSharedModeEnginePeriod`). Différente de la précédente quand
        /// un autre flux tenait déjà la périodicité du moteur.
        current_period_frames: u32,
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
/// selon [`ExclusivePolicy`]. `period` décide de la **période** d'un flux partagé :
/// au choix du moteur par défaut, ou faible latence exigée par `IAudioClient3`
/// ([`SharedPeriod`], module `lowlat`). `loopback` demande une **capture d'écho** sur
/// un endpoint de rendu (module `loopback`) : partagé obligatoire, service de capture
/// au lieu du service de rendu.
pub(crate) fn open(
    enumerator: &IMMDeviceEnumerator,
    id: &DeviceId,
    format: StreamFormat,
    policy: ExclusivePolicy,
    period: SharedPeriod,
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
    // Une politique de période forcée écarte l'exclusif et l'écho, avant tout appel
    // COM : les combinaisons se contredisent (module `lowlat`).
    lowlat::check_compatible(id, policy, period, loopback)?;
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
            Some(Err(reason)) => shared(&device, id, &client, &mix, channels, format, mask, period)
                .map(|(client, path, block)| (client, path, block, Some(reason)))?,
            None => shared(&device, id, &client, &mix, channels, format, mask, period)
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
///
/// `period` décide de la suite : en [`SharedPeriod::Default`], le chemin 1 est
/// **tenté** quand le format demandé est celui du mixage et son échec retombe
/// silencieusement sur le chemin 2 ; sinon le chemin 1 est **exigé** et tout refus
/// devient une erreur d'ouverture qui dit pourquoi (module `lowlat`).
fn shared(
    device: &windows::Win32::Media::Audio::IMMDevice,
    id: &DeviceId,
    probe: &IAudioClient,
    mix: &crate::devices::MixFormat,
    channels: u16,
    format: StreamFormat,
    mask: u32,
    period: SharedPeriod,
) -> Result<(IAudioClient, InitPath, usize), BackendError> {
    let matches_mix = mix.parsed.float32
        && mix.parsed.channels == channels
        && mix.parsed.sample_rate == format.sample_rate.hz();
    // Politique forcée : le format demandé doit être celui du mélange, faute de
    // conversion automatique sur ce chemin.
    lowlat::check_mix(id, &mix.parsed, channels, format.sample_rate.hz(), period)?;
    if matches_mix {
        match lowlat::try_low_latency(probe, mix, period, format.block_frames) {
            Ok(init) => {
                return Ok((
                    probe.clone(),
                    InitPath::LowLatency {
                        period_frames: init.period_frames,
                        current_period_frames: init.current_period_frames,
                        periods: init.periods,
                    },
                    init.period_frames as usize,
                ));
            }
            // Politique forcée : pas de repli silencieux — on mesurerait le moteur
            // audio en croyant mesurer le transport.
            Err(reason) if period.forces() => {
                return Err(lowlat::required_error(id, period, &reason))
            }
            Err(_) => {}
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

    /// Le chemin faible latence reste un chemin **partagé**, et son tampon reste du
    /// float32 : c'est le mode exclusif, et lui seul, qui change les deux.
    ///
    /// L'arithmétique de la période, elle, vit dans le module `lowlat` et s'y teste.
    #[test]
    fn le_chemin_faible_latence_est_partage_et_en_float32() {
        let path = InitPath::LowLatency {
            period_frames: SharedPeriod::Minimal.period_frames(48_000, 480, MAXWELL_LIKE),
            current_period_frames: 144,
            periods: MAXWELL_LIKE,
        };
        assert_eq!(path.share_mode(), ShareMode::Shared);
        assert_eq!(path.sample_type(), SampleType::F32);
        assert!(!path.is_loopback());
        assert!(matches!(
            path,
            InitPath::LowLatency {
                period_frames: 144,
                ..
            }
        ));
    }
}
