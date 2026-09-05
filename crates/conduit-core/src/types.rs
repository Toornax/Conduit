//! Types de base : fréquence, trames, canaux, gain.
//!
//! Tous ces newtypes sont `Copy`, sans allocation, utilisables depuis le fil audio.

use core::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Fréquence d'échantillonnage en hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SampleRate(u32);

impl SampleRate {
    /// 44,1 kHz.
    pub const HZ_44100: SampleRate = SampleRate(44_100);
    /// 48 kHz (défaut du graphe).
    pub const HZ_48000: SampleRate = SampleRate(48_000);
    /// 96 kHz.
    pub const HZ_96000: SampleRate = SampleRate(96_000);

    /// Plage acceptée.
    pub const MIN: u32 = 8_000;
    /// Plage acceptée.
    pub const MAX: u32 = 384_000;

    /// Construit une fréquence, `None` si hors de [`MIN`](Self::MIN)..=[`MAX`](Self::MAX).
    pub const fn new(hz: u32) -> Option<Self> {
        if hz >= Self::MIN && hz <= Self::MAX {
            Some(Self(hz))
        } else {
            None
        }
    }

    /// Valeur en hertz.
    pub const fn hz(self) -> u32 {
        self.0
    }

    /// Valeur en hertz, flottant.
    pub fn as_f64(self) -> f64 {
        f64::from(self.0)
    }

    /// Durée d'un nombre de trames à cette fréquence, en secondes.
    pub fn frames_to_seconds(self, frames: Frames) -> f64 {
        frames.get() as f64 / self.as_f64()
    }

    /// Nombre de trames correspondant à une durée en secondes (arrondi au plus proche).
    pub fn seconds_to_frames(self, seconds: f64) -> Frames {
        Frames::new((seconds * self.as_f64()).round().max(0.0) as usize)
    }
}

impl Default for SampleRate {
    fn default() -> Self {
        Self::HZ_48000
    }
}

impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 % 1000 == 0 {
            write!(f, "{} kHz", self.0 / 1000)
        } else {
            write!(f, "{} Hz", self.0)
        }
    }
}

/// Nombre de trames (échantillons par canal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Frames(usize);

impl Frames {
    /// Zéro trame.
    pub const ZERO: Frames = Frames(0);

    /// Construit.
    pub const fn new(n: usize) -> Self {
        Self(n)
    }

    /// Valeur brute.
    pub const fn get(self) -> usize {
        self.0
    }
}

impl From<usize> for Frames {
    fn from(n: usize) -> Self {
        Self(n)
    }
}

impl From<Frames> for usize {
    fn from(f: Frames) -> Self {
        f.0
    }
}

impl core::ops::Add for Frames {
    type Output = Frames;
    fn add(self, rhs: Frames) -> Frames {
        Frames(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Frames {
    type Output = Frames;
    fn sub(self, rhs: Frames) -> Frames {
        Frames(self.0 - rhs.0)
    }
}

impl core::ops::AddAssign for Frames {
    fn add_assign(&mut self, rhs: Frames) {
        self.0 += rhs.0;
    }
}

impl fmt::Display for Frames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} trames", self.0)
    }
}

/// Quantum : nombre de trames par cycle de traitement.
///
/// Puissance de deux dans [`MIN`](Self::MIN)..=[`MAX`](Self::MAX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(try_from = "usize", into = "usize")
)]
pub struct Quantum(usize);

impl Quantum {
    /// Plus petit quantum accepté.
    pub const MIN: usize = 32;
    /// Plus grand quantum accepté.
    pub const MAX: usize = 8192;
    /// Quantum par défaut.
    pub const DEFAULT: Quantum = Quantum(256);

    /// Construit un quantum, `None` si hors bornes ou pas une puissance de deux.
    pub const fn new(frames: usize) -> Option<Self> {
        if frames >= Self::MIN && frames <= Self::MAX && frames.is_power_of_two() {
            Some(Self(frames))
        } else {
            None
        }
    }

    /// Valeur brute.
    pub const fn get(self) -> usize {
        self.0
    }

    /// En `Frames`.
    pub const fn frames(self) -> Frames {
        Frames(self.0)
    }
}

