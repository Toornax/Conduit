//! Gardes RAII autour de COM : appartement du fil, mémoire `CoTaskMem`, `PROPVARIANT`.
//!
//! Tout ce que l'API MMDevice alloue pour nous doit être rendu avec la bonne
//! fonction : `CoTaskMemFree` pour les chaînes de `GetId` et les `WAVEFORMATEX` de
//! `GetMixFormat` / `IsFormatSupported`, `PropVariantClear` pour les valeurs de
//! `IPropertyStore::GetValue`. Ces gardes rendent l'oubli impossible.

use conduit_backend::BackendError;
use windows::core::PWSTR;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VT_BLOB, VT_LPWSTR};

/// Traduit l'échec d'un appel COM en [`BackendError::Platform`] qui nomme l'appel.
pub(crate) fn platform_error(call: &str, error: &windows::core::Error) -> BackendError {
    BackendError::Platform(format!(
        "{call} a échoué : {} (HRESULT {:#010x})",
        error.message().trim(),
        error.code().0
    ))
}

/// Appartement COM du fil courant, libéré à la destruction.
///
/// Le client de notification MMDevice rappelle depuis un fil de COM : l'appartement
/// multi-fil (MTA) est le seul qui convienne à un fil sans boucle de messages.
pub(crate) struct ComApartment(());

impl ComApartment {
    /// Initialise COM en MTA sur le fil courant.
    pub(crate) fn initialize_mta() -> Result<Self, BackendError> {
        // SAFETY: `CoInitializeEx` n'a pas de précondition ; l'appel est apparié au
        // `CoUninitialize` de `Drop`, y compris quand il renvoie `S_FALSE` (déjà
        // initialisé sur ce fil), comme l'exige la documentation.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(platform_error(
                "CoInitializeEx",
                &windows::core::Error::from(hr),
            ));
        }
        Ok(Self(()))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: apparié à l'appel réussi de `CoInitializeEx` sur ce même fil ;
        // toutes les interfaces COM du fil sont détruites avant (ordre des locales).
        unsafe { CoUninitialize() };
    }
}

/// Chaîne UTF-16 allouée par COM (`IMMDevice::GetId`), libérée par `CoTaskMemFree`.
pub(crate) struct CoTaskString(PWSTR);

impl CoTaskString {
    /// Prend possession d'une chaîne rendue par COM.
    ///
    /// # Safety
    ///
    /// `ptr` est nul ou pointe une chaîne UTF-16 terminée par NUL, allouée par
    /// `CoTaskMemAlloc`, dont l'appelant cède la propriété.
    pub(crate) unsafe fn from_raw(ptr: PWSTR) -> Self {
        Self(ptr)
    }

    /// Copie en `String` (`None` si la chaîne est nulle ou n'est pas de l'UTF-16 valide).
    pub(crate) fn to_string(&self) -> Option<String> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: invariant de `from_raw` : chaîne terminée par NUL, vivante tant
        // que `self` l'est.
        unsafe { self.0.to_string() }.ok()
    }
}

impl Drop for CoTaskString {
    fn drop(&mut self) {
        // SAFETY: invariant de `from_raw` : mémoire de `CoTaskMemAlloc`, libérée
        // une seule fois. `CoTaskMemFree` accepte un pointeur nul.
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

/// Bloc alloué par COM (`WAVEFORMATEX` de `GetMixFormat`, `ppClosestMatch`),
/// libéré par `CoTaskMemFree`.
pub(crate) struct CoTaskMem<T>(*mut T);

impl<T> CoTaskMem<T> {
    /// Garde initialement vide, à remplir par un paramètre de sortie.
    pub(crate) fn null() -> Self {
        Self(core::ptr::null_mut())
    }

    /// Prend possession d'un bloc rendu par COM.
    ///
    /// # Safety
    ///
    /// `ptr` est nul ou pointe un bloc alloué par `CoTaskMemAlloc` dont l'appelant
    /// cède la propriété.
    pub(crate) unsafe fn from_raw(ptr: *mut T) -> Self {
        Self(ptr)
    }

    /// Emplacement à passer comme paramètre de sortie (`*mut *mut T`).
    pub(crate) fn slot(&mut self) -> *mut *mut T {
        &mut self.0
    }

    /// Pointeur brut (nul si rien n'a été alloué).
    pub(crate) fn as_ptr(&self) -> *const T {
        self.0
    }
}

impl<T> Drop for CoTaskMem<T> {
    fn drop(&mut self) {
        // SAFETY: invariant de `from_raw` / `slot` : mémoire de `CoTaskMemAlloc`,
        // libérée une seule fois. `CoTaskMemFree` accepte un pointeur nul.
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

/// `PROPVARIANT` possédé, nettoyé par `PropVariantClear`.
pub(crate) struct PropVariant(PROPVARIANT);

impl PropVariant {
    /// Prend possession d'une valeur rendue par `IPropertyStore::GetValue`.
    pub(crate) fn from_owned(value: PROPVARIANT) -> Self {
        Self(value)
    }

    fn vt(&self) -> windows::Win32::System::Variant::VARENUM {
        // SAFETY: `vt` est le premier champ de toute variante de l'union, toujours
        // initialisé (`GetValue` renvoie au pire `VT_EMPTY`).
        unsafe { self.0.Anonymous.Anonymous.vt }
    }

    /// Valeur `VT_LPWSTR` copiée en `String`.
    pub(crate) fn as_wide_string(&self) -> Option<String> {
        if self.vt() != VT_LPWSTR {
            return None;
        }
        // SAFETY: le discriminant `vt` vaut `VT_LPWSTR`, donc `pwszVal` est la
        // variante active : chaîne UTF-16 terminée par NUL, possédée par `self`.
        let ptr = unsafe { self.0.Anonymous.Anonymous.Anonymous.pwszVal };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: pointeur non nul de la variante active, vivant tant que `self`.
        unsafe { ptr.to_string() }.ok()
    }

    /// Valeur `VT_BLOB` vue comme une tranche d'octets.
    pub(crate) fn as_blob(&self) -> Option<&[u8]> {
        if self.vt() != VT_BLOB {
            return None;
        }
        // SAFETY: le discriminant `vt` vaut `VT_BLOB`, donc `blob` est la variante
        // active.
        let blob = unsafe { self.0.Anonymous.Anonymous.Anonymous.blob };
        if blob.pBlobData.is_null() || blob.cbSize == 0 {
            return None;
        }
        // SAFETY: `pBlobData` pointe `cbSize` octets initialisés, possédés par `self`
        // et vivants aussi longtemps que la référence rendue.
        Some(unsafe { core::slice::from_raw_parts(blob.pBlobData, blob.cbSize as usize) })
    }
}

impl Drop for PropVariant {
    fn drop(&mut self) {
        // SAFETY: `self.0` est un `PROPVARIANT` initialisé par COM et possédé ici ;
        // `PropVariantClear` le remet à `VT_EMPTY`, l'appel n'a lieu qu'une fois.
        let _ = unsafe { PropVariantClear(&mut self.0) };
    }
}
