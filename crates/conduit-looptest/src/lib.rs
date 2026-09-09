//! `conduit-looptest` — test de boucle du pilote Conduit (ROADMAP M1a-10).
//!
//! L'outil joue un sinus sur un endpoint de **rendu**, enregistre ce qui revient
//! sur un endpoint de **capture**, et vérifie que c'est bien le même signal :
//! fréquence, amplitude, continuité de phase (aucune trame perdue ni dupliquée)
//! et absence de trous. Par défaut, les deux endpoints sont les deux côtés du
//! câble `Conduit 1` : c'est le pilote qu'on teste, pas la carte son.
//!
//! ```text
//! conduit-looptest --repeat 10          # le critère « Fait quand » de la ROADMAP
//! conduit-looptest --list               # les endpoints vus par WASAPI
//! conduit-looptest --json --repeat 10   # sortie machine
//! conduit-looptest --self-test          # test de l'outil, sans périphérique
//! conduit-looptest --loopback           # capture en écho : le moteur délivre-t-il ?
//! conduit-looptest --exclusif --seconds 20     # rendu et capture en exclusif WASAPI
//! conduit-looptest --list --show-volume # les endpoints, volume, coupure et plage
//! conduit-looptest --set-volume 0.5 --unmute   # règle, affiche, et sort
//! conduit-looptest --cable-etat                # l'état des 16 câbles (propriété KS)
//! conduit-looptest --cable-privilege           # l'état du privilège d'écriture, seul
//! conduit-looptest --cable 3 --cable-set connecte --cable-chrono
//! conduit-looptest --cable 3 --cable-invalide  # la batterie d'entrées invalides
//! ```
//!
//! Les options `--cable-*` ne mesurent rien : elles parlent au **jeu de propriétés KS
//! privé** du pilote (`KSPROPSETID_Conduit`, M1b-04) par
//! `conduit_backend_wasapi::cable`, affichent leur compte rendu et sortent. Elles
//! servent les deux moitiés du critère de M1b-04 : `--cable-chrono` mesure le délai
//! entre l'écriture et l'endpoint qui suit (F-01, « moins d'une seconde, sans PnP »), et
//! `--cable-invalide` envoie ce que le contrat refuse en affichant le **code d'erreur
//! Win32 brut** de chaque refus. Voir le module `cable` (Windows seulement, d'où le nom
//! en code et non en lien).
//!
//! **Écrire demande `SeLoadDriverPrivilege` armé, pas seulement détenu.** Le pilote
//! contrôle l'écriture par `SeSinglePrivilegeCheck`, qui exige le privilège *actif*, et
//! Windows livre les jetons avec leurs privilèges désactivés — élévation et
//! `LocalSystem` compris, ce que la mesure en machine virtuelle a établi. `--cable-set`
//! et `--cable-invalide` l'arment donc eux-mêmes, le temps de leurs écritures, et
//! restaurent le jeton ensuite. `--cable-privilege` n'arme rien : il ne fait que dire
//! dans quel état est le privilège, ce qui sépare « mauvais compte » de « bogue du
//! pilote » quand un refus 1314 persiste.
//!
//! `--loopback` répond à l'autre moitié de la question. Au lieu d'ouvrir un
//! endpoint de capture, l'outil ouvre l'endpoint de **rendu** en écho
//! (`AUDCLNT_STREAMFLAGS_LOOPBACK`) et prélève le mélange **avant** qu'il
//! n'atteigne le pilote. Le sinus entendu en écho met le moteur audio hors de
//! cause et laisse le pilote seul suspect ; un écho silencieux fait l'inverse.
//! C'est la première mesure à prendre quand une capture n'entend pas un rendu.
//!
//! `--exclusif` ouvre les deux flux en **mode exclusif WASAPI**, événementiel : le
//! moteur audio de Windows est court-circuité et chaque flux prend son endpoint pour
//! lui seul. Aucun code WASAPI n'est écrit ici — la politique
//! `ExclusivePolicy::Required` du dorsal fait tout le travail —, et `Required` veut
//! dire que l'exclusif refusé est une **erreur** : un repli silencieux en partagé
//! ferait mesurer le moteur audio en croyant mesurer le transport (notifications
//! WaveRT). L'en-tête dit le mode de chaque flux, puis, dès que les flux sont
//! ouverts, le format matériel négocié, la période et le tampon obtenus. Ce n'est pas
//! le mode normal d'un câble : un câble Conduit est fait pour coexister avec les
//! autres applications. Incompatible avec `--loopback`, dont l'écho n'existe qu'en
//! partagé.
//!
//! **Le volume de l'endpoint est vérifié avant chaque mesure.** Un endpoint coupé
//! ou à zéro rend la boucle muette, et ce silence-là est indiscernable d'un pilote
//! en panne : l'outil le relève, le dit, et nomme l'option qui corrige
//! (`--set-volume`, `--unmute`). Pour la même raison, le diagnostic d'une mesure
//! sans signal imprime la **session Windows** du processus : dans la session des
//! services (session 0), il n'y a pas d'audio d'utilisateur et la mesure n'a aucun
//! sens.
//!
//! `--show-volume` relève en outre la **plage** de chaque endpoint en décibels
//! (minimum, maximum, pas). Ce n'est pas un diagnostic mais une mesure, et elle a
//! une question à trancher : celle de l'échelle de `KSPROPERTY_AUDIO_VOLUMELEVEL`
//! que le pilote Conduit exposera (voir [`volume`]). Le relevé est passif —
//! aucun flux n'est ouvert, aucun son n'est émis.
//!
//! Codes de retour : `0` toutes les passes passent, `1` au moins une échoue,
//! `2` l'environnement ne permet pas le test (endpoints absents, backend
//! indisponible, options incohérentes, système autre que Windows).
//!
//! Organisation : [`analysis`] fait tout le calcul (sans plateforme, testé à
//! fond), [`pass`] enchaîne préparation, découpe du préambule et verdict,
//! [`report`] et [`volume`] mettent en forme (sans plateforme non plus : les
//! messages comptent autant que la mesure), `loopback` (Windows seulement, d'où le
//! nom en code et non en lien : il ne se résoudrait pas ailleurs) pilote les vrais
//! flux WASAPI par `conduit-backend-wasapi` — WASAPI n'est pas réécrit ici.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analysis;
#[cfg(windows)]
pub mod cable;
pub mod cli;
#[cfg(windows)]
pub mod loopback;
pub mod pass;
pub mod report;
pub mod volume;

