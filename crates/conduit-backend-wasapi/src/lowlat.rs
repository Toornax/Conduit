//! Ouverture **partagée faible latence** (`IAudioClient3`) : politique, périodes du
//! moteur audio et initialisation.
//!
//! # Pourquoi une politique, à côté de l'exclusif
//!
//! Un flux partagé ordinaire s'ouvre par `IAudioClient::Initialize` : le moteur
//! audio choisit alors sa période **par défaut** (10 ms) et, sur le pilote Conduit,
//! il alloue le tampon par **scrutation** (`AllocateAudioBuffer`, aucune
//! notification, aucun paquet) — mesuré en machine virtuelle, alors même que le
//! pilote expose les interfaces modernes. L'hypothèse à rendre testable est que le
//! moteur ne monte le chemin **événementiel** (notifications, paquets WaveRT) que
//! pour un flux ouvert en **faible latence**, c'est-à-dire par
//! `IAudioClient3::InitializeSharedAudioStream` avec une période courte.
//!
//! [`SharedPeriod`] est donc le pendant partagé d'[`ExclusivePolicy`] : un réglage
//! du backend, **opt-in**, [`SharedPeriod::Default`] par défaut — le comportement
//! actuel, inchangé. [`SharedPeriod::Minimal`] et [`SharedPeriod::Requested`]
//! **exigent** le chemin `IAudioClient3` : s'il n'est pas disponible, l'ouverture
//! échoue en disant pourquoi, elle ne retombe pas en silence sur la période par
//! défaut. Un repli silencieux ferait mesurer le moteur audio en croyant mesurer le
//! transport, et rien dans les chiffres ne le trahirait.
//!
//! # Le format est celui du mélange
//!
//! `InitializeSharedAudioStream` n'accepte **pas** `AUTOCONVERTPCM` : le seul
//! indicateur qu'il prend est `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`. Le format passé
//! est donc celui de `GetMixFormat`, tel quel — c'est aussi celui pour lequel la
//! période minimale est annoncée ; avec un format converti, le moteur refuse
//! couramment la période courte (`AUDCLNT_E_ENGINE_PERIODICITY_LOCKED`,
//! `AUDCLNT_E_INVALID_STREAM_FLAG`). En politique forcée, un format demandé
//! différent du mélange est donc refusé **avant** tout appel COM
//! ([`check_mix`]), avec le format à demander dans le message.
//!
//! # La période
//!
//! `GetSharedModeEnginePeriod` rend quatre valeurs en trames ([`EnginePeriods`]) :
//! la période par défaut, la **fondamentale** — toute période acceptée en est un
//! multiple —, le minimum du pilote et le maximum du moteur. [`choose_period`]
//! traduit un souhait en période acceptable : le multiple de la fondamentale
//! **immédiatement supérieur ou égal**, borné à `[min, max]`. C'est aussi ce que
//! [`SharedPeriod::Requested`] fait d'un souhait exprimé en millisecondes — jamais
//! plus court que ce qui est demandé, jamais hors des bornes du moteur.
//!
//! Le **pompage** du flux ne change pas : `InitializeSharedAudioStream` est
//! initialisé avec `EVENTCALLBACK`, `open` lui pose son `SetEventHandle` et le fil
//! du flux (module `stream`) dort sur cet événement comme pour tout autre flux
//! partagé — le backend n'a jamais pompé à la minuterie.
//!
//! [`ExclusivePolicy`]: crate::ExclusivePolicy

use core::fmt;

use conduit_backend::{BackendError, DeviceId};
use windows::core::Interface;
use windows::Win32::Media::Audio::{
    IAudioClient, IAudioClient3, AUDCLNT_E_ENGINE_FORMAT_LOCKED,
    AUDCLNT_E_ENGINE_PERIODICITY_LOCKED, AUDCLNT_E_INVALID_STREAM_FLAG,
    AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
};

use crate::com::CoTaskMem;
use crate::devices::{MixFormat, WaveFormat};
use crate::exclusive::ExclusivePolicy;

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

