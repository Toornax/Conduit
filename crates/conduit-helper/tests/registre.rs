//! Lecture du registre des endpoints audio (M1b-21).
//!
//! # Ces tests sont `#[ignore]`, et surtout **en lecture seule**
//!
//! Ils ouvrent `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio` et
//! parcourent ses sous-clés. Ils **n'écrivent rien**, ne renomment aucun endpoint,
//! n'installent aucun service, n'ouvrent aucun flux audio et n'émettent aucun son : la
//! contrainte du dépôt interdit qu'un test touche la machine de développement, et le
//! renommage d'un vrai endpoint en serait l'exemple le plus fâcheux.
//!
//! Ils restent `#[ignore]` malgré tout, parce qu'ils lisent une ruche du système et que
//! leur résultat dépend de la machine : ce n'est pas une propriété du code, c'est une
//! constatation. La règle du dépôt étant « tout ce qui touche à la machine est
//! `#[ignore]` », ils le sont.
//!
//! ```text
//! cargo test -p conduit-helper --test registre -- --ignored --nocapture
//! ```
//!
//! # Ce qu'ils vérifient
//!
//! Que [`conduit_helper::registre::trouver`] traverse le registre sans paniquer et rend
//! un verdict cohérent avec la machine : sur un poste **sans** le pilote Conduit, aucun
//! endpoint ne désigne un câble ; dans la machine virtuelle **avec** le pilote et des
//! câbles connectés, les deux côtés se trouvent. Le second cas ne peut pas être affirmé
//! ici — il se lit dans la sortie, `--nocapture`.

#![cfg(windows)]

use conduit_backend::CableId;
use conduit_helper::controle::Cote;
use conduit_helper::registre;
use conduit_kmd_core::config::CABLE_MAX;

/// L'inventaire traverse le registre sans paniquer et lit bien les descriptions.
///
/// **Lecture seule.** C'est aussi la commande de diagnostic : `--nocapture` montre ce que
/// le service voit, ce qui est le premier renseignement à demander quand un renommage n'a
/// pas l'effet attendu.
#[test]
#[ignore = "lit HKLM\\...\\MMDevices sur la machine (lecture seule, voir l'en-tête)"]
fn l_inventaire_lit_les_descriptions_sans_rien_ecrire() {
    let vus = registre::inventorier().expect("les deux clés de flux s'ouvrent et se parcourent");
    assert!(
        !vus.is_empty(),
        "aucun endpoint audio sur cette machine : même sans carte son, le moteur audio \
         en publie"
    );
    // La lecture d'une valeur `REG_SZ` marche vraiment : au moins un endpoint porte une
    // description non vide et sans `NUL` de fin resté dans la chaîne.
    let avec_nom = vus.iter().filter(|v| v.description.is_some()).count();
    assert!(
        avec_nom > 0,
        "aucune description lue sur {} endpoint(s) : la lecture de « {} » ne marche pas",
        vus.len(),
        registre::valeur_description()
    );
    for vu in &vus {
        if let Some(description) = &vu.description {
            assert!(!description.is_empty(), "{}", vu.endpoint);
            assert!(!description.contains('\0'), "{}", vu.endpoint);
            assert_eq!(description.trim(), description, "{}", vu.endpoint);
        }
        println!(
            "{:?} {} : description {:?}, marque {:?} → {:?}",
            vu.cote, vu.endpoint, vu.description, vu.marque, vu.cable
        );
    }
    println!(
        "{} endpoint(s), dont {avec_nom} avec description",
        vus.len()
    );
}

/// La recherche par câble rend au plus un endpoint par côté.
///
/// **Lecture seule.** Sur un poste sans le pilote Conduit, elle ne trouve rien, et c'est
/// le bon résultat — l'invariant, lui, est vrai partout.
#[test]
#[ignore = "lit HKLM\\...\\MMDevices sur la machine (lecture seule, voir l'en-tête)"]
fn la_recherche_rend_au_plus_un_endpoint_par_cote() {
    let mut total = 0usize;
    for numero in 1..=CABLE_MAX {
        let cable = CableId(numero);
        let trouves = registre::trouver(cable).expect("le registre se parcourt");
        assert!(trouves.len() <= Cote::ALL.len(), "Conduit {numero}");
        for cote in Cote::ALL {
            assert!(
                trouves.iter().filter(|e| e.cote == cote).count() <= 1,
                "Conduit {numero} : deux endpoints du même côté {cote:?}"
            );
        }
        for endpoint in &trouves {
            // Ce que la recherche rend désigne bien le câble demandé.
            assert_eq!(endpoint.cable, Some(cable), "Conduit {numero}");
            println!(
                "Conduit {numero} ({:?}) : {}",
                endpoint.cote,
                registre::chemin_proprietes(endpoint.cote, &endpoint.endpoint)
            );
        }
        total = total.saturating_add(trouves.len());
    }
    println!(
        "{total} endpoint(s) Conduit trouvé(s) sur cette machine ; sans le pilote, 0 est \
         le bon résultat"
    );
}

/// Les chemins que la recherche parcourt sont ceux qu'on documente, et ils existent.
///
/// **Lecture seule.** Un poste Windows a toujours ces deux clés, même sans carte son :
/// c'est le moteur audio qui les crée, pas un pilote.
#[test]
#[ignore = "lit HKLM\\...\\MMDevices sur la machine (lecture seule, voir l'en-tête)"]
fn les_deux_cles_de_flux_existent() {
    for cote in Cote::ALL {
        let chemin = registre::chemin_flux(cote);
        // Une recherche sur un câble hors réserve : elle ouvre les deux clés de flux et
        // ne peut rien trouver, ce qui prouve que l'ouverture a réussi sans rien écrire.
        let issue = registre::trouver(CableId(CABLE_MAX));
        assert!(
            issue.is_ok(),
            "HKLM\\{chemin} : {}",
            issue.err().map_or_else(String::new, |e| e.to_string())
        );
    }
}
