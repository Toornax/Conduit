//! Volume maître et coupure d'un endpoint (`IAudioEndpointVolume`).
//!
//! C'est le réglage que Windows expose dans le mélangeur : un volume **scalaire**
//! de 0 à 1 (la position du curseur, pas des décibels) et un interrupteur de
//! coupure. Il s'applique à tout ce qui traverse l'endpoint — y compris la capture
//! en écho —, ce qui en fait la première cause d'une mesure silencieuse, et la
//! moins soupçonnée.
//!
//! L'interface s'obtient par [`IMMDevice::Activate`], comme l'`IAudioClient` : un
//! endpoint est activable en plusieurs interfaces, chacune servant un usage.
//!
//! Tout ceci est **hors du trait [`Backend`]** : celui-ci est portable (PipeWire,
//! CoreAudio) et ne doit pas gagner une notion propre à Windows, au même titre que
//! `WasapiBackend::set_exclusive_policy` et `WasapiBackend::open_loopback`. C'est
//! aussi pourquoi [`EndpointVolumeControl`] possède son propre appartement COM et
//! son propre énumérateur : il ne passe pas par le fil MMDevice du backend, et sert
//! donc même sans backend ouvert (`conduit-looptest --list --show-volume`).
//!
//! [`Backend`]: conduit_backend::Backend

use conduit_backend::{BackendError, DeviceId};
use windows::core::HRESULT;
use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::com::{platform_error, ComApartment};
use crate::devices::find_active;

/// `pguidEventContext` nul : la façon documentée de dire « pas de contexte » à
/// `IAudioEndpointVolume`. Le contexte ne sert qu'à reconnaître ses propres
/// changements dans un `IAudioEndpointVolumeCallback` ; personne n'écoute ici.
const NO_EVENT_CONTEXT: *const windows::core::GUID = core::ptr::null();

/// Volume maître et coupure d'un endpoint, tels qu'`IAudioEndpointVolume` les rend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EndpointVolume {
    /// Volume maître **scalaire** : 0 (rien ne passe) à 1 (curseur au maximum).
    ///
    /// C'est la position du curseur du mélangeur, pas une atténuation en décibels :
    /// Windows applique lui-même la courbe entre les deux
    /// (`GetMasterVolumeLevelScalar`).
    pub scalar: f32,
    /// Vrai si l'endpoint est coupé (muet), quel que soit le volume.
    pub muted: bool,
}

impl EndpointVolume {
    /// Vrai si rien ne peut sortir de cet endpoint : coupé, ou volume nul.
    ///
    /// C'est la question qui compte avant une mesure : une chaîne audio par
    /// ailleurs parfaite ne rend que du silence dans ce cas.
    pub fn is_silent(&self) -> bool {
        self.muted || self.scalar <= 0.0
    }

    /// Volume en pourcentage entier, arrondi (0 à 100).
    pub fn percent(&self) -> u32 {
        (Self::clamp_scalar(self.scalar) * 100.0).round() as u32
    }

    /// Borne un volume scalaire dans `[0, 1]` ; `NaN` devient 0.
    ///
    /// `SetMasterVolumeLevelScalar` refuse (`E_INVALIDARG`) tout ce qui sort de
    /// l'intervalle : on borne plutôt que d'échouer, un curseur ne va pas au-delà
    /// de ses butées.
    pub fn clamp_scalar(scalar: f32) -> f32 {
        if scalar.is_nan() {
            0.0
        } else {
            scalar.clamp(0.0, 1.0)
        }
    }
}

/// Lecture et écriture du volume des endpoints audio.
///
/// L'objet possède son appartement COM (MTA) et son `IMMDeviceEnumerator` : il
/// n'est ni `Send` ni `Sync` et doit vivre et mourir sur le fil qui l'a créé — les
/// interfaces COM qu'il détient doivent être détruites avant le `CoUninitialize`
/// que sa destruction déclenche, ce que l'ordre des champs garantit.
///
/// ```no_run
/// # #[cfg(windows)]
/// # fn main() -> Result<(), conduit_backend::BackendError> {
/// use conduit_backend::DeviceId;
/// use conduit_backend_wasapi::EndpointVolumeControl;
///
/// let control = EndpointVolumeControl::new()?;
/// let id = DeviceId::new("{0.0.0.00000000}.{...}");
/// if let Some(volume) = control.read(&id)? {
///     println!("{} %, coupé : {}", volume.percent(), volume.muted);
/// }
/// # Ok(())
/// # }
/// # #[cfg(not(windows))]
/// # fn main() {}
/// ```
pub struct EndpointVolumeControl {
    /// Détruit **avant** l'appartement : l'ordre de déclaration est l'ordre de
    /// destruction des champs, et une interface COM ne survit pas à
    /// `CoUninitialize`.
    enumerator: IMMDeviceEnumerator,
    _apartment: ComApartment,
}