/// Trames que durent `ms` millisecondes à `rate` hertz, arrondies au plus proche et
/// jamais nulles.
///
/// Une valeur absurde (négative, NaN, démesurée) donne 1 trame : la borne basse de
/// [`choose_period`] la ramènera au minimum du moteur, ce qui est la lecture la plus
/// charitable d'une demande qu'on ne peut pas honorer telle quelle.
pub fn frames_from_ms(ms: f64, rate: u32) -> u32 {
    let frames = (ms * f64::from(rate) / 1000.0).round();
    if !frames.is_finite() || frames < 1.0 {
        return 1;
    }
    if frames >= f64::from(u32::MAX) {
        return u32::MAX;
    }
    frames as u32
}

/// Ce que Conduit demande au moteur audio comme **période** à l'ouverture d'un flux
/// partagé.
///
/// Réglage du backend, pas du trait [`Backend`](conduit_backend::Backend) : le trait
/// est portable et ne doit pas gagner une notion propre à Windows. Le défaut est
/// [`SharedPeriod::Default`], et il le restera : c'est le comportement que
/// `conduitd` a toujours eu.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SharedPeriod {
    /// **Défaut** : la période que le moteur choisit. Le chemin `IAudioClient3` est
    /// tenté quand le format demandé est celui du mélange, avec la plus petite
    /// période ≥ `block_frames` ; sinon le flux s'ouvre par
    /// `IAudioClient::Initialize` avec conversion, à la période par défaut du
    /// moteur. Un échec du premier chemin retombe sur le second, sans bruit.
    #[default]
    Default,
    /// **Exiger** `IAudioClient3` avec la période **minimale** annoncée par
    /// `GetSharedModeEnginePeriod` pour le format de mixage. Un refus est une
    /// erreur d'ouverture, jamais un repli.
    Minimal,
    /// **Exiger** `IAudioClient3` avec la période demandée, en **millisecondes**,
    /// arrondie au multiple de la fondamentale immédiatement supérieur ou égal puis
    /// bornée à `[min, max]` ([`choose_period`]). Un refus est une erreur
    /// d'ouverture.
    Requested(f64),
}

impl SharedPeriod {
    /// Vrai si cette politique **exige** le chemin `IAudioClient3` : un échec
    /// devient alors une erreur d'ouverture.
    pub fn forces(self) -> bool {
        !matches!(self, Self::Default)
    }

    /// Période à demander, en trames, pour les périodes annoncées par le moteur.
    ///
    /// `block_frames` n'est lu qu'en [`SharedPeriod::Default`] : c'est le souhait de
    /// l'appelant, que le chemin automatique honore au plus près. `rate` est la
    /// fréquence du **format de mixage** — celui qu'`InitializeSharedAudioStream`
    /// reçoit —, la seule à laquelle une durée se traduise en trames.
    pub fn period_frames(self, rate: u32, block_frames: usize, periods: EnginePeriods) -> u32 {
        match self {
            Self::Default => choose_period(block_frames, periods),
            Self::Minimal => choose_period(periods.min as usize, periods),
            Self::Requested(ms) => choose_period(frames_from_ms(ms, rate) as usize, periods),
        }
    }
}

impl fmt::Display for SharedPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("période au choix du moteur"),
            Self::Minimal => f.write_str("période minimale du moteur (IAudioClient3)"),
            Self::Requested(ms) => write!(f, "période demandée : {ms} ms (IAudioClient3)"),
        }
    }
}

/// Un flux partagé faible latence initialisé, prêt pour la suite commune de `open`.
pub(crate) struct LowLatencyInit {
    /// Période **demandée** à `InitializeSharedAudioStream`, en trames.
    pub(crate) period_frames: u32,
    /// Période que le moteur dit servir après l'initialisation
    /// (`GetCurrentSharedModeEnginePeriod`), en trames. Elle diffère de la
    /// précédente quand un autre flux tient déjà la périodicité du moteur.
    pub(crate) current_period_frames: u32,
    /// Les quatre périodes annoncées pour le format de mixage.
    pub(crate) periods: EnginePeriods,
}

