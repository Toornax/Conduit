//! Codes `NTSTATUS` et alias de types du noyau utilisés par le contrat COM.
//!
//! `ntstatus.h` n'est pas dans les bindings de `portcls-sys` ; les quelques codes dont
//! le modèle objet a besoin sont recopiés ici, avec la même représentation que côté
//! WDK (`LONG` signé, donc `i32`).

/// `NTSTATUS` : entier signé 32 bits, négatif en cas d'échec.
pub type NtStatus = i32;

/// `ULONG` : compte de références renvoyé par `AddRef`/`Release`.
pub type ULong = u32;

/// Pointeur opaque (`PVOID`) : le `this` des méthodes COM et les paramètres de sortie
/// de `QueryInterface`.
pub type RawPtr = *mut core::ffi::c_void;

/// Succès.
pub const STATUS_SUCCESS: NtStatus = 0;

/// Échec générique (`0xC0000001`).
pub const STATUS_UNSUCCESSFUL: NtStatus = 0xC000_0001_u32 as i32;

/// Fonction non implémentée (`0xC0000002`) : réponse d'un slot de vtable non pris en
/// charge.
pub const STATUS_NOT_IMPLEMENTED: NtStatus = 0xC000_0002_u32 as i32;

/// Paramètre invalide (`0xC000000D`) : réponse de `QueryInterface` à un IID inconnu
/// ou à un pointeur de sortie nul (comportement SYSVAD).
pub const STATUS_INVALID_PARAMETER: NtStatus = 0xC000_000D_u32 as i32;

/// Mémoire insuffisante (`0xC000009A`) : échec d'allocation dans le pool non paginé.
pub const STATUS_INSUFFICIENT_RESOURCES: NtStatus = 0xC000_009A_u32 as i32;

/// `NT_SUCCESS(status)` : vrai pour les codes de succès et d'information (`status >= 0`).
pub const fn nt_success(status: NtStatus) -> bool {
    status >= 0
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    #[test]
    fn valeurs_du_wdk() {
        assert_eq!(STATUS_SUCCESS, 0);
        assert_eq!(STATUS_UNSUCCESSFUL, -1_073_741_823);
        assert_eq!(STATUS_NOT_IMPLEMENTED, -1_073_741_822);
        assert_eq!(STATUS_INVALID_PARAMETER, -1_073_741_811);
        assert_eq!(STATUS_INSUFFICIENT_RESOURCES, -1_073_741_670);
        assert_eq!(STATUS_INVALID_PARAMETER as u32, 0xC000_000D);
    }

    #[test]
    fn nt_success_signe() {
        assert!(nt_success(STATUS_SUCCESS));
        assert!(nt_success(0x4000_0000)); // information
        assert!(!nt_success(STATUS_UNSUCCESSFUL));
        assert!(!nt_success(STATUS_INVALID_PARAMETER));
        assert!(!nt_success(STATUS_INSUFFICIENT_RESOURCES));
    }

    #[test]
    fn rawptr_a_la_taille_d_un_pointeur() {
        assert_eq!(
            core::mem::size_of::<RawPtr>(),
            core::mem::size_of::<usize>()
        );
        assert_eq!(core::mem::size_of::<ULong>(), 4);
        assert_eq!(core::mem::size_of::<NtStatus>(), 4);
    }
}
