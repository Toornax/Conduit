//! Mode exclusif WASAPI (M1b-32) : politique, négociation du format matériel et
//! initialisation avec alignement du tampon.
//!
//! # Politique
//!
//! Le mode exclusif est **opt-in, par backend, jamais imposé** :
//! [`WasapiBackend::set_exclusive_policy`] fixe une [`ExclusivePolicy`] que chaque
//! ouverture suivante applique. Le défaut est [`ExclusivePolicy::Never`], et il le
//! restera : en exclusif le flux **prend le périphérique pour lui seul**, le moteur
//! audio de Windows est court-circuité et plus aucune autre application ne peut
//! jouer sur cette carte. Un câble Conduit est fait pour **coexister** avec le
//! reste du système (SPEC §5.6 : le mode exclusif est un bonus de latence, pas la
//! norme ; §2 « Windows : WASAPI mode exclusif si disponible, sinon partagé »).
//! [`ExclusivePolicy::Preferred`] tente l'exclusif et retombe en partagé en
//! conservant la raison du refus ([`WasapiHandle::exclusive_refusal`]) ;
//! [`ExclusivePolicy::Required`] échoue avec [`BackendError::UnsupportedFormat`]
//! plutôt que de rendre un flux partagé qu'on n'a pas demandé.
//!
//! # Négociation
//!
//! Il n'y a **pas** d'`AUTOCONVERTPCM` en exclusif : le format passé à `Initialize`
//! est celui que le convertisseur reçoit, tel quel. On propose donc à
//! `IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, …)`, dans cet ordre, un
//! `WAVEFORMATEXTENSIBLE` aux **fréquence et canaux demandés** : float32, PCM 24
//! dans un conteneur 32, PCM 24 compacté, PCM 16 ([`CANDIDATES`]) — le matériel
//! exclusif n'accepte souvent que l'entier. Si le pilote répond `S_FALSE` avec une
//! proposition, elle n'est retenue que si elle garde la fréquence et le nombre de
//! canaux demandés (changer l'un des deux reviendrait à ne pas honorer le format,
//! ce que le reste du backend garantit).
//!
//! # Période et alignement du tampon
//!
//! `GetDevicePeriod` donne la période **minimale** du pilote ; c'est elle qu'on
//! passe en `hnsBufferDuration` **et** `hnsPeriodicity` (l'événementiel exclusif
//! exige les deux égales). Beaucoup de pilotes refusent alors la taille de tampon
//! qui en découle avec `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` : c'est le rite de
//! passage du mode exclusif. La reprise documentée par Microsoft est de lire la
//! taille **alignée** que le pilote vient de fixer (`GetBufferSize`), d'en déduire
//! la période ([`aligned_period_hns`]), de **libérer et recréer** l'`IAudioClient`
//! (obligatoire : un client dont l'initialisation a échoué n'est pas réutilisable)
//! et de réinitialiser. Une seule reprise, puis abandon.
//!
//! [`WasapiBackend::set_exclusive_policy`]: crate::WasapiBackend::set_exclusive_policy
//! [`WasapiHandle::exclusive_refusal`]: crate::WasapiHandle::exclusive_refusal
//! [`BackendError::UnsupportedFormat`]: conduit_backend::BackendError::UnsupportedFormat

use core::fmt;

use conduit_backend::BackendError;
use conduit_core::types::SampleRate;
use windows::Win32::Foundation::{S_FALSE, S_OK};
use windows::Win32::Media::Audio::{
    IAudioClient, IMMDevice, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE,
    AUDCLNT_E_ENDPOINT_CREATE_FAILED, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED,
    AUDCLNT_E_INVALID_DEVICE_PERIOD, AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE_EXCLUSIVE,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
};

use crate::com::CoTaskMem;
use crate::convert::SampleType;
use crate::devices::{
    activate_client, device_period, frames_from_period, hardware_format, wave_format_from_bytes,
    WAVEFORMATEX_SIZE,
};

/// Unités de 100 ns dans une seconde.
const HNS_PER_SECOND: u64 = 10_000_000;

