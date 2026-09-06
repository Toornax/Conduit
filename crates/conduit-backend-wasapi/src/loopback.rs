//! Capture en **écho** d'un endpoint de rendu (`AUDCLNT_STREAMFLAGS_LOOPBACK`).
//!
//! Un flux d'écho s'ouvre sur un endpoint de **rendu** et prélève le mélange que
//! le moteur audio de Windows vient d'y écrire — **avant** que le pilote du
//! périphérique ne le consomme. Le flux se comporte ensuite comme une capture
//! ordinaire : c'est un `IAudioCaptureClient` que `GetService` rend, et le rappel
//! reçoit des trames d'entrée.
//!
//! # À quoi cela sert
//!
//! C'est l'outil qui coupe en deux une chaîne audio muette. Sur un câble virtuel
//! dont la capture n'entend pas le rendu, l'écho répond à la question qu'aucune
//! inspection du pilote ne tranche :
//!
//! - le signal est **entendu en écho** → le moteur audio délivre bien vers cet
//!   endpoint, le défaut est en aval, dans l'échange de données du pilote ;
//! - l'écho est **silencieux** → rien n'arrive jusqu'au pilote, et le défaut est
//!   en amont (volume de l'endpoint, coupure, format par défaut, autre programme
//!   en mode exclusif).
//!
//! # Mode partagé seulement
//!
//! `AUDCLNT_STREAMFLAGS_LOOPBACK` n'existe qu'en `AUDCLNT_SHAREMODE_SHARED` :
//! `IAudioClient::Initialize` le refuse en exclusif, et c'est cohérent avec ce
//! que l'écho est — un prélèvement dans le **mélange du moteur audio**, alors
//! qu'un flux exclusif court-circuite précisément ce moteur pour parler au pilote
//! en direct. Il n'y a donc rien à prélever. [`check_shared`] refuse la
//! combinaison avant tout appel COM plutôt que de laisser Windows rendre un
//! `HRESULT` obscur.
//!
//! # Format
//!
//! Comme partout dans ce backend, le format demandé est honoré : le rappel reçoit
//! `format.channels` canaux `f32` entrelacés à `format.sample_rate`. Deux chemins,
//! calqués sur ceux du mode partagé ordinaire (module `open`) :
//!
//! 1. le format demandé **est** le format de mixage (float32, mêmes canaux, même
//!    fréquence) : `Initialize` avec le bloc rendu par `GetMixFormat` et les seuls
//!    `EVENTCALLBACK | LOOPBACK`. Aucune conversion — c'est le chemin normal, et
//!    celui que l'écho a de meilleur à offrir, puisqu'il livre exactement ce que
//!    le moteur a mélangé ;
//! 2. sinon : `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY` en plus, avec un
//!    `WAVEFORMATEXTENSIBLE` float32 aux valeurs demandées. Windows convertit.
//!    Tous les moteurs n'acceptent pas la conversion sur un flux d'écho : le refus
//!    devient une erreur qui nomme le format de mixage à demander.
//!
//! La période est celle par défaut du moteur (`hnsBufferDuration = 0`), comme pour
//! le chemin par conversion : `IAudioClient3::InitializeSharedAudioStream` ne
//! prend pas d'indicateur d'écho, la basse latence n'est pas disponible ici.

use conduit_backend::{BackendError, DeviceDirection, DeviceId, StreamFormat};
use windows::Win32::Media::Audio::{
    IAudioClient, IMMDevice, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};

use crate::convert::SampleType;
use crate::devices::{
    activate_client, device_period, frames_from_period, hardware_format, MixFormat,
    DEFAULT_PERIOD_HNS,
};
use crate::exclusive::ExclusivePolicy;
use crate::open::{unsupported_or_platform, InitPath};

/// Indicateurs d'`IAudioClient::Initialize` pour un flux d'écho.
///
/// `EVENTCALLBACK` : comme tous les flux de ce backend, l'écho est événementiel
/// (le fil du flux dort sur l'événement du tampon). Le moteur ne signale
/// l'événement d'un écho que **tant qu'il mélange quelque chose** vers cet
/// endpoint ; c'est sans conséquence ici, puisqu'on joue en même temps qu'on
/// écoute, et la boucle du flux se réveille de toute façon toutes les deux
/// secondes.
///
/// `convert` demande la conversion automatique (chemin 2 ci-dessus) : à ne poser
/// que si le format demandé diffère du format de mixage.
pub(crate) const fn stream_flags(convert: bool) -> u32 {
    let base = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK;
    if convert {
        base | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
    } else {
        base
    }
}

/// Refuse un écho sur un endpoint de **capture** : il n'y a pas de mélange à
/// prélever sur une entrée, et Windows rendrait `AUDCLNT_E_WRONG_ENDPOINT_TYPE`.
pub(crate) fn check_render(id: &DeviceId, direction: DeviceDirection) -> Result<(), BackendError> {
    if direction == DeviceDirection::Render {
        return Ok(());
    }
    Err(BackendError::UnsupportedFormat {
        device: id.clone(),
        reason: "la capture en écho ne s'ouvre que sur un endpoint de rendu : elle prélève le \
                 mélange que le moteur audio y écrit, et un endpoint de capture n'en a pas. \
                 Ouvrez-le comme une capture ordinaire (Backend::open)"
            .to_string(),
    })
}

