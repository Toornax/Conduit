//! Mise en forme française des grandeurs affichées.
//!
//! Trois conventions, tenues partout :
//!
//! - **virgule décimale** (`5,3` et non `5.3`) ;
//! - **espace insécable** avant l'unité ([`INSECABLE`]) et **espace insécable
//!   fine** entre les groupes de milliers ([`FINE`]), pour qu'un nombre ne se
//!   coupe jamais en fin de ligne ;
//! - **signe moins typographique** ([`MOINS`], U+2212) et non le trait d'union.
//!
//! Les unités elles-mêmes viennent d'[`crate::i18n`] : aucune chaîne affichée
//! n'est écrite ici.
//!
//! Les chiffres n'ont pas de variante tabulaire (`tnum` est inaccessible depuis
//! `iced`) : l'alignement d'une colonne de nombres s'obtient en fixant la
//! largeur de la cellule, `text(millisecondes(x)).width(72).align_x(Right)`.

use conduit_core::types::Db;

use crate::i18n::{self, Text};

/// Espace insécable (U+00A0), entre un nombre et son unité.
pub const INSECABLE: char = '\u{00a0}';
/// Espace insécable fine (U+202F), entre les groupes de milliers.
pub const FINE: char = '\u{202f}';
/// Signe moins typographique (U+2212).
pub const MOINS: char = '\u{2212}';

/// Groupe les chiffres par trois depuis la droite, séparés par [`FINE`].
fn grouper(chiffres: &str) -> String {
    let mut groupe = String::with_capacity(chiffres.len() + chiffres.len() / 3);
    let reste = chiffres.len() % 3;
    for (i, c) in chiffres.chars().enumerate() {
        if i > 0 && i % 3 == reste {
            groupe.push(FINE);
        }
        groupe.push(c);
    }
    groupe
}

/// Un nombre à la française : virgule décimale, milliers groupés, signe moins
/// typographique.
///
/// Un zéro arrondi ne porte jamais de signe (`-0,04` à une décimale donne
/// `0,0` et non `−0,0`).
pub fn nombre(valeur: f64, decimales: usize) -> String {
    if !valeur.is_finite() {
        return i18n::t(Text::Silence).to_string();
    }
    let rendu = format!("{:.*}", decimales, valeur.abs());
    let (entiere, fraction) = match rendu.split_once('.') {
        Some((e, f)) => (e, Some(f)),
        None => (rendu.as_str(), None),
    };
    let nul =
        entiere.chars().all(|c| c == '0') && fraction.is_none_or(|f| f.chars().all(|c| c == '0'));
    let mut sortie = String::new();
    if valeur.is_sign_negative() && !nul {
        sortie.push(MOINS);
    }
    sortie.push_str(&grouper(entiere));
    if let Some(fraction) = fraction {
        sortie.push(',');
        sortie.push_str(fraction);
    }
    sortie
}

/// Un entier à la française : milliers groupés par [`FINE`].
pub fn entier(valeur: u64) -> String {
    grouper(&valeur.to_string())
}

/// Une valeur et son unité, séparées d'une espace insécable.
fn avec_unite(valeur: String, unite: Text) -> String {
    format!("{valeur}{INSECABLE}{}", i18n::t(unite))
}

/// Une fréquence en hertz, à l'entier : « 440 Hz ».
pub fn hertz(hz: f32) -> String {
    avec_unite(nombre(f64::from(hz), 0), Text::UnitHz)
}

/// Une durée en millisecondes, à la dixième : « 5,3 ms ».
pub fn millisecondes(ms: f64) -> String {
    avec_unite(nombre(ms, 1), Text::UnitMs)
}

/// Un gain en décibels, à la dixième, signé : « +3,0 dB », « −12,0 dB »,
/// « −∞ dB » pour le silence.
pub fn decibels(db: Db) -> String {
    if db.is_silent() {
        return avec_unite(i18n::t(Text::Silence).to_string(), Text::UnitDb);
    }
    let valeur = f64::from(db.get());
    let rendu = nombre(valeur, 1);
    let signe = if valeur > 0.0 { "+" } else { "" };
    avec_unite(format!("{signe}{rendu}"), Text::UnitDb)
}

/// Une fraction en pourcentage entier : `0.425` donne « 43 % ».
pub fn pourcentage(fraction: f32) -> String {
    avec_unite(nombre(f64::from(fraction) * 100.0, 0), Text::UnitPercent)
}

/// Un rapport, à la centième : « 1,50 × ».
pub fn ratio(valeur: f64) -> String {
    avec_unite(nombre(valeur, 2), Text::UnitRatio)
}