/// Formats proposés au matériel, dans l'ordre : le float32 du rappel d'abord (aucune
/// conversion), puis les entiers du plus fin au plus grossier.
pub(crate) const CANDIDATES: [SampleType; 4] = [
    SampleType::F32,
    SampleType::Pcm24In32,
    SampleType::Pcm24,
    SampleType::Pcm16,
];

/// Ce que Conduit fait du mode exclusif à l'ouverture d'un flux.
///
/// Réglage du backend, pas du trait [`Backend`](conduit_backend::Backend) : le
/// trait est portable et ne doit pas gagner une notion propre à Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExclusivePolicy {
    /// **Défaut** : toujours le mode partagé. Le périphérique reste utilisable par
    /// les autres applications.
    #[default]
    Never,
    /// Tenter l'exclusif, retomber en partagé s'il est refusé. La raison du refus
    /// est conservée sur la poignée.
    Preferred,
    /// Exiger l'exclusif : un refus devient une erreur d'ouverture, pas un flux
    /// partagé silencieux.
    Required,
}

impl ExclusivePolicy {
    /// Vrai si le chemin exclusif doit être tenté.
    pub(crate) fn tries_exclusive(self) -> bool {
        !matches!(self, Self::Never)
    }
}

impl fmt::Display for ExclusivePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Never => "partagé seulement",
            Self::Preferred => "exclusif si possible",
            Self::Required => "exclusif exigé",
        })
    }
}

/// Mode de partage effectivement obtenu par un flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareMode {
    /// Le flux passe par le moteur audio de Windows, avec les autres applications.
    Shared,
    /// Le flux a le périphérique pour lui seul.
    Exclusive,
}

impl fmt::Display for ShareMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Shared => "partagé",
            Self::Exclusive => "exclusif",
        })
    }
}

/// Période, en unités de 100 ns, qui correspond exactement à `frames` trames à
/// `rate` : la formule de reprise après `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`.
///
/// Microsoft l'écrit en flottant, `hns = 10000.0 * 1000 / rate * frames + 0,5`
/// tronqué ; on la calcule en entiers (arrondi au plus proche, demi vers le haut),
/// ce qui donne le même résultat sans dépendre de l'arrondi binaire.
///
/// La fonction est **inversible** par
/// [`frames_from_period`](crate::devices::frames_from_period) pour toutes les
/// fréquences acceptées (`SampleRate` : 8 kHz à 384 kHz) : l'erreur d'arrondi vaut
/// au plus une demi-unité de 100 ns, soit moins de 0,02 trame.
pub fn aligned_period_hns(frames: u32, rate: SampleRate) -> i64 {
    let rate = u64::from(rate.hz().max(1));
    // (2 × 1e7 × frames + rate) / (2 × rate) : arrondi au plus proche, demi vers
    // le haut, en 128 bits pour ne jamais déborder.
    let numerator = u128::from(2 * HNS_PER_SECOND) * u128::from(frames) + u128::from(rate);
    let hns = numerator / u128::from(2 * rate);
    i64::try_from(hns).unwrap_or(i64::MAX)
}

/// Un flux exclusif initialisé, prêt pour la suite commune de `open`.
pub(crate) struct ExclusiveInit {
    /// Client initialisé en exclusif (le service et l'horloge en sortiront).
    pub(crate) client: IAudioClient,
    /// Format d'échantillon que le matériel a accepté.
    pub(crate) sample: SampleType,
    /// Période obtenue, en unités de 100 ns.
    pub(crate) period_hns: i64,
    /// Période obtenue, en trames.
    pub(crate) period_frames: u32,
    /// Vrai si `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` a imposé une reprise.
    pub(crate) realigned: bool,
}