impl core::fmt::Debug for EndpointVolumeControl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EndpointVolumeControl")
            .finish_non_exhaustive()
    }
}

impl EndpointVolumeControl {
    /// Initialise COM en MTA sur le fil courant et crée l'énumérateur MMDevice.
    ///
    /// # Erreurs
    ///
    /// [`BackendError::Platform`] si COM ou l'énumérateur refusent ; le message
    /// nomme l'appel fautif et son `HRESULT`.
    pub fn new() -> Result<Self, BackendError> {
        let apartment = ComApartment::initialize_mta()?;
        // SAFETY: COM vient d'être initialisé sur ce fil ; `MMDeviceEnumerator` est
        // le CLSID documenté de l'énumérateur, sans agrégation.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| platform_error("CoCreateInstance(MMDeviceEnumerator)", &e))?;
        Ok(Self {
            enumerator,
            _apartment: apartment,
        })
    }

    /// Volume et coupure de l'endpoint `id`.
    ///
    /// `Ok(None)` quand l'endpoint **n'expose pas de contrôle de volume** : c'est
    /// une réponse, pas une panne, et l'appelant n'a alors rien à conclure du
    /// silence.
    ///
    /// # Erreurs
    ///
    /// [`BackendError::NotFound`] si l'endpoint est inconnu ou n'est pas actif ;
    /// [`BackendError::Platform`] si un appel COM échoue.
    pub fn read(&self, id: &DeviceId) -> Result<Option<EndpointVolume>, BackendError> {
        match self.activate(id)? {
            Some(volume) => read_volume(&volume),
            None => Ok(None),
        }
    }

    /// Règle le volume maître scalaire de l'endpoint `id` et **relit** le résultat.
    ///
    /// `scalar` est borné à `[0, 1]` par [`EndpointVolume::clamp_scalar`]. La
    /// valeur relue peut différer légèrement de celle demandée : Windows la range
    /// sur les crans du périphérique.
    ///
    /// `Ok(None)` quand l'endpoint n'expose pas de contrôle de volume.
    ///
    /// # Erreurs
    ///
    /// Comme [`Self::read`].
    pub fn set_scalar(
        &self,
        id: &DeviceId,
        scalar: f32,
    ) -> Result<Option<EndpointVolume>, BackendError> {
        let Some(volume) = self.activate(id)? else {
            return Ok(None);
        };
        // SAFETY: interface valide ; la valeur est bornée à [0, 1] comme l'exige
        // `SetMasterVolumeLevelScalar`, et le contexte d'événement nul est la façon
        // documentée de dire « pas de contexte » (aucun rappel à corréler ici).
        match unsafe {
            volume
                .SetMasterVolumeLevelScalar(EndpointVolume::clamp_scalar(scalar), NO_EVENT_CONTEXT)
        } {
            Ok(()) => {}
            Err(e) if is_unsupported(e.code()) => return Ok(None),
            Err(e) => {
                return Err(platform_error(
                    "IAudioEndpointVolume::SetMasterVolumeLevelScalar",
                    &e,
                ))
            }
        }
        read_volume(&volume)
    }

    /// Coupe ou rétablit le son de l'endpoint `id` et **relit** le résultat.
    ///
    /// `Ok(None)` quand l'endpoint n'expose pas de contrôle de volume.
    ///
    /// # Erreurs
    ///
    /// Comme [`Self::read`].
    pub fn set_mute(
        &self,
        id: &DeviceId,
        muted: bool,
    ) -> Result<Option<EndpointVolume>, BackendError> {
        let Some(volume) = self.activate(id)? else {
            return Ok(None);
        };
        // SAFETY: interface valide ; contexte d'événement nul (voir `set_scalar`).
        match unsafe { volume.SetMute(muted, NO_EVENT_CONTEXT) } {
            Ok(()) => {}
            Err(e) if is_unsupported(e.code()) => return Ok(None),
            Err(e) => return Err(platform_error("IAudioEndpointVolume::SetMute", &e)),
        }
        read_volume(&volume)
    }

    /// Retrouve l'endpoint actif `id` et l'active en `IAudioEndpointVolume`.
    fn activate(&self, id: &DeviceId) -> Result<Option<IAudioEndpointVolume>, BackendError> {
        let device = find_active(&self.enumerator, id.as_str())?
            .ok_or_else(|| BackendError::NotFound(id.clone()))?;
        activate_volume(&device)
    }
}

