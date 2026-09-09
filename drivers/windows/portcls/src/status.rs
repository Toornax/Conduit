//! Codes `NTSTATUS` du contrat PortCls absents de `conduit-com` (`ntstatus.h` n'est pas
//! dans les bindings de `portcls-sys` ; `wdk-sys` n'est pas une dépendance ici).

use conduit_com::NtStatus;

/// `STATUS_BUFFER_OVERFLOW` (`0x80000005`, avertissement : `NT_SUCCESS` faux) : réponse
/// de `DataRangeIntersection` à une interrogation de taille (tampon de sortie absent ou
/// de longueur nulle), la taille requise étant écrite dans `ResultantFormatLength`.
pub const STATUS_BUFFER_OVERFLOW: NtStatus = 0x8000_0005_u32 as i32;

/// `STATUS_BUFFER_TOO_SMALL` (`0xC0000023`) : tampon de sortie fourni mais trop petit.
pub const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xC000_0023_u32 as i32;

/// `STATUS_NO_MATCH` (`0xC0000272`) : réponse de `DataRangeIntersection` quand les deux
/// plages ne se croisent pas.
///
/// C'est le code que la documentation de `IMiniport::DataRangeIntersection` nomme — « *there
/// is no intersection* » — et il ne se confond pas avec `STATUS_NOT_IMPLEMENTED`, qui
/// **délègue** au gestionnaire par défaut de PortCls au lieu de refuser.
pub const STATUS_NO_MATCH: NtStatus = 0xC000_0272_u32 as i32;

/// `STATUS_NOT_SUPPORTED` (`0xC00000BB`) : réponse par défaut de
/// `IMiniportWaveRTStream::SetFormat` (changement de format en cours de flux refusé,
/// comme SYSVAD).
pub const STATUS_NOT_SUPPORTED: NtStatus = 0xC000_00BB_u32 as i32;

/// `STATUS_INVALID_DEVICE_REQUEST` (`0xC0000010`) : la requête n'a pas de sens pour cette
/// cible. Réponse d'un gestionnaire de propriété (voir [`crate::property`]) à un
/// `MajorTarget` nul, à une vtable inattendue (confusion de types entre une table
/// d'automatisation et le miniport qui la porte) et à un verbe hors
/// GET/SET/BASICSUPPORT.
pub const STATUS_INVALID_DEVICE_REQUEST: NtStatus = 0xC000_0010_u32 as i32;

/// `STATUS_PRIVILEGE_NOT_HELD` (`0xC0000061`) : l'appelant n'a pas le droit d'effectuer
/// l'opération. Réponse prévue d'une propriété privée de configuration à un `SET` venu
/// d'un client non autorisé.
pub const STATUS_PRIVILEGE_NOT_HELD: NtStatus = 0xC000_0061_u32 as i32;

/// `STATUS_NOT_FOUND` (`0xC0000225`) : l'objet visé n'existe pas. Réponse prévue à une
/// propriété adressée à un nœud ou à une instance (canal, câble) inconnus.
pub const STATUS_NOT_FOUND: NtStatus = 0xC000_0225_u32 as i32;

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_com::nt_success;

    #[test]
    fn valeurs_du_wdk() {
        assert_eq!(STATUS_BUFFER_OVERFLOW as u32, 0x8000_0005);
        assert_eq!(STATUS_BUFFER_TOO_SMALL as u32, 0xC000_0023);
        assert_eq!(STATUS_NO_MATCH as u32, 0xC000_0272);
        assert_eq!(STATUS_NOT_SUPPORTED as u32, 0xC000_00BB);
        assert_eq!(STATUS_INVALID_DEVICE_REQUEST as u32, 0xC000_0010);
        assert_eq!(STATUS_PRIVILEGE_NOT_HELD as u32, 0xC000_0061);
        assert_eq!(STATUS_NOT_FOUND as u32, 0xC000_0225);
        assert!(!nt_success(STATUS_BUFFER_OVERFLOW));
        assert!(!nt_success(STATUS_BUFFER_TOO_SMALL));
        assert!(!nt_success(STATUS_NO_MATCH));
        assert!(!nt_success(STATUS_NOT_SUPPORTED));
        assert!(!nt_success(STATUS_INVALID_DEVICE_REQUEST));
        assert!(!nt_success(STATUS_PRIVILEGE_NOT_HELD));
        assert!(!nt_success(STATUS_NOT_FOUND));
    }
}
