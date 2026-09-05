//! Codes `NTSTATUS` du contrat PortCls absents de `conduit-com` (`ntstatus.h` n'est pas
//! dans les bindings de `portcls-sys` ; `wdk-sys` n'est pas une dépendance ici).

use conduit_com::NtStatus;

/// `STATUS_BUFFER_OVERFLOW` (`0x80000005`, avertissement : `NT_SUCCESS` faux) : réponse
/// de `DataRangeIntersection` à une interrogation de taille (tampon de sortie absent ou
/// de longueur nulle), la taille requise étant écrite dans `ResultantFormatLength`.
pub const STATUS_BUFFER_OVERFLOW: NtStatus = 0x8000_0005_u32 as i32;

/// `STATUS_BUFFER_TOO_SMALL` (`0xC0000023`) : tampon de sortie fourni mais trop petit.
pub const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xC000_0023_u32 as i32;

/// `STATUS_NOT_SUPPORTED` (`0xC00000BB`) : réponse par défaut de
/// `IMiniportWaveRTStream::SetFormat` (changement de format en cours de flux refusé,
/// comme SYSVAD).
pub const STATUS_NOT_SUPPORTED: NtStatus = 0xC000_00BB_u32 as i32;

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_com::nt_success;

    #[test]
    fn valeurs_du_wdk() {
        assert_eq!(STATUS_BUFFER_OVERFLOW as u32, 0x8000_0005);
        assert_eq!(STATUS_BUFFER_TOO_SMALL as u32, 0xC000_0023);
        assert_eq!(STATUS_NOT_SUPPORTED as u32, 0xC000_00BB);
        assert!(!nt_success(STATUS_BUFFER_OVERFLOW));
        assert!(!nt_success(STATUS_BUFFER_TOO_SMALL));
        assert!(!nt_success(STATUS_NOT_SUPPORTED));
    }
}