/// Ouvre `client` en partagé faible latence, au format de mixage.
///
/// `Err` porte une explication en français — la raison du refus **et** ce qu'on peut
/// y faire — que l'appelant remonte telle quelle en politique forcée, ou ignore en
/// [`SharedPeriod::Default`] (où le repli par conversion prend le relais).
pub(crate) fn try_low_latency(
    client: &IAudioClient,
    mix: &MixFormat,
    period: SharedPeriod,
    block_frames: usize,
) -> Result<LowLatencyInit, String> {
    let client3: IAudioClient3 = client.cast().map_err(|e| {
        format!(
            "cet endpoint n'expose pas IAudioClient3 (QueryInterface : {} [HRESULT {:#010x}]) : \
             l'ouverture partagée faible latence demande Windows 10 version 1703 ou plus \
             récent, et un pilote audio qui la prend en charge",
            e.message().trim(),
            e.code().0
        )
    })?;
    let periods = engine_periods(&client3, mix.as_ptr())?;
    let wanted = period.period_frames(mix.parsed.sample_rate, block_frames, periods);
    // SAFETY: `mix` pointe le bloc rendu par `GetMixFormat`, vivant pendant l'appel ;
    // `EVENTCALLBACK` est le seul indicateur qu'`InitializeSharedAudioStream`
    // accepte ; aucun GUID de session.
    unsafe {
        client3.InitializeSharedAudioStream(
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            wanted,
            mix.as_ptr(),
            None,
        )
    }
    .map_err(|e| explain(&e, period, wanted, periods))?;
    Ok(LowLatencyInit {
        period_frames: wanted,
        // Le moteur peut servir une autre période que celle demandée : c'est
        // précisément le chiffre que la mesure veut lire, pas celui qu'on espérait.
        current_period_frames: current_period(&client3).unwrap_or(wanted),
        periods,
    })
}

/// Périodes du moteur pour un format (`GetSharedModeEnginePeriod`).
fn engine_periods(
    client3: &IAudioClient3,
    format: *const WAVEFORMATEX,
) -> Result<EnginePeriods, String> {
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
    .map_err(|e| {
        format!(
            "IAudioClient3::GetSharedModeEnginePeriod a échoué : {} [HRESULT {:#010x}]",
            e.message().trim(),
            e.code().0
        )
    })?;
    if periods.fundamental == 0 || periods.max == 0 {
        return Err(
            "IAudioClient3::GetSharedModeEnginePeriod a rendu des périodes nulles".to_string(),
        );
    }
    Ok(periods)
}

/// Période que le moteur sert à ce flux après l'initialisation, en trames.
///
/// `None` si l'appel échoue ou rend zéro : l'appelant retient alors la période
/// demandée, faute de mieux — mais il ne l'invente pas, il la dit.
fn current_period(client3: &IAudioClient3) -> Option<u32> {
    let mut format: *mut WAVEFORMATEX = core::ptr::null_mut();
    let mut frames = 0u32;
    // SAFETY: client initialisé ; les deux sorties sont des locales vivantes, et le
    // format éventuellement écrit nous revient (`CoTaskMem` le libère).
    let ok = unsafe { client3.GetCurrentSharedModeEnginePeriod(&mut format, &mut frames) }.is_ok();
    // SAFETY: `format` est nul ou un bloc `CoTaskMemAlloc` dont la propriété nous
    // revient.
    let _format = unsafe { CoTaskMem::from_raw(format) };
    (ok && frames > 0).then_some(frames)
}

