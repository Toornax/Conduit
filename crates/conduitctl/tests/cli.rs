//! Tests bout en bout de chaque commande contre un démon null (M0-91 à M0-95).

use std::path::Path;

use conduit_backend::null::{NullBackend, NullDeviceSpec};
use conduitctl::run;
use conduitd::config::Config;
use conduitd::{Daemon, DaemonOptions};

struct Ctx {
    daemon: Daemon,
    null: NullBackend,
    _dir: tempfile::TempDir,
}

async fn start(config: Config) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let null = NullBackend::new();
    null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
    null.add_device(NullDeviceSpec::capture("Micro").layout(1, 240));
    let daemon = Daemon::spawn(
        Box::new(null.clone()),
        DaemonOptions::under(dir.path(), config),
    )
    .unwrap();
    Ctx {
        daemon,
        null,
        _dir: dir,
    }
}

async fn ctl(socket: &Path, args: &[&str]) -> Result<String, String> {
    let mut v = vec!["--socket".to_string(), socket.display().to_string()];
    v.extend(args.iter().map(|s| s.to_string()));
    run(v).await.map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn help_documents_every_command() {
    let err = run(vec!["--help".into()]).await.unwrap_err().to_string();
    for cmd in [
        "status", "nodes", "ports", "links", "link", "unlink", "volume", "driver", "cable",
        "monitor", "dump", "load", "xruns", "save",
    ] {
        assert!(err.contains(cmd), "aide sans {cmd} : {err}");
    }
    let err = run(vec!["cable".into(), "--help".into()])
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("add")
            && err.contains("remove")
            && err.contains("rename")
            && err.contains("list")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_nodes_ports_links_and_json() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    let out = ctl(s, &["status"]).await.unwrap();
    assert!(
        out.contains("backend null")
            && out.contains("48 kHz")
            && out.contains("pilote null:haut-parleurs"),
        "{out}"
    );
    let nodes = ctl(s, &["nodes"]).await.unwrap();
    assert!(
        nodes.contains("Haut-parleurs") && nodes.contains("Micro") && nodes.contains("driver"),
        "{nodes}"
    );
    let ports = ctl(s, &["ports", "Haut-parleurs"]).await.unwrap();
    assert!(
        ports.contains("in0  FL") && ports.contains("in1  FR"),
        "{ports}"
    );
    let ports = ctl(s, &["ports", "micro"]).await.unwrap();
    assert!(ports.contains("out0  MONO"), "{ports}");
    let json = ctl(s, &["--json", "nodes"]).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["reply"], "nodes");
    assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
    let links = ctl(s, &["links"]).await.unwrap();
    assert!(links.starts_with("ID"), "{links}");
    let err = ctl(s, &["ports", "inexistant"]).await.unwrap_err();
    assert!(err.contains("aucun nœud"), "{err}");
    c.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn link_unlink_volume_driver() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    let out = ctl(s, &["link", "Micro:MONO", "Haut-parleurs:FL"])
        .await
        .unwrap();
    assert!(out.starts_with("lien l0.0"), "{out}");
    let out = ctl(s, &["link", "micro:0", "haut-parleurs:fr"])
        .await
        .unwrap();
    assert!(out.starts_with("lien l1.0"), "{out}");
    let err = ctl(s, &["link", "Micro:MONO", "Haut-parleurs:FL"])
        .await
        .unwrap_err();
    assert!(err.contains("existe déjà"), "{err}");
    let err = ctl(s, &["link", "Micro:FL", "Haut-parleurs:FL"])
        .await
        .unwrap_err();
    assert!(err.contains("port « FL » inconnu"), "{err}");
    let links = ctl(s, &["links"]).await.unwrap();
    assert_eq!(links.lines().count(), 3, "{links}");
    assert_eq!(ctl(s, &["volume", "l0.0", "-6"]).await.unwrap(), "ok\n");
    assert_eq!(
        ctl(s, &["volume", "Haut-parleurs", "mute"]).await.unwrap(),
        "ok\n"
    );
    assert_eq!(ctl(s, &["volume", "Micro", "+3dB"]).await.unwrap(), "ok\n");
    let nodes = ctl(s, &["nodes"]).await.unwrap();
    assert!(
        nodes.contains("(muet)") && nodes.contains("+3.0 dB"),
        "{nodes}"
    );
    let links = ctl(s, &["links"]).await.unwrap();
    assert!(links.contains("-6.0 dB"), "{links}");
    assert!(ctl(s, &["volume", "Micro", "fort"])
        .await
        .unwrap_err()
        .contains("volume invalide"));
    assert_eq!(ctl(s, &["unlink", "l1.0"]).await.unwrap(), "ok\n");
    assert!(ctl(s, &["unlink", "l1.0"])
        .await
        .unwrap_err()
        .contains("inconnu"));
    assert!(ctl(s, &["unlink", "zz"])
        .await
        .unwrap_err()
        .contains("identifiant de lien invalide"));
    // Pilote.
    assert_eq!(ctl(s, &["driver", "internal"]).await.unwrap(), "ok\n");
    assert!(ctl(s, &["driver"])
        .await
        .unwrap()
        .contains("pilote horloge interne"));
    assert_eq!(ctl(s, &["driver", "Haut-parleurs"]).await.unwrap(), "ok\n");
    assert!(ctl(s, &["status"])
        .await
        .unwrap()
        .contains("pilote null:haut-parleurs"));
    assert!(
        ctl(s, &["driver", "Micro"]).await.is_ok(),
        "un périphérique de capture peut piloter"
    );
    assert_eq!(ctl(s, &["driver", "auto"]).await.unwrap(), "ok\n");
    c.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cable_add_remove_rename_list() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    let out = ctl(s, &["cable", "add", "--name", "Musique"])
        .await
        .unwrap();
    // Le format entier, pas seulement les canaux : c'est ce qu'un câble sert.
    assert!(
        out.starts_with("câble 1 « Musique » 48 kHz float 32, 2 canaux"),
        "{out}"
    );
    let out = ctl(s, &["cable", "add", "--channels", "1"]).await.unwrap();
    assert!(out.contains("1 canal"), "{out}");
    let list = ctl(s, &["cable", "list"]).await.unwrap();
    assert_eq!(list.lines().count(), 3, "{list}");
    assert!(list.contains("Musique") && list.contains("Conduit 2"));
    // La colonne FORMAT : compacte, et son jeton de profondeur est celui qui se retape.
    assert!(list.contains("FORMAT"), "{list}");
    assert!(list.contains("48k f32"), "{list}");
    assert!(ctl(s, &["cable", "rename", "1", "Jeu"])
        .await
        .unwrap()
        .contains("« Jeu »"));
    assert!(ctl(s, &["cable", "channels", "2", "6"])
        .await
        .unwrap()
        .contains("6 canaux"));
    assert_eq!(ctl(s, &["cable", "remove", "1"]).await.unwrap(), "ok\n");
    assert!(ctl(s, &["cable", "remove", "1"])
        .await
        .unwrap_err()
        .contains("inconnu"));
    assert!(ctl(s, &["cable", "add", "--channels", "9"])
        .await
        .unwrap_err()
        .contains("entre 1 et 8"));
    let nodes = ctl(s, &["nodes"]).await.unwrap();
    assert!(nodes.contains("null:cable2:render"), "{nodes}");
    c.daemon.shutdown().await;
}