/// Tente d'ouvrir `device` en mode exclusif au format demandé.
///
/// `Err` porte une explication en français — la raison du refus **et** ce qu'on
/// peut y faire —, à journaliser en [`ExclusivePolicy::Preferred`] ou à remonter
/// telle quelle en [`ExclusivePolicy::Required`]. Aucun objet n'est laissé derrière
/// en cas d'échec : le client sondé et le client refusé sont détruits ici.
pub(crate) fn try_exclusive(
    device: &IMMDevice,
    channels: u16,
    rate: SampleRate,
    mask: u32,
) -> Result<ExclusiveInit, String> {
    let probe = activate_client(device)
        .map_err(|e| format!("l'IAudioClient du périphérique ne s'active pas : {e}"))?;
    let sample = negotiate(&probe, channels, rate, mask).ok_or_else(|| {
        format!(
            "aucun format exclusif accepté à {rate} × {channels} canaux (essayés : {}) : \
             le matériel n'expose pas cette combinaison en exclusif — essayez la fréquence \
             et le nombre de canaux du format par défaut du périphérique (Paramètres > \
             Son > Propriétés > Avancé)",
            CANDIDATES
                .iter()
                .map(SampleType::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let minimum = device_period(&probe)
        .map(|p| p.minimum)
        .map_err(|e| format!("IAudioClient::GetDevicePeriod a échoué : {e}"))?;
    // Le client de sondage n'a servi qu'à interroger : `Initialize` part d'un neuf.
    drop(probe);
    initialize(device, sample, channels, rate, mask, minimum)
}

/// Premier [`CANDIDATES`] que `IsFormatSupported` accepte en exclusif.
fn negotiate(
    client: &IAudioClient,
    channels: u16,
    rate: SampleRate,
    mask: u32,
) -> Option<SampleType> {
    let mut suggested = None;
    for &sample in &CANDIDATES {
        match supported(client, sample, channels, rate, mask) {
            Supported::Yes => return Some(sample),
            // Une proposition du pilote : notée, essayée seulement si aucun de nos
            // candidats ne passe tel quel.
            Supported::Closest(other) => suggested = suggested.or(Some(other)),
            Supported::No => {}
        }
    }
    // La proposition doit à son tour être acceptée telle que *nous* la construisons.
    let other = suggested?;
    matches!(
        supported(client, other, channels, rate, mask),
        Supported::Yes
    )
    .then_some(other)
}

/// Verdict de `IsFormatSupported` pour un format candidat.
enum Supported {
    /// `S_OK` : le format passe tel quel.
    Yes,
    /// `S_FALSE` avec une proposition qui garde fréquence et canaux, traduite en
    /// l'un de nos formats.
    Closest(SampleType),
    /// Refusé, ou proposition inexploitable.
    No,
}

/// `IsFormatSupported(EXCLUSIVE, …)` pour un format candidat.
///
/// En exclusif, Microsoft documente `S_OK` ou `AUDCLNT_E_UNSUPPORTED_FORMAT` et un
/// `*ppClosestMatch` toujours nul ; on gère quand même `S_FALSE` + proposition, que
/// certains pilotes rendent, plutôt que de dépendre d'un comportement non observé.
fn supported(
    client: &IAudioClient,
    sample: SampleType,
    channels: u16,
    rate: SampleRate,
    mask: u32,
) -> Supported {
    let wanted = hardware_format(sample, channels, rate, mask);
    let mut closest: *mut WAVEFORMATEX = core::ptr::null_mut();
    // SAFETY: `wanted` vit pendant l'appel et commence par un `WAVEFORMATEX` ;
    // `closest` est une locale vivante, et le bloc éventuellement écrit nous revient
    // (`CoTaskMem` le libère).
    let hr = unsafe {
        client.IsFormatSupported(
            AUDCLNT_SHAREMODE_EXCLUSIVE,
            core::ptr::addr_of!(wanted).cast::<WAVEFORMATEX>(),
            Some(&mut closest),
        )
    };
    // SAFETY: `closest` est nul ou un bloc `CoTaskMemAlloc` dont la propriété nous
    // revient.
    let closest = unsafe { CoTaskMem::from_raw(closest) };
    if hr == S_OK {
        return Supported::Yes;
    }
    if hr != S_FALSE {
        return Supported::No;
    }
    let ptr = closest.as_ptr();
    if ptr.is_null() {
        return Supported::No;
    }
    // SAFETY: `ptr` pointe un `WAVEFORMATEX` complet alloué par le pilote ; la
    // structure est `packed(1)`, on lit `cbSize` par `read_unaligned` puis les
    // `18 + cbSize` octets qui suivent.
    let bytes = unsafe {
        let cb_size = usize::from(core::ptr::addr_of!((*ptr).cbSize).read_unaligned());
        core::slice::from_raw_parts(ptr.cast::<u8>(), WAVEFORMATEX_SIZE + cb_size)
    };
    match wave_format_from_bytes(bytes) {
        // La proposition ne vaut que si elle honore le format demandé : changer la
        // fréquence ou le nombre de canaux n'est pas à nous de le décider.
        Some(proposed) if proposed.channels == channels && proposed.sample_rate == rate.hz() => {
            match proposed.sample_type() {
                Some(other) => Supported::Closest(other),
                None => Supported::No,
            }
        }
        _ => Supported::No,
    }
}

/// `Initialize` en exclusif, avec la reprise d'alignement du tampon.
fn initialize(
    device: &IMMDevice,
    sample: SampleType,
    channels: u16,
    rate: SampleRate,
    mask: u32,
    minimum_hns: i64,
) -> Result<ExclusiveInit, String> {
    let mut period_hns = minimum_hns.max(1);
    let mut realigned = false;
    // Deux tours au plus : le second n'a lieu qu'après un désalignement du tampon.
    for _ in 0..2 {
        let client = activate_client(device)
            .map_err(|e| format!("l'IAudioClient du périphérique ne s'active pas : {e}"))?;
        let format = hardware_format(sample, channels, rate, mask);
        // SAFETY: `format` vit pendant l'appel et commence par un `WAVEFORMATEX` ;
        // l'événementiel exclusif exige `hnsBufferDuration == hnsPeriodicity` ;
        // aucun GUID de session.
        let result = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_EXCLUSIVE,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                period_hns,
                period_hns,
                core::ptr::addr_of!(format).cast::<WAVEFORMATEX>(),
                None,
            )
        };
        match result {
            Ok(()) => {
                return Ok(ExclusiveInit {
                    client,
                    sample,
                    period_hns,
                    period_frames: u32::try_from(frames_from_period(period_hns, rate))
                        .unwrap_or(u32::MAX),
                    realigned,
                });
            }
            Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED && !realigned => {
                // Le pilote a arrêté sa propre taille de tampon : on la lui reprend
                // (seul appel valide sur un client dont `Initialize` a échoué ainsi).
                // SAFETY: le client existe encore ; `GetBufferSize` est l'appel que
                // Microsoft prescrit dans ce cas précis.
                let aligned = unsafe { client.GetBufferSize() }.map_err(|e| {
                    format!(
                        "tampon désaligné, et IAudioClient::GetBufferSize a échoué \
                         ensuite : {} (HRESULT {:#010x})",
                        e.message().trim(),
                        e.code().0
                    )
                })?;
                // Recréation **obligatoire** : ce client n'est plus réutilisable.
                drop(client);
                period_hns = aligned_period_hns(aligned, rate);
                realigned = true;
            }
            Err(e) => return Err(explain(&e, sample, channels, rate, period_hns, realigned)),
        }
    }
    Err(format!(
        "la taille de tampon exclusive reste désalignée après une reprise à \
         {period_hns} × 100 ns : le pilote de ce périphérique n'accepte pas la période \
         qu'il a lui-même proposée"
    ))
}

/// Traduit un `HRESULT` d'`Initialize` en exclusif en une phrase qui dit **ce qui**
/// a échoué et **quoi faire**.
fn explain(
    error: &windows::core::Error,
    sample: SampleType,
    channels: u16,
    rate: SampleRate,
    period_hns: i64,
    realigned: bool,
) -> String {
    let code = error.code();
    let what = if code == AUDCLNT_E_DEVICE_IN_USE {
        "un autre programme utilise déjà ce périphérique en mode exclusif : fermez-le \
         (ou ouvrez ce flux en mode partagé)"
            .to_string()
    } else if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
        "le mode exclusif est désactivé pour ce périphérique : Paramètres > Système > \
         Son > Propriétés du périphérique > Avancé, case « Autoriser les applications à \
         prendre le contrôle exclusif de ce périphérique »"
            .to_string()
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
        format!(
            "format refusé à l'initialisation alors qu'IsFormatSupported l'acceptait \
             ({sample}, {channels} canaux, {rate}) : demandez le format par défaut du \
             périphérique"
        )
    } else if code == AUDCLNT_E_INVALID_DEVICE_PERIOD {
        format!(
            "période refusée ({period_hns} × 100 ns{}) : le pilote n'accepte pas sa \
             propre période minimale, restez en mode partagé sur ce périphérique",
            if realigned {
                ", après réalignement"
            } else {
                ""
            }
        )
    } else if code == AUDCLNT_E_ENDPOINT_CREATE_FAILED {
        "le service audio n'a pas pu créer l'endpoint exclusif : le périphérique est \
         peut-être en veille ou en cours de reconfiguration"
            .to_string()
    } else if code == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
        format!("taille de tampon désalignée, non rattrapée à {period_hns} × 100 ns")
    } else {
        format!("échec inattendu ({})", error.message().trim())
    };
    format!("{what} [HRESULT {:#010x}]", code.0)
}

