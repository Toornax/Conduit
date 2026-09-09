//! Les crates `fuzz/` construisent-ils encore ? (M1b-21)
//!
//! # Le piège que ce fichier ferme
//!
//! Les trois crates `crates/*/fuzz` sont **hors** du workspace racine : chacun porte un
//! `[workspace]` vide dans son manifeste, parce qu'un crate logé sous un membre n'est pas
//! affranchi par le seul `exclude` du parent et que `cargo fuzz` échouerait alors avant de
//! compiler quoi que ce soit (voir le commentaire du `Cargo.toml` racine). C'est
//! nécessaire, et c'est aussi un **angle mort total** : `cargo test --workspace`, `cargo
//! clippy --workspace`, `nix flake check` et `drivers\windows\tools\check.ps1` sont tous
//! verts pendant qu'une cible de fuzz ne compile plus.
//!
//! Le dépôt s'y est fait prendre **deux fois**. La campagne du 2026-09-09 a trouvé que
//! `crates/conduit-protocol/fuzz` ne construisait plus depuis un moment ; M1b-21 a trouvé
//! que `crates/conduit-kmd-core/fuzz` ne construisait plus non plus, sa cible `formats`
//! référençant un `M1A_FORMATS` que M1b-05 avait supprimé. Un harnais de fuzzing cassé
//! qu'on croit fonctionnel est pire que pas de harnais : il fait croire qu'un parseur est
//! éprouvé alors que plus personne ne l'exécute.
//!
//! # Pourquoi cette parade-ci
//!
//! Elle **attrape le cas** parce qu'elle demande à cargo de compiler exactement ce qui
//! avait cessé de compiler : `cargo check` sur le manifeste de chaque crate `fuzz/`, donc
//! sur chacune de ses cibles, avec les mêmes dépendances et le même crate cible que
//! `cargo fuzz`. Et elle **tourne** : c'est un test du workspace, il part avec
//! `cargo test --workspace`, c'est-à-dire à chaque `cargo test` de développement et à
//! chaque poussée (le job `cargo` de `.github/workflows/ci.yml`), sans réintégrer les
//! crates `fuzz/` au workspace — ce qui casserait `--workspace` et le check `cross-windows`.
//!
//! **Le nightly n'est pas nécessaire**, contrairement à ce que « `libfuzzer-sys` exige le
//! nightly » laisse croire : cette exigence porte sur l'**édition de liens** du binaire
//! instrumenté, pas sur l'analyse. `cargo check` de la chaîne stable du dépôt (1.96.1)
//! traverse bien `fuzz_target!` et résout bien les chemins — c'est ce qui a rendu l'erreur
//! `E0432: unresolved import conduit_kmd_core::format::M1A_FORMATS` visible en dix-sept
//! secondes, sans cargo-fuzz ni ASan.
//!
//! # Ce qui est vérifié, et où
//!
//! - [`chaque_cible_declaree_existe`] ne lance rien : elle relit les manifestes. Elle
//!   tourne donc **partout**, bac à sable Nix compris.
//! - [`chaque_crate_fuzz_construit`] lance `cargo check`, qui a besoin du réseau la
//!   première fois (les crates `fuzz/` n'ont pas de `Cargo.lock` versionné). Elle se
//!   retire dans un bac à sable Nix, qui n'en a pas — même raison qui fait de `cargo-deny`
//!   un check crane et non un hook (`nix/hooks.nix`). Le job `cargo` de la CI et les
//!   postes de développement, eux, la lancent.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::{env, fs};

/// La racine du dépôt, déduite du manifeste de ce crate (`<racine>/crates/conduit-testing`).
fn racine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("le manifeste de conduit-testing n'est pas à deux niveaux de la racine")
        .to_path_buf()
}

/// Tous les répertoires `crates/*/fuzz` qui portent un manifeste, découverts et non
/// énumérés : une quatrième cible ajoutée demain est couverte sans toucher à ce fichier.
fn crates_fuzz() -> Vec<PathBuf> {
    let crates = racine().join("crates");
    let mut trouves: Vec<PathBuf> = fs::read_dir(&crates)
        .unwrap_or_else(|e| panic!("lecture de {} : {e}", crates.display()))
        .map(|entree| {
            entree
                .unwrap_or_else(|e| panic!("entrée illisible dans {} : {e}", crates.display()))
                .path()
                .join("fuzz")
        })
        .filter(|fuzz| fuzz.join("Cargo.toml").is_file())
        .collect();
    trouves.sort();
    // Sans cette assertion, un renommage de `crates/` rendrait le test vert et vide, ce
    // qui est exactement la panne muette qu'il existe pour empêcher.
    assert!(
        !trouves.is_empty(),
        "aucun crate fuzz trouvé sous {} : la découverte est cassée, pas le dépôt",
        crates.display()
    );
    trouves
}