use std::process::ExitCode;

/// Code de retour : au moins une passe a échoué.
pub const EXIT_FAILED: u8 = 1;
/// Code de retour : l'environnement ne permet pas le test.
pub const EXIT_ENVIRONMENT: u8 = 2;

/// Exécute l'outil.
///
/// # Erreurs
///
/// Message en français quand l'environnement ne permet pas le test : l'appelant
/// en fait un code de retour [`EXIT_ENVIRONMENT`].
#[cfg(not(windows))]
pub fn run(_args: &cli::Args) -> Result<ExitCode, String> {
    Err(
        "conduit-looptest teste le pilote Windows par WASAPI : il ne fonctionne que sur \
         Windows. Ailleurs, seuls ses tests tournent (`cargo test -p conduit-looptest`)."
            .to_string(),
    )
}

/// D'où viennent les trames d'une passe.
#[cfg(windows)]
enum Source<'a> {
    /// Boucle simulée en mémoire (`--self-test`) : aucun périphérique ouvert.
    Simulated,
    /// Vrais flux WASAPI.
    Live(&'a mut loopback::Session),
}

#[cfg(windows)]
impl Source<'_> {
    /// Sinus joué **et** analysé. Une session peut l'imposer : en `--loopback`,
    /// c'est le format de mixage de l'endpoint de rendu qui décide.
    fn spec(&self, args: &cli::Args) -> analysis::SineSpec {
        match self {
            Source::Simulated => args.spec(),
            Source::Live(session) => session.spec(),
        }
    }

    /// Une passe d'enregistrement. `entete` demande de compléter l'en-tête avec ce
    /// que les flux ont négocié, dès leur ouverture et avant la mesure.
    fn record(&mut self, args: &cli::Args, entete: bool) -> Result<Vec<f32>, String> {
        match self {
            // 50 ms de préambule : le décalage qu'une vraie capture voit avant
            // que le rendu ne démarre.
            Source::Simulated => Ok(pass::simulate(
                &args.spec(),
                args.frames(),
                args.rate as usize / 20,
                args.glitch_frame(),
            )),
            Source::Live(session) => session.record(&mut |bloc| imprime_negocie(entete, bloc)),
        }
    }
}

/// Le bloc « négocié » rendu par une ouverture de flux, quand il y a lieu de
/// l'imprimer : à la première passe, et hors `--json` (dont le document n'a pas de
/// place pour un en-tête de texte).
#[cfg(windows)]
fn imprime_negocie(actif: bool, bloc: Option<&str>) {
    if let (true, Some(bloc)) = (actif, bloc) {
        println!("{bloc}");
    }
}