/// Refuse la combinaison écho + mode exclusif.
///
/// `AUDCLNT_STREAMFLAGS_LOOPBACK` n'est valide qu'en `AUDCLNT_SHAREMODE_SHARED`
/// (voir l'en-tête du module) : plutôt que d'envoyer à Windows une combinaison
/// qu'il refusera, on la refuse ici avec la raison et la marche à suivre.
pub(crate) fn check_shared(id: &DeviceId, policy: ExclusivePolicy) -> Result<(), BackendError> {
    if !policy.tries_exclusive() {
        return Ok(());
    }
    Err(BackendError::UnsupportedFormat {
        device: id.clone(),
        reason: format!(
            "la capture en écho n'existe qu'en mode partagé : AUDCLNT_STREAMFLAGS_LOOPBACK est \
             refusé en AUDCLNT_SHAREMODE_EXCLUSIVE, et un flux exclusif court-circuite \
             justement le moteur audio dont l'écho prélève le mélange — il n'y aurait rien à \
             prélever. Politique du backend en vigueur : {policy}. Remettez-la à \
             ExclusivePolicy::Never (WasapiBackend::set_exclusive_policy) avant d'ouvrir un écho"
        ),
    })
}

/// `IAudioClient::Initialize` d'un flux d'écho. Rend le client **initialisé**, le
/// chemin retenu et `block_frames` effectif, comme le `shared` du module `open`.
///
/// `probe` est le client déjà activé qui a servi à lire le format de mixage : il
/// n'est utilisé ici que pour interroger la période du moteur ; `Initialize` part
/// d'un client neuf, puisqu'un client dont l'initialisation échoue n'est plus
/// réutilisable.
pub(crate) fn initialize(
    device: &IMMDevice,
    id: &DeviceId,
    probe: &IAudioClient,
    mix: &MixFormat,
    channels: u16,
    format: StreamFormat,
    mask: u32,
) -> Result<(IAudioClient, InitPath, usize), BackendError> {
    let matches_mix = mix.parsed.float32
        && mix.parsed.channels == channels
        && mix.parsed.sample_rate == format.sample_rate.hz();
    let period_hns = device_period(probe)
        .map(|p| p.default)
        .unwrap_or(DEFAULT_PERIOD_HNS);
    let client = activate_client(device)?;
    let wanted = hardware_format(SampleType::F32, channels, format.sample_rate, mask);
    let (flags, wave) = if matches_mix {
        (stream_flags(false), mix.as_ptr())
    } else {
        (
            stream_flags(true),
            core::ptr::addr_of!(wanted).cast::<WAVEFORMATEX>(),
        )
    };
    // SAFETY: `wave` pointe soit le bloc rendu par `GetMixFormat` (vivant tant que
    // `mix` l'est), soit la locale `wanted`, vivante pendant l'appel ; les deux
    // commencent par un `WAVEFORMATEX`. Période et tampon au choix du moteur
    // (0, 0), aucun GUID de session.
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, wave, None) }.map_err(
        |e| match unsupported_or_platform(id, "IAudioClient::Initialize (écho)", &e) {
            // Le refus le plus probable est celui de la conversion : le message dit
            // quel format demander pour rester sur le chemin sans conversion.
            BackendError::UnsupportedFormat { device, reason } => BackendError::UnsupportedFormat {
                device,
                reason: format!(
                    "{reason} — la capture en écho de cet endpoint n'accepte que son format de \
                     mixage ({} Hz, {} canaux, float32) ; c'est celui que le moteur mélange, \
                     demandez-le tel quel",
                    mix.parsed.sample_rate, mix.parsed.channels
                ),
            },
            other => other,
        },
    )?;
    Ok((
        client,
        InitPath::Loopback {
            converted: !matches_mix,
        },
        frames_from_period(period_hns, format.sample_rate),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusive::ShareMode;

    #[test]
    fn les_indicateurs_portent_toujours_l_echo_et_l_evenement() {
        let simple = stream_flags(false);
        assert_eq!(
            simple & AUDCLNT_STREAMFLAGS_LOOPBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK
        );
        assert_eq!(
            simple & AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        );
        // Sans conversion : rien d'autre.
        assert_eq!(
            simple,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK
        );
        assert_eq!(simple & AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, 0);
    }

    #[test]
    fn la_conversion_ajoute_autoconvert_et_la_qualite_par_defaut() {
        let converti = stream_flags(true);
        assert_eq!(converti & stream_flags(false), stream_flags(false));
        assert_eq!(
            converti & AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        );
        assert_eq!(
            converti & AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        );
    }

    #[test]
    fn l_echo_est_refuse_en_exclusif() {
        let id = DeviceId::new("{endpoint}");
        assert!(check_shared(&id, ExclusivePolicy::Never).is_ok());
        for policy in [ExclusivePolicy::Preferred, ExclusivePolicy::Required] {
            let e = check_shared(&id, policy).expect_err("écho + exclusif");
            let text = e.to_string();
            assert!(text.contains("mode partagé"), "{text}");
            assert!(text.contains("Never"), "{text}");
            assert!(text.contains(&policy.to_string()), "{text}");
            assert!(matches!(
                e,
                BackendError::UnsupportedFormat { ref device, .. } if device.as_str() == "{endpoint}"
            ));
        }
    }

    #[test]
    fn l_echo_est_refuse_sur_une_capture() {
        let id = DeviceId::new("{endpoint}");
        assert!(check_render(&id, DeviceDirection::Render).is_ok());
        let e = check_render(&id, DeviceDirection::Capture).expect_err("écho sur une capture");
        assert!(e.to_string().contains("endpoint de rendu"), "{e}");
    }

    #[test]
    fn le_chemin_d_echo_est_partage_et_en_float32() {
        for converted in [false, true] {
            let path = InitPath::Loopback { converted };
            assert_eq!(path.share_mode(), ShareMode::Shared);
            assert_eq!(path.sample_type(), SampleType::F32);
        }
    }
}