/// Le ratio d'un rééchantillonneur, à la millionième : « 1,000018 ».
///
/// Ce ratio-là vaut toujours à peu de chose près 1 : les deux décimales de
/// [`ratio`] l'écriraient « 1,00 » et n'en diraient rien — c'est la sixième
/// qui porte la dérive d'horloge. `conduitctl status` l'écrit déjà ainsi
/// (`{:.6}`) ; la GUI ne le contredit pas.
///
/// Sans unité : le « × » de [`ratio`] annonce un facteur d'échelle, pas une
/// horloge qui glisse.
pub fn ratio_reechantillonnage(valeur: f64) -> String {
    nombre(valeur, 6)
}

/// Une durée en microsecondes, à l'entier : « 5 333 µs ».
///
/// C'est l'unité dans laquelle `conduitctl` écrit les temps de cycle, et donc
/// celle dans laquelle la vue Diagnostic les détaille.
pub fn microsecondes(us: u64) -> String {
    avec_unite(entier(us), Text::UnitUs)
}

/// Un remplissage de tampon, accordé : « 1 trame », « 960 trames ».
///
/// [`DeviceStatus::fill`] est un **nombre de trames**, et le protocole
/// n'annonce aucune capacité : il n'y a pas de pourcentage à en tirer.
///
/// [`DeviceStatus::fill`]: conduit_protocol::DeviceStatus::fill
pub fn trames(compte: u32) -> String {
    let unite = if compte <= 1 {
        Text::UnitFrame
    } else {
        Text::UnitFrames
    };
    format!("{}{INSECABLE}{}", entier(u64::from(compte)), i18n::t(unite))
}

/// Un décompte de xruns : « aucun xrun », « 1 xrun », « 1 234 xruns ».
pub fn xruns(compte: u64) -> String {
    match compte {
        0 => i18n::t(Text::NoXrun).to_string(),
        1 => format!("1{INSECABLE}{}", i18n::t(Text::Xrun)),
        n => format!("{}{INSECABLE}{}", entier(n), i18n::t(Text::Xruns)),
    }
}

/// Une fréquence d'échantillonnage en kilohertz : « 48 kHz », « 44,1 kHz ».
///
/// La décimale n'apparaît que si elle porte de l'information.
pub fn kilohertz(hz: u32) -> String {
    let decimales = usize::from(hz % 1_000 != 0);
    avec_unite(nombre(f64::from(hz) / 1_000.0, decimales), Text::UnitKhz)
}

/// Une durée de marche, à l'unité qui se lit : « 45 s », « 12 min »,
/// « 3 h 05 ».
///
/// Au-delà de l'heure, les minutes sont écrites sur deux chiffres, comme une
/// heure de la journée.
pub fn duree_de_marche(secondes: u64) -> String {
    match secondes {
        s if s < 60 => avec_unite(entier(s), Text::UnitS),
        s if s < 3_600 => avec_unite(entier(s / 60), Text::UnitMin),
        s => format!(
            "{}{INSECABLE}{}{INSECABLE}{:02}",
            entier(s / 3_600),
            i18n::t(Text::UnitH),
            (s % 3_600) / 60
        ),
    }
}

/// Latence estimée d'un nœud, en millisecondes : deux quanta, soit
/// `2 × quantum / fréquence`.
///
/// Une fréquence nulle — un nœud dont le format n'est pas encore négocié —
/// donne 0.
pub fn latence_estimee_ms(quantum: u32, frequence: u32) -> f64 {
    if frequence == 0 {
        return 0.0;
    }
    2.0 * f64::from(quantum) / f64::from(frequence) * 1000.0
}