impl Default for Quantum {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<usize> for Quantum {
    type Error = InvalidQuantum;
    fn try_from(v: usize) -> Result<Self, Self::Error> {
        Self::new(v).ok_or(InvalidQuantum(v))
    }
}

impl From<Quantum> for usize {
    fn from(q: Quantum) -> usize {
        q.0
    }
}

impl fmt::Display for Quantum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Erreur : quantum invalide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "quantum invalide {0} : attendu une puissance de deux entre {min} et {max}",
    min = Quantum::MIN,
    max = Quantum::MAX
)]
pub struct InvalidQuantum(pub usize);

/// Nombre de canaux d'un port ou d'un câble, entre 1 et [`MAX`](Self::MAX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(try_from = "u8", into = "u8")
)]
pub struct ChannelCount(u8);

impl ChannelCount {
    /// Nombre maximal de canaux (SPEC F-03 : 8 par câble).
    pub const MAX: u8 = 8;
    /// Mono.
    pub const MONO: ChannelCount = ChannelCount(1);
    /// Stéréo.
    pub const STEREO: ChannelCount = ChannelCount(2);

    /// Construit, `None` si 0 ou > [`MAX`](Self::MAX).
    pub const fn new(n: u8) -> Option<Self> {
        if n >= 1 && n <= Self::MAX {
            Some(Self(n))
        } else {
            None
        }
    }

    /// Valeur brute.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Valeur en `usize`.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl Default for ChannelCount {
    fn default() -> Self {
        Self::STEREO
    }
}

impl TryFrom<u8> for ChannelCount {
    type Error = InvalidChannelCount;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::new(v).ok_or(InvalidChannelCount(v))
    }
}

impl From<ChannelCount> for u8 {
    fn from(c: ChannelCount) -> u8 {
        c.0
    }
}

impl fmt::Display for ChannelCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1 => f.write_str("mono"),
            2 => f.write_str("stéréo"),
            n => write!(f, "{n} canaux"),
        }
    }
}

/// Erreur : nombre de canaux invalide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("nombre de canaux invalide {0} : attendu entre 1 et {max}", max = ChannelCount::MAX)]
pub struct InvalidChannelCount(pub u8);

/// Gain en décibels.
///
/// `Db::NEG_INF` (`-inf`) représente le silence. Les valeurs sont bornées à
/// [`MAX`](Self::MAX) lors de la construction.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Db(f32);

#[cfg(feature = "serde")]
impl Serialize for Db {
    /// Nombre, ou la chaîne `"-inf"` pour le silence (JSON n'a pas d'infini).
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_silent() {
            s.serialize_str("-inf")
        } else {
            s.serialize_f32(self.0)
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Db {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Num(f32),
            Str(String),
        }
        match Repr::deserialize(d)? {
            Repr::Num(v) => Ok(Db::new(v)),
            Repr::Str(s)
                if s.eq_ignore_ascii_case("-inf") || s.eq_ignore_ascii_case("-infinity") =>
            {
                Ok(Db::NEG_INF)
            }
            Repr::Str(s) => s.parse::<f32>().map(Db::new).map_err(|_| {
                serde::de::Error::custom(format!("gain invalide {s:?} : nombre en dB ou \"-inf\""))
            }),
        }
    }
}

impl Db {
    /// 0 dB : gain unité.
    pub const UNITY: Db = Db(0.0);
    /// −∞ dB : silence.
    pub const NEG_INF: Db = Db(f32::NEG_INFINITY);
    /// Gain maximal autorisé (+24 dB).
    pub const MAX: f32 = 24.0;
    /// En dessous de ce seuil, un gain est considéré comme le silence (−∞).
    pub const SILENCE_THRESHOLD: f32 = -120.0;

    /// Construit un gain en dB, borné à [`MAX`](Self::MAX). `NaN` devient −∞.
    pub fn new(db: f32) -> Self {
        if db.is_nan() || db <= Self::SILENCE_THRESHOLD {
            Self::NEG_INF
        } else {
            Self(db.min(Self::MAX))
        }
    }

    /// Valeur brute en dB.
    pub const fn get(self) -> f32 {
        self.0
    }

    /// Vrai si le gain est −∞ (silence).
    pub fn is_silent(self) -> bool {
        self.0 == f32::NEG_INFINITY
    }

    /// Conversion en gain linéaire.
    pub fn to_gain(self) -> Gain {
        if self.is_silent() {
            Gain::SILENCE
        } else {
            Gain(10f32.powf(self.0 / 20.0))
        }
    }
}

impl Default for Db {
    fn default() -> Self {
        Self::UNITY
    }
}

impl From<Gain> for Db {
    fn from(g: Gain) -> Self {
        g.to_db()
    }
}