/// Chaque `[[bin]]` déclaré pointe sur un fichier qui existe, et chaque crate `fuzz/` garde
/// le `[workspace]` vide qui l'affranchit du workspace racine.
///
/// Ne lance aucune commande : ce test tourne partout, y compris sans réseau.
#[test]
fn chaque_cible_declaree_existe() {
    for crate_fuzz in crates_fuzz() {
        let manifeste = crate_fuzz.join("Cargo.toml");
        let texte = fs::read_to_string(&manifeste)
            .unwrap_or_else(|e| panic!("lecture de {} : {e}", manifeste.display()));
        let doc: toml::Value = texte
            .parse()
            .unwrap_or_else(|e| panic!("{} n'est pas un TOML valide : {e}", manifeste.display()));

        // Le `[workspace]` vide : c'est la seule chose qui empêche cargo de rattacher le
        // crate au workspace racine, et sa disparition casserait `cargo fuzz` d'un coup.
        assert!(
            doc.get("workspace").is_some(),
            "{} a perdu son `[workspace]` vide : cargo le rattacherait au workspace racine",
            manifeste.display()
        );

        let cibles = doc
            .get("bin")
            .and_then(toml::Value::as_array)
            .unwrap_or_else(|| panic!("{} ne déclare aucun [[bin]]", manifeste.display()));
        assert!(
            !cibles.is_empty(),
            "{} déclare une liste de cibles vide",
            manifeste.display()
        );

        for cible in cibles {
            let nom = cible
                .get("name")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("une cible de {} n'a pas de nom", manifeste.display()));
            let chemin = cible
                .get("path")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "la cible « {nom} » de {} n'a pas de chemin",
                        manifeste.display()
                    )
                });
            let fichier = crate_fuzz.join(chemin);
            assert!(
                fichier.is_file(),
                "la cible « {nom} » de {} pointe sur {}, qui n'existe pas",
                manifeste.display(),
                fichier.display()
            );
        }
    }
}

/// Pourquoi la construction des crates `fuzz/` ne peut pas être vérifiée ici, ou `None`.
fn raison_de_se_retirer() -> Option<String> {
    if env::var_os("NIX_BUILD_TOP").is_some() {
        return Some(
            "bac à sable Nix : pas de réseau, et les crates fuzz/ n'ont pas de Cargo.lock \
             versionné à vendorer. Le job `cargo` de la CI et les postes de développement \
             lancent ce test."
                .to_owned(),
        );
    }
    if env::var_os("CONDUIT_SANS_CHECK_FUZZ").is_some() {
        return Some("CONDUIT_SANS_CHECK_FUZZ est posée dans l'environnement".to_owned());
    }
    None
}

/// `cargo check` sur chaque crate `fuzz/` : c'est **la** parade contre la récidive.
///
/// Chaque crate est vérifié depuis son propre manifeste et dans son propre `fuzz/target`
/// (d'où le retrait de `CARGO_TARGET_DIR`, que le cargo appelant pose parfois) : rien de ce
/// que fait ce test ne rentre dans le workspace racine.
#[test]
fn chaque_crate_fuzz_construit() {
    if let Some(raison) = raison_de_se_retirer() {
        eprintln!(
            "construction des crates fuzz/ non vérifiée ici — {raison}\n\
             À la main : cargo check --manifest-path crates/<crate>/fuzz/Cargo.toml"
        );
        return;
    }

    let cargo = env!("CARGO");
    for crate_fuzz in crates_fuzz() {
        let manifeste = crate_fuzz.join("Cargo.toml");
        let debut = Instant::now();
        let sortie = Command::new(cargo)
            .arg("check")
            .arg("--manifest-path")
            .arg(&manifeste)
            // Le cargo appelant peut avoir dirigé toutes les sorties vers un target
            // partagé : le crate fuzz doit garder le sien, celui que `cargo fuzz` emploie.
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .unwrap_or_else(|e| panic!("lancement de « {cargo} check » sur {} : {e}", manifeste.display()));

        assert!(
            sortie.status.success(),
            "la cible de fuzz ne compile plus : « cargo check » a échoué sur {}\n\
             ----- sortie d'erreur -----\n{}\n\
             ---------------------------\n\
             Réparez la cible, ou retirez-la du manifeste si le code qu'elle éprouvait a \
             disparu. Ne désactivez pas ce test : c'est la troisième récidive qu'il empêche.",
            manifeste.display(),
            String::from_utf8_lossy(&sortie.stderr)
        );
        eprintln!(
            "{} : cargo check vert en {:.1?}",
            manifeste.display(),
            debut.elapsed()
        );
    }
}