/// `cable set-format` : les champs donnés s'appliquent, les champs omis se conservent, et
/// chaque refus nomme son domaine.
///
/// La conservation est le point : `--channels 6` sur un câble réglé en 96 kHz PCM 24 ne
/// doit pas le ramener au défaut. C'est le `cable list` préalable de `conduitctl` qui le
/// garantit, et rien d'autre ne le vérifierait.
#[tokio::test(flavor = "multi_thread")]
async fn cable_set_format_keeps_the_fields_left_out() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    ctl(s, &["cable", "add"]).await.unwrap();

    let out = ctl(
        s,
        &[
            "cable",
            "set-format",
            "1",
            "--rate",
            "96000",
            "--depth",
            "pcm24",
        ],
    )
    .await
    .unwrap();
    assert!(out.contains("96 kHz PCM 24, 2 canaux"), "{out}");

    // Les canaux seuls : la fréquence et la profondeur sont conservées.
    let out = ctl(s, &["cable", "set-format", "1", "--channels", "6"])
        .await
        .unwrap();
    assert!(out.contains("96 kHz PCM 24, 6 canaux"), "{out}");
    let list = ctl(s, &["cable", "list"]).await.unwrap();
    assert!(list.contains("96k pcm24"), "{list}");

    // Chaque refus nomme le champ, la valeur reçue et le domaine.
    let e = ctl(s, &["cable", "set-format", "1", "--depth", "double"])
        .await
        .unwrap_err();
    assert!(e.contains("double") && e.contains("pcm16"), "{e}");
    let e = ctl(s, &["cable", "set-format", "1", "--channels", "9"])
        .await
        .unwrap_err();
    assert!(e.contains("9") && e.contains("1 à 8"), "{e}");
    // Un câble que cette machine ne sert pas : le message donne la commande qui liste.
    let e = ctl(s, &["cable", "set-format", "7", "--channels", "2"])
        .await
        .unwrap_err();
    assert!(e.contains("cable list"), "{e}");
    c.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_param_meter_label_remove_and_internal_nodes() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    let out = ctl(
        s,
        &[
            "add",
            "sine",
            "gen",
            "--frequency",
            "1000",
            "--amplitude",
            "0.5",
            "--channels",
            "1",
        ],
    )
    .await
    .unwrap();
    assert!(out.contains("sine « gen »"), "{out}");
    assert!(ctl(s, &["add", "meter", "vu", "--channels", "1"])
        .await
        .is_ok());
    ctl(
        s,
        &[
            "add",
            "eq",
            "eq",
            "--channels",
            "1",
            "--band",
            "peak:1000:1:-6",
            "--band",
            "lowpass:8000",
        ],
    )
    .await
    .unwrap();
    assert!(ctl(s, &["add", "eq", "bad", "--band", "wobble:1000"])
        .await
        .unwrap_err()
        .contains("bande invalide"));
    assert!(ctl(s, &["add", "noise", "n", "--white"]).await.is_ok());
    assert!(ctl(s, &["add", "mixer", "m", "--buses", "3"]).await.is_ok());
    assert!(ctl(s, &["add", "splitter", "sp"]).await.is_ok());
    assert!(ctl(
        s,
        &["add", "adapter", "ad", "--inputs", "6", "--outputs", "2"]
    )
    .await
    .is_ok());
    assert!(ctl(s, &["add", "silence", "si"]).await.is_ok());
    assert!(ctl(s, &["add", "silence", "gen"])
        .await
        .unwrap_err()
        .contains("existe déjà"));
    ctl(s, &["link", "gen:MONO", "eq:MONO"]).await.unwrap();
    ctl(s, &["link", "eq:MONO", "vu:MONO"]).await.unwrap();
    ctl(s, &["link", "vu:MONO", "Haut-parleurs:FL"])
        .await
        .unwrap();
    assert_eq!(
        ctl(s, &["param", "eq", "band.0.gain_db", "-12"])
            .await
            .unwrap(),
        "ok\n"
    );
    assert!(ctl(s, &["param", "eq", "band.9.q", "1"])
        .await
        .unwrap_err()
        .contains("bande inexistante"));
    // Le pilote (null, manuel) avance sur commande du test : 1 s d'audio.
    c.null.advance(std::time::Duration::from_secs(1));
    let meter = ctl(s, &["meter", "vu"]).await.unwrap();
    assert!(meter.contains("CANAL") && meter.contains("dBFS"), "{meter}");
    let json = ctl(s, &["--json", "meter", "vu"]).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let peak = v["channels"][0]["peak"].as_f64().unwrap();
    assert!(
        peak > 0.05 && peak < 0.3,
        "crête {peak} (−12 dB de la cloche EQ à 500 Hz sur 0,5)"
    );
    assert_eq!(
        ctl(s, &["label", "gen", "Générateur"]).await.unwrap(),
        "ok\n"
    );
    assert!(ctl(s, &["nodes"]).await.unwrap().contains("Générateur"));
    assert_eq!(ctl(s, &["remove", "si"]).await.unwrap(), "ok\n");
    assert!(ctl(s, &["remove", "Haut-parleurs"])
        .await
        .unwrap_err()
        .contains("présent"));
    c.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn monitor_xruns_dump_save_load() {
    let c = start(Config::default()).await;
    let s = &c.daemon.socket;
    let out = ctl(s, &["xruns"]).await.unwrap();
    assert!(out.starts_with("xruns 0"), "{out}");
    assert!(ctl(s, &["xruns", "--reset"])
        .await
        .unwrap()
        .starts_with("xruns 0"));
    let dump = ctl(s, &["dump"]).await.unwrap();
    assert!(
        dump.contains("conduitd") && dump.contains("nœuds (2)"),
        "{dump}"
    );
    assert_eq!(ctl(s, &["save"]).await.unwrap(), "ok\n");
    assert_eq!(ctl(s, &["load"]).await.unwrap(), "ok\n");
    // Monitor : un événement produit par une autre commande.
    let socket = s.clone();
    let monitor =
        tokio::spawn(
            async move { ctl(&socket, &["monitor", "--count", "1", "--timeout", "5"]).await },
        );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctl(s, &["add", "silence", "x"]).await.unwrap();
    let out = monitor.await.unwrap().unwrap();
    assert!(
        out.contains("nœud ajouté") && out.contains("« x »"),
        "{out}"
    );
    let socket = s.clone();
    let monitor = tokio::spawn(async move {
        ctl(
            &socket,
            &["--json", "monitor", "--count", "1", "--timeout", "5"],
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctl(s, &["remove", "x"]).await.unwrap();
    let out = monitor.await.unwrap().unwrap();
    assert!(out.contains("\"event\":\"node_removed\""), "{out}");
    let none = ctl(s, &["monitor", "--timeout", "0.2"]).await.unwrap();
    assert!(none.is_empty());
    c.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_daemon_gives_actionable_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = ctl(&dir.path().join("absent.sock"), &["status"])
        .await
        .unwrap_err();
    assert!(err.contains("Est-il démarré"), "{err}");
}