impl fmt::Display for Db {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_silent() {
            f.write_str("-inf dB")
        } else {
            write!(f, "{:+.1} dB", self.0)
        }
    }
}

/// Gain linéaire (facteur multiplicatif), toujours ≥ 0 et fini.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Gain(f32);

impl Gain {
    /// Gain unité (1,0).
    pub const UNITY: Gain = Gain(1.0);
    /// Silence (0,0).
    pub const SILENCE: Gain = Gain(0.0);

    /// Construit un gain linéaire. Négatif, `NaN` ou infini → borné dans [0, max].
    pub fn new(linear: f32) -> Self {
        let max = Db(Db::MAX).to_gain().0;
        if linear.is_nan() || linear <= 0.0 {
            Self::SILENCE
        } else {
            Self(linear.min(max))
        }
    }

    /// Valeur brute.
    pub const fn get(self) -> f32 {
        self.0
    }

    /// Vrai si le gain est exactement 1,0.
    pub fn is_unity(self) -> bool {
        self.0 == 1.0
    }

    /// Vrai si le gain est nul.
    pub fn is_silent(self) -> bool {
        self.0 == 0.0
    }

    /// Conversion en dB.
    pub fn to_db(self) -> Db {
        if self.0 <= 0.0 {
            Db::NEG_INF
        } else {
            Db::new(20.0 * self.0.log10())
        }
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::UNITY
    }
}

impl From<Db> for Gain {
    fn from(db: Db) -> Self {
        db.to_gain()
    }
}

