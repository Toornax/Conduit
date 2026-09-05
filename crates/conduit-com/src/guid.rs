//! Identifiants d'interface (`GUID`, `IID`) avec la disposition mémoire du WDK.

use core::fmt;

/// Identifiant globalement unique, disposition `GUID` de `guiddef.h`.
///
/// Même disposition que `GUID` généré par bindgen dans `portcls-sys`
/// (`Data1: c_ulong, Data2: c_ushort, Data3: c_ushort, Data4: [c_uchar; 8]`) et que
/// `windows::core::GUID` : 16 octets, alignement 4, `repr(C)`. Un `*const Guid` peut
/// donc être lu là où PortCls passe un `REFIID`, et réciproquement, sans conversion.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Guid {
    /// Premier groupe (32 bits) de la forme textuelle.
    pub data1: u32,
    /// Deuxième groupe (16 bits).
    pub data2: u16,
    /// Troisième groupe (16 bits).
    pub data3: u16,
    /// Quatrième et cinquième groupes, dans l'ordre des octets de la forme textuelle.
    pub data4: [u8; 8],
}

impl Guid {
    /// Construit un GUID à partir de ses champs, comme la macro `DEFINE_GUID` du WDK
    /// (`data4` reçoit les huit derniers arguments dans l'ordre).
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    /// Construit un GUID à partir de ses 128 bits, lus dans l'ordre de la forme
    /// textuelle : `Guid::from_u128(0x00000000_0000_0000_0000_C00000000046)` est
    /// `00000000-0000-0000-0000-C00000000046`.
    pub const fn from_u128(value: u128) -> Self {
        let b = value.to_be_bytes();
        Self {
            data1: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_be_bytes([b[4], b[5]]),
            data3: u16::from_be_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }
}

impl fmt::Display for Guid {
    /// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`, hexadécimal majuscule, sans accolades.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [d0, d1, d2, d3, d4, d5, d6, d7] = self.data4;
        write!(
            f,
            "{:08X}-{:04X}-{:04X}-{d0:02X}{d1:02X}-{d2:02X}{d3:02X}{d4:02X}{d5:02X}{d6:02X}{d7:02X}",
            self.data1, self.data2, self.data3
        )
    }
}

/// `IID_IUnknown` **tel que le définit `punknown.h` du WDK** :
/// `00000000-0000-0000-0000-C00000000046`.
///
/// Attention : ce n'est pas la valeur OLE de `unknwn.h` en mode utilisateur
/// (`00000000-0000-0000-C000-000000000046`, octet `C0` en tête de `Data4`). PortCls est
/// compilé contre `punknown.h` et interroge les miniports avec **cette** valeur
/// (`DEFINE_GUID(IID_IUnknown, 0, 0, 0, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x46)`,
/// WDK 10.0.26100 ; vérifié par `portcls-sys/tests/layout.golden`). Un objet qui ne
/// répondrait qu'à la valeur OLE échouerait tous les `QueryInterface(IID_IUnknown)` du
/// noyau.
pub const IID_IUNKNOWN: Guid = Guid::new(0, 0, 0, [0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x46]);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};
    use std::string::ToString;

    #[test]
    fn disposition_du_wdk() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
        assert_eq!(core::mem::offset_of!(Guid, data1), 0);
        assert_eq!(core::mem::offset_of!(Guid, data2), 4);
        assert_eq!(core::mem::offset_of!(Guid, data3), 6);
        assert_eq!(core::mem::offset_of!(Guid, data4), 8);
    }

    #[test]
    fn affichage_majuscules_sans_accolades() {
        // IID_IMiniportTopology (portcls.h) : B4C90A31-5791-11D0-86F9-00A0C911B544.
        let g = Guid::new(
            0xB4C9_0A31,
            0x5791,
            0x11D0,
            [0x86, 0xF9, 0x00, 0xA0, 0xC9, 0x11, 0xB5, 0x44],
        );
        assert_eq!(g.to_string(), "B4C90A31-5791-11D0-86F9-00A0C911B544");
        assert_eq!(
            IID_IUNKNOWN.to_string(),
            "00000000-0000-0000-0000-C00000000046"
        );
        assert_eq!(
            Guid::new(0, 0, 0, [0; 8]).to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn from_u128_coherent_avec_new() {
        let attendu = Guid::new(
            0xB4C9_0A31,
            0x5791,
            0x11D0,
            [0x86, 0xF9, 0x00, 0xA0, 0xC9, 0x11, 0xB5, 0x44],
        );
        assert_eq!(
            Guid::from_u128(0xB4C90A31_5791_11D0_86F9_00A0C911B544),
            attendu
        );
        assert_eq!(
            Guid::from_u128(0x00000000_0000_0000_0000_C00000000046),
            IID_IUNKNOWN
        );
        assert_eq!(
            Guid::from_u128(u128::MAX).to_string(),
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF"
        );
    }

    #[test]
    fn egalite_et_copie() {
        let a = Guid::from_u128(1);
        let b = a;
        assert_eq!(a, b);
        assert_ne!(a, Guid::from_u128(2));
    }
}