/// Traduit un `HRESULT` d'`InitializeSharedAudioStream` en une phrase qui dit **ce
/// qui** a échoué et **quoi faire**.
fn explain(
    error: &windows::core::Error,
    period: SharedPeriod,
    wanted: u32,
    periods: EnginePeriods,
) -> String {
    let code = error.code();
    let bornes = format!(
        "le moteur annonce {} à {} trames par pas de {} (défaut {})",
        periods.min, periods.max, periods.fundamental, periods.default
    );
    let what = if code == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED {
        format!(
            "un autre flux tient déjà la périodicité du moteur pour cet endpoint : \
             {wanted} trames ne peuvent plus lui être imposées ({bornes}). Fermez les \
             applications qui jouent ou enregistrent sur cet endpoint, puis recommencez"
        )
    } else if code == AUDCLNT_E_ENGINE_FORMAT_LOCKED {
        "un autre flux tient déjà le format du moteur pour cet endpoint : le format de \
         mixage ne peut plus être imposé. Fermez les applications qui l'utilisent, puis \
         recommencez"
            .to_string()
    } else if code == AUDCLNT_E_INVALID_STREAM_FLAG {
        "indicateur refusé : InitializeSharedAudioStream n'accepte que \
         AUDCLNT_STREAMFLAGS_EVENTCALLBACK — ni AUTOCONVERTPCM, ni LOOPBACK, ni le mode \
         exclusif. Le format doit donc être celui du mélange, sans conversion"
            .to_string()
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
        format!(
            "format refusé pour un flux faible latence : demandez le format de mixage de \
             cet endpoint, tel que GetMixFormat le rend ({bornes})"
        )
    } else {
        format!("échec inattendu ({})", error.message().trim())
    };
    format!("{period} — {what} [HRESULT {:#010x}]", code.0)
}

/// Refuse une politique de période forcée là où `IAudioClient3` n'a pas cours.
///
/// Deux combinaisons n'ont pas de sens et sont écartées **avant** tout appel COM,
/// plutôt que de laisser Windows rendre un `HRESULT` obscur :
///
/// - **mode exclusif** : un flux exclusif court-circuite le moteur audio, dont on
///   demanderait ici la période ; les deux politiques se contredisent ;
/// - **écho** (`AUDCLNT_STREAMFLAGS_LOOPBACK`) : `InitializeSharedAudioStream` ne
///   prend pas cet indicateur (voir le module `loopback`), la faible latence n'y est
///   pas disponible.
pub(crate) fn check_compatible(
    id: &DeviceId,
    policy: ExclusivePolicy,
    period: SharedPeriod,
    loopback: bool,
) -> Result<(), BackendError> {
    if !period.forces() {
        return Ok(());
    }
    if policy.tries_exclusive() {
        return Err(BackendError::UnsupportedFormat {
            device: id.clone(),
            reason: format!(
                "l'ouverture partagée faible latence ({period}) demande au **moteur audio** une \
                 période courte, et le mode exclusif est précisément celui où il n'y a plus de \
                 moteur : les deux politiques se contredisent. Politique de partage en \
                 vigueur : {policy}. Gardez l'une des deux"
            ),
        });
    }
    if loopback {
        return Err(BackendError::UnsupportedFormat {
            device: id.clone(),
            reason: format!(
                "la capture en écho s'ouvre par IAudioClient::Initialize avec \
                 AUDCLNT_STREAMFLAGS_LOOPBACK, qu'InitializeSharedAudioStream n'accepte pas : \
                 la faible latence n'existe pas pour un écho ({period}). Remettez la politique \
                 à SharedPeriod::Default avant d'ouvrir un écho"
            ),
        });
    }
    Ok(())
}

/// Refuse un format demandé différent du **mélange** quand la politique force le
/// chemin `IAudioClient3`, en nommant le format à demander.
///
/// `InitializeSharedAudioStream` n'a pas d'`AUTOCONVERTPCM` : ouvrir au format de
/// mixage alors qu'on a demandé autre chose reviendrait à ne pas honorer le format,
/// ce que le reste de ce backend garantit.
pub(crate) fn check_mix(
    id: &DeviceId,
    mix: &WaveFormat,
    channels: u16,
    sample_rate: u32,
    period: SharedPeriod,
) -> Result<(), BackendError> {
    if !period.forces() {
        return Ok(());
    }
    if mix.float32 && mix.channels == channels && mix.sample_rate == sample_rate {
        return Ok(());
    }
    Err(BackendError::UnsupportedFormat {
        device: id.clone(),
        reason: format!(
            "l'ouverture partagée faible latence ({period}) passe le format de mixage tel quel \
             à InitializeSharedAudioStream, qui n'a pas de conversion automatique : demandez \
             {} Hz, {} canaux, float32 — le format de mixage de cet endpoint — au lieu de \
             {sample_rate} Hz, {channels} canaux",
            mix.sample_rate, mix.channels
        ),
    })
}