impl fmt::Display for Gain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "×{:.3}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_rate_bounds_and_display() {
        assert_eq!(SampleRate::new(48_000), Some(SampleRate::HZ_48000));
        assert_eq!(SampleRate::new(100), None);
        assert_eq!(SampleRate::new(1_000_000), None);
        assert_eq!(SampleRate::HZ_44100.to_string(), "44100 Hz");
        assert_eq!(SampleRate::HZ_48000.to_string(), "48 kHz");
        assert_eq!(SampleRate::default(), SampleRate::HZ_48000);
    }

    #[test]
    fn sample_rate_frame_conversions() {
        let sr = SampleRate::HZ_48000;
        assert_eq!(sr.seconds_to_frames(0.01), Frames::new(480));
        assert!((sr.frames_to_seconds(Frames::new(480)) - 0.01).abs() < 1e-12);
        assert_eq!(sr.seconds_to_frames(-1.0), Frames::ZERO);
    }

    #[test]
    fn quantum_validation() {
        assert!(Quantum::new(256).is_some());
        assert!(Quantum::new(32).is_some());
        assert!(Quantum::new(8192).is_some());
        assert!(Quantum::new(16).is_none());
        assert!(Quantum::new(16384).is_none());
        assert!(Quantum::new(300).is_none());
        assert_eq!(
            Quantum::try_from(300usize).unwrap_err(),
            InvalidQuantum(300)
        );
        assert_eq!(Quantum::default().get(), 256);
    }

    #[test]
    fn channel_count_validation() {
        assert_eq!(ChannelCount::new(0), None);
        assert_eq!(ChannelCount::new(9), None);
        assert_eq!(ChannelCount::new(8).map(ChannelCount::get), Some(8));
        assert_eq!(ChannelCount::MONO.to_string(), "mono");
        assert_eq!(ChannelCount::STEREO.to_string(), "stéréo");
        assert_eq!(ChannelCount::new(6).unwrap().to_string(), "6 canaux");
        assert_eq!(
            ChannelCount::try_from(0u8).unwrap_err(),
            InvalidChannelCount(0)
        );
    }

    #[test]
    fn db_zero_is_unity_gain() {
        assert_eq!(Db::UNITY.to_gain(), Gain::UNITY);
        assert_eq!(Gain::UNITY.to_db(), Db::UNITY);
        assert_eq!(Db::UNITY.to_string(), "+0.0 dB");
    }

    #[test]
    fn db_neg_inf_is_silence() {
        assert_eq!(Db::NEG_INF.to_gain(), Gain::SILENCE);
        assert!(Db::NEG_INF.is_silent());
        assert_eq!(Gain::SILENCE.to_db(), Db::NEG_INF);
        assert_eq!(Db::NEG_INF.to_string(), "-inf dB");
        assert!(Db::new(f32::NAN).is_silent());
        assert!(Db::new(-200.0).is_silent());
        assert!(Db::new(-120.0).is_silent());
        assert!(!Db::new(-119.9).is_silent());
    }

    #[test]
    fn db_gain_roundtrip_and_known_values() {
        let cases = [
            (-6.0206, 0.5),
            (6.0206, 2.0),
            (-20.0, 0.1),
            (20.0, 10.0),
            (-40.0, 0.01),
        ];
        for (db, lin) in cases {
            let g = Db::new(db).to_gain().get();
            assert!((g - lin).abs() < 1e-4, "{db} dB → {g}, attendu {lin}");
            let back = Gain::new(lin).to_db().get();
            assert!((back - db).abs() < 1e-3, "{lin} → {back} dB, attendu {db}");
        }
    }

    #[test]
    fn db_is_clamped_to_max() {
        assert_eq!(Db::new(100.0).get(), Db::MAX);
        let g = Gain::new(1e9);
        assert!((g.to_db().get() - Db::MAX).abs() < 1e-4);
    }

    #[test]
    fn gain_rejects_negative_and_nan() {
        assert!(Gain::new(-1.0).is_silent());
        assert!(Gain::new(f32::NAN).is_silent());
        assert!(Gain::new(f32::INFINITY).get().is_finite());
    }

    #[test]
    fn frames_arithmetic() {
        let a = Frames::new(100);
        let b = Frames::new(28);
        assert_eq!(a + b, Frames::new(128));
        assert_eq!(a - b, Frames::new(72));
        let mut c = a;
        c += b;
        assert_eq!(c, Frames::new(128));
        assert_eq!(usize::from(c), 128);
        assert_eq!(c.to_string(), "128 trames");
    }

    #[cfg(feature = "serde")]
    fn rmp_serde_roundtrip(db: Db) -> Db {
        // MessagePack via serde_json Value comme pivot neutre (pas de dépendance rmp ici).
        let v = serde_json::to_value(db).unwrap();
        serde_json::from_value(v).unwrap()
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip_and_validation() {
        let q: Quantum = serde_json::from_str("512").unwrap();
        assert_eq!(q.get(), 512);
        assert!(serde_json::from_str::<Quantum>("500").is_err());
        let c: ChannelCount = serde_json::from_str("2").unwrap();
        assert_eq!(c, ChannelCount::STEREO);
        assert!(serde_json::from_str::<ChannelCount>("0").is_err());
        assert_eq!(
            serde_json::to_string(&SampleRate::HZ_48000).unwrap(),
            "48000"
        );
        assert_eq!(serde_json::to_string(&Db::new(-6.0)).unwrap(), "-6.0");
        assert_eq!(serde_json::to_string(&Db::NEG_INF).unwrap(), "\"-inf\"");
        assert_eq!(serde_json::from_str::<Db>("\"-inf\"").unwrap(), Db::NEG_INF);
        assert_eq!(
            serde_json::from_str::<Db>("\"-3.5\"").unwrap(),
            Db::new(-3.5)
        );
        assert_eq!(serde_json::from_str::<Db>("-200").unwrap(), Db::NEG_INF);
        assert!(serde_json::from_str::<Db>("\"fort\"").is_err());
        let bin = rmp_serde_roundtrip(Db::NEG_INF);
        assert_eq!(bin, Db::NEG_INF);
    }
}

#[cfg(feature = "schema")]
mod schema_impls {
    use super::{ChannelCount, Db, Quantum};
    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use std::borrow::Cow;

    impl JsonSchema for Quantum {
        fn schema_name() -> Cow<'static, str> {
            "Quantum".into()
        }
        fn json_schema(_: &mut SchemaGenerator) -> Schema {
            json_schema!({
                "type": "integer",
                "minimum": Quantum::MIN,
                "maximum": Quantum::MAX,
                "description": "Trames par cycle, puissance de deux"
            })
        }
    }

    impl JsonSchema for ChannelCount {
        fn schema_name() -> Cow<'static, str> {
            "ChannelCount".into()
        }
        fn json_schema(_: &mut SchemaGenerator) -> Schema {
            json_schema!({ "type": "integer", "minimum": 1, "maximum": ChannelCount::MAX })
        }
    }

    impl JsonSchema for Db {
        fn schema_name() -> Cow<'static, str> {
            "Db".into()
        }
        fn json_schema(_: &mut SchemaGenerator) -> Schema {
            json_schema!({
                "oneOf": [
                    { "type": "number", "maximum": Db::MAX, "description": "Gain en dB" },
                    { "type": "string", "enum": ["-inf"], "description": "Silence" }
                ]
            })
        }
    }
}