/// La latence estimée, mise en forme : « 5,3 ms ».
pub fn latence_estimee(quantum: u32, frequence: u32) -> String {
    millisecondes(latence_estimee_ms(quantum, frequence))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Virgule décimale, milliers groupés, signe moins typographique.
    #[test]
    fn les_nombres_sont_ecrits_a_la_francaise() {
        assert_eq!(nombre(5.34, 1), "5,3");
        assert_eq!(nombre(5.36, 1), "5,4");
        assert_eq!(nombre(1234.5, 1), "1\u{202f}234,5");
        assert_eq!(nombre(1234567.0, 0), "1\u{202f}234\u{202f}567");
        assert_eq!(nombre(-12.0, 1), "\u{2212}12,0");
        // Un zéro arrondi ne porte pas de signe.
        assert_eq!(nombre(-0.04, 1), "0,0");
        assert_eq!(entier(1_000), "1\u{202f}000");
        assert_eq!(entier(999), "999");
    }

    /// L'unité est collée au nombre par une espace insécable.
    #[test]
    fn l_unite_est_liee_au_nombre() {
        assert_eq!(millisecondes(5.333), "5,3\u{a0}ms");
        assert_eq!(pourcentage(0.425), "43\u{a0}%");
        assert_eq!(pourcentage(1.0), "100\u{a0}%");
        assert_eq!(ratio(1.5), "1,50\u{a0}×");
    }

    /// Le ratio d'un rééchantillonneur se lit à la sixième décimale, sans
    /// unité : c'est là que se voit la dérive d'horloge.
    #[test]
    fn le_ratio_de_reechantillonnage_va_a_la_millionieme() {
        assert_eq!(ratio_reechantillonnage(1.0), "1,000000");
        assert_eq!(ratio_reechantillonnage(0.999_821), "0,999821");
        assert_eq!(ratio_reechantillonnage(1.000_018), "1,000018");
        // Les deux décimales de `ratio` n'en diraient rien : c'est bien deux
        // fonctions qu'il faut.
        assert_eq!(ratio(1.000_018), "1,00\u{a0}×");
        assert!(!ratio_reechantillonnage(1.0).contains('×'));
    }

    /// Les microsecondes s'écrivent à l'entier, milliers groupés.
    #[test]
    fn les_microsecondes_s_ecrivent_a_l_entier() {
        assert_eq!(microsecondes(511), "511\u{a0}µs");
        assert_eq!(microsecondes(5_333), "5\u{202f}333\u{a0}µs");
        assert_eq!(microsecondes(0), "0\u{a0}µs");
    }

    /// Le remplissage s'écrit en trames, accordé, jamais en pourcentage.
    #[test]
    fn le_remplissage_s_ecrit_en_trames() {
        assert_eq!(trames(0), "0\u{a0}trame");
        assert_eq!(trames(1), "1\u{a0}trame");
        assert_eq!(trames(960), "960\u{a0}trames");
        assert_eq!(trames(1_920), "1\u{202f}920\u{a0}trames");
        assert!(!trames(960).contains('%'));
    }

    /// Les gains sont signés ; le silence s'écrit « −∞ dB ».
    #[test]
    fn les_gains_sont_signes_et_le_silence_est_infini() {
        assert_eq!(decibels(Db::UNITY), "0,0\u{a0}dB");
        assert_eq!(decibels(Db::new(3.0)), "+3,0\u{a0}dB");
        assert_eq!(decibels(Db::new(-12.0)), "\u{2212}12,0\u{a0}dB");
        assert_eq!(decibels(Db::NEG_INF), "\u{2212}∞\u{a0}dB");
    }

    /// Le décompte de xruns s'accorde en nombre.
    #[test]
    fn les_xruns_s_accordent_en_nombre() {
        assert_eq!(xruns(0), "aucun xrun");
        assert_eq!(xruns(1), "1\u{a0}xrun");
        assert_eq!(xruns(1234), "1\u{202f}234\u{a0}xruns");
    }

    /// Les kilohertz ne portent une décimale que si elle dit quelque chose.
    #[test]
    fn les_frequences_sont_ecrites_en_kilohertz() {
        assert_eq!(kilohertz(48_000), "48\u{a0}kHz");
        assert_eq!(kilohertz(96_000), "96\u{a0}kHz");
        assert_eq!(kilohertz(44_100), "44,1\u{a0}kHz");
        // Un hertz s'écrit à l'entier, insécable devant son unité.
        assert_eq!(hertz(440.0), "440\u{a0}Hz");
        assert_eq!(hertz(1000.0), "1\u{202f}000\u{a0}Hz");
    }

    /// La durée de marche change d'unité à la minute puis à l'heure.
    #[test]
    fn la_duree_de_marche_change_d_unite() {
        assert_eq!(duree_de_marche(0), "0\u{a0}s");
        assert_eq!(duree_de_marche(45), "45\u{a0}s");
        assert_eq!(duree_de_marche(59), "59\u{a0}s");
        assert_eq!(duree_de_marche(60), "1\u{a0}min");
        assert_eq!(duree_de_marche(750), "12\u{a0}min");
        assert_eq!(duree_de_marche(3_600), "1\u{a0}h\u{a0}00");
        assert_eq!(duree_de_marche(11_100), "3\u{a0}h\u{a0}05");
    }

    /// Deux quanta, et rien d'autre.
    #[test]
    fn la_latence_estimee_vaut_deux_quanta() {
        assert!((latence_estimee_ms(128, 48_000) - 5.3333).abs() < 1e-3);
        assert!((latence_estimee_ms(256, 48_000) - 10.6666).abs() < 1e-3);
        assert!((latence_estimee_ms(48, 48_000) - 2.0).abs() < 1e-9);
        // Format non négocié : pas de division par zéro.
        assert_eq!(latence_estimee_ms(128, 0), 0.0);
        assert_eq!(latence_estimee(128, 48_000), "5,3\u{a0}ms");
        assert_eq!(latence_estimee(128, 0), "0,0\u{a0}ms");
    }
}