/// [`BackendError::UnsupportedFormat`] pour un refus en politique forcée.
pub(crate) fn required_error(id: &DeviceId, period: SharedPeriod, reason: &str) -> BackendError {
    BackendError::UnsupportedFormat {
        device: id.clone(),
        reason: format!(
            "ouverture partagée faible latence exigée ({period}) mais indisponible — {reason}. \
             Remettez la politique à SharedPeriod::Default \
             (WasapiBackend::set_shared_period) pour l'ouverture partagée ordinaire"
        ),
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

    /// La politique par défaut ne force rien, et c'est ce que `conduitd` a toujours
    /// fait : la période suit `block_frames`, comme avant.
    #[test]
    fn la_politique_par_defaut_ne_force_rien() {
        assert_eq!(SharedPeriod::default(), SharedPeriod::Default);
        assert!(!SharedPeriod::Default.forces());
        assert!(SharedPeriod::Minimal.forces());
        assert!(SharedPeriod::Requested(3.0).forces());
        assert_eq!(
            SharedPeriod::Default.period_frames(48_000, 480, MAXWELL_LIKE),
            480
        );
        assert_eq!(
            SharedPeriod::Default.period_frames(48_000, 256, MAXWELL_LIKE),
            288
        );
    }

    /// La période minimale est celle du pilote, pas une valeur devinée.
    #[test]
    fn la_periode_minimale_est_celle_du_moteur() {
        assert_eq!(
            SharedPeriod::Minimal.period_frames(48_000, 480, MAXWELL_LIKE),
            144
        );
        // Un minimum qui ne serait pas un multiple de la fondamentale reste honoré :
        // la borne basse rattrape l'arrondi vers le haut.
        let bancal = EnginePeriods {
            min: 100,
            ..MAXWELL_LIKE
        };
        let choisie = SharedPeriod::Minimal.period_frames(48_000, 480, bancal);
        assert!(choisie >= bancal.min && choisie <= bancal.max, "{choisie}");
    }

    /// Une période demandée en millisecondes devient des trames à la fréquence du
    /// **mélange**, arrondies au multiple de la fondamentale immédiatement supérieur
    /// ou égal, puis bornées.
    #[test]
    fn la_periode_demandee_monte_au_multiple_de_la_fondamentale() {
        // 3 ms à 48 kHz = 144 trames, déjà un multiple de 48.
        assert_eq!(frames_from_ms(3.0, 48_000), 144);
        assert_eq!(
            SharedPeriod::Requested(3.0).period_frames(48_000, 480, MAXWELL_LIKE),
            144
        );
        // 5 ms = 240 trames, multiple de 48 : inchangé.
        assert_eq!(
            SharedPeriod::Requested(5.0).period_frames(48_000, 480, MAXWELL_LIKE),
            240
        );
        // 4 ms = 192 trames ; 4,1 ms = 196,8 → 197 trames, qui monte à 240.
        assert_eq!(frames_from_ms(4.1, 48_000), 197);
        assert_eq!(
            SharedPeriod::Requested(4.1).period_frames(48_000, 480, MAXWELL_LIKE),
            240
        );
        // Plus court que le minimum, plus long que le maximum : borné aux deux bouts.
        assert_eq!(
            SharedPeriod::Requested(0.5).period_frames(48_000, 480, MAXWELL_LIKE),
            144
        );
        assert_eq!(
            SharedPeriod::Requested(500.0).period_frames(48_000, 480, MAXWELL_LIKE),
            960
        );
        // La fréquence du mélange, et non 48 kHz supposés : 3 ms à 44,1 kHz = 132,3
        // → 132 trames, qui monte à 144.
        assert_eq!(frames_from_ms(3.0, 44_100), 132);
    }

    /// Une durée absurde ne panique pas et ne rend jamais zéro trame.
    #[test]
    fn une_duree_absurde_retombe_sur_une_trame() {
        for ms in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(frames_from_ms(ms, 48_000), 1, "{ms}");
        }
        assert_eq!(frames_from_ms(1e12, 48_000), u32::MAX);
    }

    #[test]
    fn la_politique_se_dit_en_francais() {
        assert_eq!(
            SharedPeriod::Default.to_string(),
            "période au choix du moteur"
        );
        assert!(SharedPeriod::Minimal.to_string().contains("minimale"));
        let demandee = SharedPeriod::Requested(2.5).to_string();
        assert!(demandee.contains("2.5"), "{demandee}");
        assert!(demandee.contains("ms"), "{demandee}");
    }

    /// L'exclusif et la faible latence partagée se contredisent, et l'écho n'a pas
    /// de faible latence : les deux refus le disent et nomment la sortie.
    #[test]
    fn la_politique_forcee_refuse_l_exclusif_et_l_echo() {
        let id = DeviceId::new("{endpoint}");
        // Politique par défaut : rien n'est refusé, quoi qu'on demande à côté.
        assert!(
            check_compatible(&id, ExclusivePolicy::Required, SharedPeriod::Default, true).is_ok()
        );

        let e = check_compatible(&id, ExclusivePolicy::Required, SharedPeriod::Minimal, false)
            .expect_err("exclusif + faible latence");
        let texte = e.to_string();
        assert!(texte.contains("moteur audio"), "{texte}");
        assert!(texte.contains("exclusif"), "{texte}");

        let e = check_compatible(&id, ExclusivePolicy::Never, SharedPeriod::Minimal, true)
            .expect_err("écho + faible latence");
        let texte = e.to_string();
        assert!(texte.contains("LOOPBACK"), "{texte}");
        assert!(texte.contains("SharedPeriod::Default"), "{texte}");
        assert!(matches!(
            e,
            BackendError::UnsupportedFormat { ref device, .. } if device.as_str() == "{endpoint}"
        ));
    }

    /// Le format de mixage, tel quel, ou un refus qui **nomme** le format à
    /// demander : `InitializeSharedAudioStream` n'a pas de conversion automatique,
    /// et ouvrir au format du mélange alors qu'on a demandé autre chose reviendrait
    /// à ne pas honorer le format.
    #[test]
    fn la_politique_forcee_exige_le_format_du_melange() {
        let id = DeviceId::new("{endpoint}");
        let melange = WaveFormat {
            channels: 2,
            sample_rate: 48_000,
            block_align: 8,
            channel_mask: Some(3),
            bits: 32,
            valid_bits: 32,
            float32: true,
            pcm: false,
        };
        assert!(check_mix(&id, &melange, 2, 48_000, SharedPeriod::Minimal).is_ok());
        // Politique par défaut : le chemin par conversion existe toujours, rien à
        // refuser.
        assert!(check_mix(&id, &melange, 6, 44_100, SharedPeriod::Default).is_ok());

        for (canaux, hz) in [(6u16, 48_000u32), (2, 44_100)] {
            let e = check_mix(&id, &melange, canaux, hz, SharedPeriod::Minimal)
                .expect_err("format différent du mélange");
            let texte = e.to_string();
            assert!(texte.contains("48000 Hz"), "{texte}");
            assert!(texte.contains("float32"), "{texte}");
        }
        // Un mélange entier (jamais vu en partagé, mais le contrat le prévoit) est
        // refusé aussi : le rappel attend du float32.
        let entier = WaveFormat {
            float32: false,
            pcm: true,
            ..melange
        };
        assert!(check_mix(&id, &entier, 2, 48_000, SharedPeriod::Minimal).is_err());
    }

    #[test]
    fn le_refus_force_dit_comment_revenir_en_arriere() {
        let e = required_error(
            &DeviceId::new("dev"),
            SharedPeriod::Minimal,
            "raison du moteur",
        );
        let texte = e.to_string();
        assert!(texte.contains("raison du moteur"), "{texte}");
        assert!(texte.contains("SharedPeriod::Default"), "{texte}");
    }
}