/// [`BackendError::UnsupportedFormat`] pour un refus en [`ExclusivePolicy::Required`].
pub(crate) fn required_error(device: &conduit_backend::DeviceId, reason: &str) -> BackendError {
    BackendError::UnsupportedFormat {
        device: device.clone(),
        reason: format!(
            "mode exclusif exigé (ExclusivePolicy::Required) mais indisponible — {reason}. \
             Passez la politique à Preferred pour retomber automatiquement en mode partagé"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::frames_from_period;
    use proptest::prelude::*;

    #[test]
    fn policy_defaults_to_shared() {
        assert_eq!(ExclusivePolicy::default(), ExclusivePolicy::Never);
        assert!(!ExclusivePolicy::Never.tries_exclusive());
        assert!(ExclusivePolicy::Preferred.tries_exclusive());
        assert!(ExclusivePolicy::Required.tries_exclusive());
        assert_eq!(ExclusivePolicy::Never.to_string(), "partagé seulement");
        assert_eq!(ShareMode::Exclusive.to_string(), "exclusif");
        assert_eq!(ShareMode::Shared.to_string(), "partagé");
    }

    #[test]
    fn aligned_period_matches_the_microsoft_formula() {
        // 480 trames à 48 kHz = 10 ms = 100 000 × 100 ns.
        assert_eq!(aligned_period_hns(480, SampleRate::HZ_48000), 100_000);
        assert_eq!(aligned_period_hns(144, SampleRate::HZ_48000), 30_000);
        // 448 trames à 44,1 kHz : 1e7 × 448 / 44 100 = 101 587,3 → 101 587.
        assert_eq!(aligned_period_hns(448, SampleRate::HZ_44100), 101_587);
        assert_eq!(aligned_period_hns(0, SampleRate::HZ_48000), 0);
        // La formule flottante de Microsoft donne la même chose.
        for frames in [16u32, 128, 441, 448, 480, 960, 1_056, 8_192] {
            for rate in [
                SampleRate::HZ_44100,
                SampleRate::HZ_48000,
                SampleRate::HZ_96000,
            ] {
                let microsoft =
                    (10_000.0 * 1_000.0 / f64::from(rate.hz()) * f64::from(frames) + 0.5) as i64;
                assert_eq!(
                    aligned_period_hns(frames, rate),
                    microsoft,
                    "{frames} trames à {rate}"
                );
            }
        }
    }

    #[test]
    fn required_error_says_what_to_do() {
        let e = required_error(&"dev".into(), "raison du pilote");
        let text = e.to_string();
        assert!(text.contains("raison du pilote"), "{text}");
        assert!(text.contains("Preferred"), "{text}");
        assert!(matches!(
            e,
            BackendError::UnsupportedFormat { ref device, .. } if device.as_str() == "dev"
        ));
    }

    proptest! {
        /// La période alignée redonne exactement le nombre de trames par la formule
        /// inverse (`frames_from_period`), pour toute taille de tampon plausible et
        /// toute fréquence acceptée.
        #[test]
        fn aligned_period_round_trips_through_frames(
            frames in 1u32..1_000_000,
            hz in SampleRate::MIN..=SampleRate::MAX,
        ) {
            let rate = SampleRate::new(hz).expect("hz dans la plage");
            let hns = aligned_period_hns(frames, rate);
            prop_assert!(hns > 0, "période nulle pour {frames} trames à {rate}");
            prop_assert_eq!(
                frames_from_period(hns, rate),
                frames as usize,
                "{} × 100 ns ne redonne pas {} trames à {}",
                hns,
                frames,
                rate
            );
        }
    }
}