/// Active l'`IAudioEndpointVolume` d'un endpoint ; `None` s'il n'en a pas.
fn activate_volume(device: &IMMDevice) -> Result<Option<IAudioEndpointVolume>, BackendError> {
    // SAFETY: interface valide ; aucun paramètre d'activation (comme pour
    // `IAudioClient`, voir `devices::activate_client`).
    match unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) } {
        Ok(volume) => Ok(Some(volume)),
        Err(e) if is_unsupported(e.code()) => Ok(None),
        Err(e) => Err(platform_error(
            "IMMDevice::Activate(IAudioEndpointVolume)",
            &e,
        )),
    }
}

/// Lit volume et coupure ; `None` si le périphérique ne les implémente pas.
fn read_volume(volume: &IAudioEndpointVolume) -> Result<Option<EndpointVolume>, BackendError> {
    // SAFETY: interface valide, rendue par `Activate`.
    let scalar = match unsafe { volume.GetMasterVolumeLevelScalar() } {
        Ok(scalar) => scalar,
        Err(e) if is_unsupported(e.code()) => return Ok(None),
        Err(e) => {
            return Err(platform_error(
                "IAudioEndpointVolume::GetMasterVolumeLevelScalar",
                &e,
            ))
        }
    };
    // SAFETY: interface valide, rendue par `Activate`.
    let muted = match unsafe { volume.GetMute() } {
        Ok(muted) => muted.as_bool(),
        Err(e) if is_unsupported(e.code()) => return Ok(None),
        Err(e) => return Err(platform_error("IAudioEndpointVolume::GetMute", &e)),
    };
    Ok(Some(EndpointVolume {
        scalar: EndpointVolume::clamp_scalar(scalar),
        muted,
    }))
}

/// Vrai pour les `HRESULT` qui disent « ce périphérique n'a pas de contrôle de
/// volume » plutôt que « l'appel a échoué ».
///
/// Un endpoint sans mélangeur matériel ni logiciel (certains périphériques de
/// diffusion, des pilotes minimalistes) refuse l'activation par `E_NOINTERFACE`,
/// ou l'accepte puis répond `E_NOTIMPL` aux accesseurs. Ce n'est pas une panne, et
/// l'appelant doit pouvoir le distinguer d'une erreur pour ne pas accuser le volume
/// à tort.
fn is_unsupported(code: HRESULT) -> bool {
    code == E_NOINTERFACE || code == E_NOTIMPL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_volume_scalaire_est_borne() {
        assert_eq!(EndpointVolume::clamp_scalar(0.5), 0.5);
        assert_eq!(EndpointVolume::clamp_scalar(0.0), 0.0);
        assert_eq!(EndpointVolume::clamp_scalar(1.0), 1.0);
        assert_eq!(EndpointVolume::clamp_scalar(-0.5), 0.0);
        assert_eq!(EndpointVolume::clamp_scalar(2.5), 1.0);
        assert_eq!(EndpointVolume::clamp_scalar(f32::NAN), 0.0);
        assert_eq!(EndpointVolume::clamp_scalar(f32::INFINITY), 1.0);
        assert_eq!(EndpointVolume::clamp_scalar(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn le_pourcentage_arrondit() {
        let at = |scalar| EndpointVolume {
            scalar,
            muted: false,
        };
        assert_eq!(at(0.0).percent(), 0);
        assert_eq!(at(0.5).percent(), 50);
        assert_eq!(at(0.755).percent(), 76);
        assert_eq!(at(1.0).percent(), 100);
        assert_eq!(at(2.0).percent(), 100);
    }

    #[test]
    fn le_silence_vient_de_la_coupure_ou_du_zero() {
        let volume = |scalar, muted| EndpointVolume { scalar, muted };
        assert!(volume(0.0, false).is_silent());
        assert!(volume(1.0, true).is_silent());
        assert!(volume(0.0, true).is_silent());
        assert!(!volume(0.01, false).is_silent());
        assert!(!volume(1.0, false).is_silent());
    }

    #[test]
    fn seuls_e_nointerface_et_e_notimpl_valent_absence_de_controle() {
        assert!(is_unsupported(E_NOINTERFACE));
        assert!(is_unsupported(E_NOTIMPL));
        assert!(!is_unsupported(HRESULT(0)));
        assert!(!is_unsupported(windows::Win32::Foundation::E_ACCESSDENIED));
    }
}
