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
//! ```
//!
//! `--loopback` répond à l'autre moitié de la question. Au lieu d'ouvrir un
//! endpoint de capture, l'outil ouvre l'endpoint de **rendu** en écho
//! (`AUDCLNT_STREAMFLAGS_LOOPBACK`) et prélève le mélange **avant** qu'il
//! n'atteigne le pilote. Le sinus entendu en écho met le moteur audio hors de
//! cause et laisse le pilote seul suspect ; un écho silencieux fait l'inverse.
//! C'est la première mesure à prendre quand une capture n'entend pas un rendu.
//!
//! Codes de retour : `0` toutes les passes passent, `1` au moins une échoue,
//! `2` l'environnement ne permet pas le test (endpoints absents, backend
//! indisponible, options incohérentes, système autre que Windows).
//!
//! Organisation : [`analysis`] fait tout le calcul (sans plateforme, testé à
//! fond), [`pass`] enchaîne préparation, découpe du préambule et verdict,
//! [`report`] met en forme, `loopback` (Windows seulement, d'où le nom en code
//! et non en lien : il ne se résoudrait pas ailleurs) pilote les vrais flux WASAPI
//! par `conduit-backend-wasapi` — WASAPI n'est pas réécrit ici.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analysis;
pub mod cli;
#[cfg(windows)]
pub mod loopback;
pub mod pass;
pub mod report;

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

    /// Une passe d'enregistrement.
    fn record(&mut self, args: &cli::Args) -> Result<Vec<f32>, String> {
        match self {
            // 50 ms de préambule : le décalage qu'une vraie capture voit avant
            // que le rendu ne démarre.
            Source::Simulated => Ok(pass::simulate(
                &args.spec(),
                args.frames(),
                args.rate as usize / 20,
                args.glitch_frame(),
            )),
            Source::Live(session) => session.record(),
        }
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
        print!("{}", loopback::list()?);
        return Ok(ExitCode::SUCCESS);
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
        let recording = source.record(args)?;
        let pass = match pass::evaluate(index, &recording, &spec, &options, &tolerances, skip) {
            Ok(pass) => pass,
            // En écho, l'absence de signal n'est pas un accident d'environnement :
            // c'est **la** réponse que le mode sert à obtenir. On la commente.
            Err(e) if args.loopback => return Err(format!("{e}\n\n{}", report::loopback_silent())),
            Err(e) => return Err(e),
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

/// `--no-capture` : on joue, on vérifie seulement que la lecture ne casse pas.
#[cfg(windows)]
fn play_only(args: &cli::Args, session: &mut loopback::Session) -> Result<ExitCode, String> {
    for index in 1..=args.repeat {
        session.record()?;
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