/// Exécute l'outil.
///
/// # Erreurs
///
/// Message en français quand l'environnement ne permet pas le test (backend
/// WASAPI indisponible, endpoint absent, options incohérentes, enregistrement
/// inexploitable) : l'appelant en fait un code de retour [`EXIT_ENVIRONMENT`].
#[cfg(windows)]
pub fn run(args: &cli::Args) -> Result<ExitCode, String> {
    let rate = args.validate()?;
    if args.list {
        print!("{}", loopback::list(args.show_volume)?);
        return Ok(ExitCode::SUCCESS);
    }
    if args.adjusts_volume() {
        print!("{}", loopback::adjust_volume(args)?);
        return Ok(ExitCode::SUCCESS);
    }
    if args.controls_cable() {
        let rapport = cable::run(args)?;
        print!("{}", rapport.texte);
        return Ok(if rapport.conforme {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(EXIT_FAILED)
        });
    }
    if args.self_test {
        if !args.json {
            println!(
                "test de l'outil : boucle simulée en mémoire, aucun périphérique ouvert{}",
                match args.glitch_frame() {
                    Some(frame) => format!(" (trame {frame} retirée)"),
                    None => String::new(),
                }
            );
        }
        return run_passes(args, Source::Simulated);
    }
    let mut session = loopback::Session::open(args, rate)?;
    if !args.json {
        println!("{}", session.description());
    }
    // Avant toute mesure : un endpoint coupé ou à zéro rend la boucle muette, et le
    // silence qui en résulte est indiscernable d'un pilote en panne. L'avertissement
    // part sur la sortie d'erreur, pour rester visible même en `--json`.
    let levels = session.levels();
    if args.show_volume && !args.json {
        if let Some(block) = volume::levels_block(&levels) {
            println!("{block}");
        }
    }
    for warning in volume::warnings(&levels) {
        eprintln!("{warning}");
    }
    if args.capture_disabled() {
        return play_only(args, &mut session);
    }
    run_passes(args, Source::Live(&mut session))
}

/// Enchaîne les passes, les affiche et rend le code de retour.
#[cfg(windows)]
fn run_passes(args: &cli::Args, mut source: Source<'_>) -> Result<ExitCode, String> {
    let spec = source.spec(args);
    let options = args.analysis_options();
    let tolerances = args.tolerances();
    let skip = args.skip_frames_at(spec.sample_rate);
    let mut passes = Vec::with_capacity(args.repeat);
    for index in 1..=args.repeat {
        // Le format matériel n'est connu qu'une fois les flux ouverts : il complète
        // l'en-tête à la première passe, et ne se répète pas ensuite.
        let recording = source.record(args, index == 1 && !args.json)?;
        let pass = match pass::evaluate(index, &recording, &spec, &options, &tolerances, skip) {
            Ok(pass) => pass,
            Err(e) => return Err(no_signal_report(args, &e)),
        };
        if !args.json {
            println!("{}", report::pass_lines(&pass, args.repeat));
        }
        passes.push(pass);
    }
    let all_ok = passes.iter().all(|p| p.verdict.ok);
    if args.json {
        let document = report::json_document(&spec, &passes);
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|e| format!("sérialisation JSON : {e}"))?
        );
    } else {
        println!("{}", report::summary_line(&passes));
        // Le sinus a été retrouvé dans l'écho (sinon `evaluate` aurait échoué plus
        // haut) : c'est le verdict qui compte ici, pas la qualité de la boucle.
        if args.loopback {
            println!("\n{}", report::loopback_heard());
        }
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILED)
    })
}

/// Le message d'une passe inexploitable, augmenté de ce qui explique un silence.
///
/// Le diagnostic n'est joint qu'à une **absence de signal** : c'est le seul cas où
/// la question « pourquoi rien ? » se pose. Un enregistrement trop court est un
/// réglage à corriger, et une liste de causes possibles n'y ferait que du bruit.
///
/// S'y ajoutent, dans l'ordre où on les vérifie : la lecture de l'écho quand c'est
/// lui qui n'a rien entendu, puis la **session Windows** — un test lancé dans la
/// session des services n'a pas l'audio de l'utilisateur, et la mesure n'y a aucun
/// sens. Le volume, lui, a déjà été relevé et signalé avant la passe.
#[cfg(windows)]
fn no_signal_report(args: &cli::Args, error: &pass::Unusable) -> String {
    if !error.is_no_signal() {
        return error.message().to_string();
    }
    let mut out = error.message().to_string();
    if args.loopback {
        out.push_str(&format!("\n\n{}", report::loopback_silent()));
    }
    out.push_str(&format!(
        "\n\n{}",
        report::session_note(conduit_backend_wasapi::current_session_id())
    ));
    out
}

/// `--no-capture` : on joue, on vérifie seulement que la lecture ne casse pas.
#[cfg(windows)]
fn play_only(args: &cli::Args, session: &mut loopback::Session) -> Result<ExitCode, String> {
    for index in 1..=args.repeat {
        let entete = index == 1 && !args.json;
        session.record(&mut |bloc| imprime_negocie(entete, bloc))?;
        if !args.json {
            println!(
                "passe {index}/{} : {} s jouées sans erreur (mode --no-capture, rien à analyser)",
                args.repeat,
                report::fr(args.seconds, 1)
            );
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "outil": "conduit-looptest",
                "mode": "no-capture",
                "sinus": args.spec(),
                "resume": { "passes": args.repeat, "verdict": "ok" }
            })
        );
    } else {
        println!(
            "résumé : {} passe(s) de lecture seule, aucune erreur",
            args.repeat
        );
    }
    Ok(ExitCode::SUCCESS)
}
