//! `conduitd` — le démon : moteur, IPC, persistance, règles.

// `deny` et non `forbid` : le seul `unsafe` du démon est dans `session_end`, sous
// `cfg(windows)` — les fenêtres Win32 n'ont pas de façade sûre (voir ce module et
// docs/dev-guide.md §6). Partout ailleurs, y compris dans le binaire, l'`unsafe` reste
// interdit.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod autoconnect;
pub mod autostart;
pub mod config;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod persist;
pub mod service;
#[cfg(windows)]
pub mod session_end;
pub mod single_instance;

use std::path::PathBuf;
use std::sync::Arc;

use conduit_backend::{Backend, CableSpec};
use conduit_core::types::ChannelCount;
use conduit_engine::{Engine, EngineConfig};
use tokio::sync::watch;

use config::Config;
use paths::Paths;
use service::{Service, ServiceOptions};

/// Nom annoncé aux clients.
pub fn server_name() -> String {
    format!("conduitd {}", env!("CARGO_PKG_VERSION"))
}

/// Erreurs de démarrage.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Configuration.
    #[error("{0}")]
    Config(#[from] config::ConfigError),
    /// Moteur.
    #[error("moteur : {0}")]
    Engine(#[from] conduit_engine::EngineError),
    /// Entrée/sortie.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Options du démon.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// Chemins.
    pub paths: Paths,
    /// Configuration.
    pub config: Config,
    /// Watchdog du fil audio.
    pub watchdog: bool,
    /// Persistance de l'état.
    pub persist: bool,
}

impl DaemonOptions {
    /// Options sous un répertoire racine (tests).
    pub fn under(root: &std::path::Path, config: Config) -> Self {
        Self {
            paths: Paths::under(root),
            config,
            watchdog: false,
            persist: true,
        }
    }
}

/// Construit le moteur : fréquence, quantum, pilote et câbles de la configuration.
pub fn build_engine(mut backend: Box<dyn Backend>, config: &Config) -> Result<Engine, DaemonError> {
    ensure_cables(backend.as_mut(), config);
    let engine_config = EngineConfig {
        sample_rate: config.sample_rate(),
        quantum: config.quantum(),
        driver: config.driver_choice(),
        ..Default::default()
    };
    Ok(Engine::new(backend, engine_config)?)
}

/// Crée les câbles manquants de la configuration et applique les alias (F-01).
///
/// « Manquant » a deux sens selon la plateforme, et les deux sont traités. Là où les
/// câbles sont créés à la demande (backend `null`), un câble absent de la liste est
/// créé. Sous Windows le pilote a une **réserve fixe** de seize câbles (SPEC §5.4) :
/// ils sont tous listés, actifs ou non, et « manquant » veut dire **déconnecté** —
/// `create` est alors la façon de le connecter (M1b-34).
///
/// # Le format : sur un câble déconnecté, avant de le connecter — jamais autrement
///
/// Appliquer un format écrit `CableFormat<n>` dans la clé matérielle du périphérique puis
/// **redémarre celui-ci** : environ une seconde de silence sur les seize câbles, flux
/// ouverts compris. C'est trop cher pour une convergence silencieuse, et c'est pourquoi
/// les trois cas sont traités différemment :
///
/// | le câble est… | ce que fait le démon |
/// |---|---|
/// | déconnecté, conforme à sa section | il le connecte, sans rien écrire |
/// | déconnecté, format divergent | il applique le format **puis** connecte, en une seule séquence |
/// | connecté, format divergent | il **journalise** et n'écrit rien |
///
/// Le troisième cas est le seul choix discutable, et il est délibéré : un fichier TOML
/// relu au démarrage ne vaut pas une seconde de silence que personne n'a demandée sur un
/// câble en service — et, le format d'un endpoint étant figé à sa création, l'écriture ne
/// déplacerait de toute façon pas l'endpoint déjà publié. L'avertissement nomme
/// `conduitctl cable set-format`, qui fait le travail quand l'utilisateur, lui, le
/// demande.
fn ensure_cables(backend: &mut dyn Backend, config: &Config) {
    let Some(cc) = backend.cable_control() else {
        if !config.cables.is_empty() {
            tracing::warn!("ce backend ne gère pas les câbles : section [[cable]] ignorée");
        }
        return;
    };
    appliquer_les_cables(cc, config);
}

/// La moitié de [`ensure_cables`] qui **décide**, séparée de celle qui trouve le contrôle.
///
/// Séparée pour être vérifiable : le choix entre « connecter », « formater puis
/// connecter » et « journaliser sans rien écrire » est la seule chose que ce fichier ait à
/// dire sur les câbles, et un `Backend` complet n'est pas nécessaire pour l'exercer.
fn appliquer_les_cables(cc: &mut dyn conduit_backend::CableControl, config: &Config) {
    let existing = cc.list().unwrap_or_default();
    for c in &config.cables {
        let channels = ChannelCount::new(c.channels).unwrap_or_default();
        match existing.iter().find(|e| e.id.0 == c.id) {
            Some(e) => {
                let voulu = c.format(e.format);
                // Présent mais déconnecté : le connecter en le désignant par son nom,
                // pour ne pas activer le premier câble libre à sa place. Le format part
                // dans la même demande quand il diverge : `create` l'écrit **avant**
                // d'activer, seule séquence qui produise un endpoint au bon format.
                if !e.active {
                    let format = (voulu != e.format).then_some(voulu);
                    match cc.create(CableSpec {
                        name: Some(e.name.clone()),
                        channels,
                        format,
                    }) {
                        Ok(info) => match format {
                            Some(_) => {
                                tracing::info!("câble activé en {} : {}", info.format, info.name);
                            }
                            None => tracing::info!("câble activé : {}", info.name),
                        },
                        Err(err) => tracing::warn!("câble {} : {err}", c.id),
                    }
                } else if voulu != e.format {
                    tracing::warn!(
                        "câble {} : la configuration demande « {voulu} » et le câble sert \
                         « {} », mais il est connecté — changer son format demande de \
                         redémarrer son périphérique, soit environ une seconde de silence \
                         sur tous les câbles. Rien n'a été écrit ; utilisez « conduitctl \
                         cable remove {} », « conduitctl cable set-format {} », puis \
                         « conduitctl cable add » quand cela vous convient",
                        c.id,
                        e.format,
                        c.id,
                        c.id
                    );
                }
                if let Some(alias) = &c.alias {
                    if &e.name != alias {
                        // Le renommage écrit dans le registre depuis M1b-21 : son échec
                        // se journalise, comme celui d'une activation. Il est rejoué à
                        // chaque démarrage — `CableInfo::name` rend le nom d'origine du
                        // câble, pas le nom personnalisé, que seul le dorsal WASAPI
                        // connaît — mais réécrire la même valeur ne coûte rien.
                        match cc.rename(e.id, alias) {
                            Ok(info) => tracing::info!("câble renommé : {}", info.name),
                            Err(err) => tracing::warn!("câble {} non renommé : {err}", c.id),
                        }
                    }
                }
            }
            // Absent de la liste : la plateforme crée ses câbles à la demande. Le format
            // part avec la création, où il ne coûte rien — il n'y a pas encore d'endpoint
            // à interrompre. Il n'est donné que si la section l'a dit : sans `rate` ni
            // `depth`, `format()` rend le défaut portable et la demande reste muette.
            None => {
                let voulu = c.format(conduit_backend::CableFormat::default());
                let format = (c.rate().is_some() || c.depth().is_some()).then_some(voulu);
                match cc.create(CableSpec {
                    name: c.alias.clone(),
                    channels,
                    format,
                }) {
                    Ok(info) => tracing::info!("câble créé : {} ({})", info.name, info.format),
                    Err(e) => tracing::warn!("câble {} : {e}", c.id),
                }
            }
        }
    }
}

/// Démon en cours d'exécution.
pub struct Daemon {
    /// Chemin ou nom du socket.
    pub socket: PathBuf,
    /// Service (commandes directes, sans IPC).
    pub service: Arc<Service>,
    shutdown: watch::Sender<bool>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl core::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Daemon")
            .field("socket", &self.socket)
            .finish()
    }
}

impl Daemon {
    /// Démarre le moteur, le service et le serveur IPC (dans le runtime courant).
    pub fn spawn(backend: Box<dyn Backend>, options: DaemonOptions) -> Result<Self, DaemonError> {
        options.paths.ensure_dirs()?;
        let engine = build_engine(backend, &options.config)?;
        let service = Arc::new(Service::start(
            engine,
            ServiceOptions {
                state_file: options.persist.then(|| options.paths.state_file.clone()),
                rules: options.config.autoconnect.clone(),
                watchdog: options.watchdog,
                ..Default::default()
            },
        ));
        let listener = ipc::Listener::bind(&options.paths.socket)?;
        let (shutdown, rx) = watch::channel(false);
        let server = tokio::spawn(ipc::serve(
            listener,
            Arc::clone(&service),
            rx,
            server_name(),
        ));
        tracing::info!("démon prêt sur {}", options.paths.socket.display());
        Ok(Self {
            socket: options.paths.socket.clone(),
            service,
            shutdown,
            server: Some(server),
        })
    }

    /// Arrête le serveur IPC puis le service.
    ///
    /// Au retour, la boucle d'acceptation **et** toutes les sessions clientes sont
    /// terminées : le socket Unix est supprimé et plus aucune instance du named pipe
    /// n'est ouverte. Un démon peut donc être relancé aussitôt sur le même chemin, sans
    /// attente ni nouvelle tentative.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(s) = self.server.take() {
            let _ = s.await;
        }
        if let Ok(mut service) = Arc::try_unwrap(self.service) {
            service.shutdown();
        }
    }

    /// Tourne jusqu'au signal d'arrêt (Ctrl-C / SIGTERM), et sous Windows jusqu'à la
    /// fermeture de session (`WM_ENDSESSION`, voir le module `session_end` —
    /// en code et non en lien : il n'existe que sous Windows).
    pub async fn run_until_signal(self) {
        #[cfg(windows)]
        {
            self.run_until_signal_windows().await;
        }
        #[cfg(not(windows))]
        {
            wait_for_signal().await;
            tracing::info!("arrêt demandé");
            self.shutdown().await;
        }
    }

    /// Variante Windows : Ctrl-C **ou** fin de session.
    ///
    /// L'accusé de réception est envoyé après l'arrêt complet (état sauvegardé, flux
    /// fermés) : c'est lui qui laisse le système achever la fermeture de session.
    #[cfg(windows)]
    async fn run_until_signal_windows(self) {
        let session = session_end::spawn();
        match &session {
            Some(s) => {
                tokio::select! {
                    _ = wait_for_signal() => tracing::info!("arrêt demandé"),
                    _ = s.wait() => tracing::info!("fermeture de session Windows : arrêt propre"),
                }
            }
            None => {
                wait_for_signal().await;
                tracing::info!("arrêt demandé");
            }
        }
        self.shutdown().await;
        if let Some(s) = session {
            s.acknowledge();
        }
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use conduit_backend::cable::SampleDepth;
    use conduit_backend::{CableControl, CableError, CableFormat, CableId, CableInfo, DeviceId};
    use conduit_core::types::SampleRate;
    use config::CableSection;

    /// Un contrÃ´le des cÃ¢bles Ã  **rÃ©serve fixe**, comme celui de Windows : les cÃ¢bles
    /// existent tous, actifs ou non, et chaque Ã©criture est notÃ©e.
    ///
    /// C'est la forme qui compte : le backend `null` crÃ©e ses cÃ¢bles Ã  la demande et n'a
    /// pas d'Ã©tat Â« prÃ©sent mais dÃ©connectÃ© Â», donc il ne peut pas exercer les trois cas
    /// que [`appliquer_les_cables`] distingue.
    #[derive(Debug, Default)]
    struct Reserve {
        cables: Vec<CableInfo>,
        /// Les Ã©critures faites, dans l'ordre : de quoi dire ce qui a Ã©tÃ© touchÃ© **et**
        /// ce qui ne l'a pas Ã©tÃ©.
        gestes: Vec<String>,
    }

    impl Reserve {
        fn avec(cables: Vec<CableInfo>) -> Self {
            Self {
                cables,
                gestes: Vec::new(),
            }
        }

        fn index(&self, id: CableId) -> Result<usize, CableError> {
            self.cables
                .iter()
                .position(|c| c.id == id)
                .ok_or(CableError::NotFound(id))
        }
    }

    impl CableControl for Reserve {
        fn max_cables(&self) -> usize {
            16
        }

        fn list(&self) -> Result<Vec<CableInfo>, CableError> {
            Ok(self.cables.clone())
        }

        fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError> {
            let nom = spec.name.clone().unwrap_or_default();
            let i = self
                .cables
                .iter()
                .position(|c| c.name == nom)
                .ok_or(CableError::LimitReached { max: 16 })?;
            let id = self.cables[i].id;
            // Le vrai contrÃ´le Ã©crit le format **avant** d'activer ; le double note la
            // mÃªme sÃ©quence, et c'est elle que le test vÃ©rifie.
            if let Some(format) = spec.format {
                self.set_format(id, format)?;
            }
            self.gestes.push(format!("activer {}", id.0));
            self.cables[i].active = true;
            Ok(self.cables[i].clone())
        }

        fn remove(&mut self, id: CableId) -> Result<(), CableError> {
            let i = self.index(id)?;
            self.gestes.push(format!("desactiver {}", id.0));
            self.cables[i].active = false;
            Ok(())
        }

        fn set_channels(
            &mut self,
            id: CableId,
            channels: ChannelCount,
        ) -> Result<CableInfo, CableError> {
            let i = self.index(id)?;
            self.gestes
                .push(format!("canaux {} {}", id.0, channels.get()));
            // Comme le pilote : il refuse tout compte qui n'est pas celui de son format.
            Err(CableError::Unsupported(format!(
                "ce cÃ¢ble sert {} canaux",
                self.cables[i].channels.get()
            )))
        }

        fn set_format(
            &mut self,
            id: CableId,
            format: CableFormat,
        ) -> Result<CableInfo, CableError> {
            let i = self.index(id)?;
            self.gestes.push(format!("format {} {format}", id.0));
            if self.cables[i].active {
                return Err(CableError::Unsupported("ce cÃ¢ble est connectÃ©".into()));
            }
            self.cables[i].format = format;
            self.cables[i].channels = format.channels;
            Ok(self.cables[i].clone())
        }

        fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError> {
            let i = self.index(id)?;
            self.gestes.push(format!("renommer {} {name}", id.0));
            self.cables[i].name = name.to_owned();
            Ok(self.cables[i].clone())
        }
    }

    fn cable(id: u32, actif: bool, format: CableFormat) -> CableInfo {
        CableInfo {
            id: CableId(id),
            name: format!("Conduit {id}"),
            channels: format.channels,
            format,
            active: actif,
            render: DeviceId::new(format!("conduit:cable{id}:rendu")),
            capture: DeviceId::new(format!("conduit:cable{id}:capture")),
        }
    }

    fn section(id: u32, rate: Option<u32>, depth: Option<&str>, channels: u8) -> CableSection {
        CableSection {
            id,
            alias: None,
            channels,
            rate,
            depth: depth.map(str::to_owned),
        }
    }

    fn config_de(cables: Vec<CableSection>) -> Config {
        Config {
            cables,
            ..Config::default()
        }
    }

    /// **Les trois cas du format**, en une seule table.
    #[test]
    fn le_format_ne_part_que_sur_un_cable_deconnecte() {
        let defaut = CableFormat::default();
        let six = CableFormat {
            channels: ChannelCount::new(6).unwrap(),
            ..defaut
        };

        // 1. DÃ©connectÃ©, conforme : on connecte, et rien d'autre.
        let mut r = Reserve::avec(vec![cable(1, false, defaut)]);
        appliquer_les_cables(&mut r, &config_de(vec![section(1, None, None, 2)]));
        assert_eq!(r.gestes, vec!["activer 1".to_string()]);
        assert!(r.cables[0].active);

        // 2. DÃ©connectÃ©, divergent : le format **puis** l'activation, dans cet ordre.
        let mut r = Reserve::avec(vec![cable(1, false, defaut)]);
        appliquer_les_cables(&mut r, &config_de(vec![section(1, Some(96_000), None, 6)]));
        assert_eq!(
            r.gestes,
            vec![
                "format 1 96 kHz float 32, 6 canaux".to_string(),
                "activer 1".to_string()
            ],
            "le format avant l'activation, jamais aprÃ¨s"
        );
        assert_eq!(r.cables[0].format.sample_rate, SampleRate::HZ_96000);
        assert_eq!(r.cables[0].channels.get(), 6);

        // 3. ConnectÃ©, divergent : **rien n'est Ã©crit**. Ni `format`, ni `canaux` â€” la
        // seconde de silence n'est pas prise sans qu'on la demande.
        let mut r = Reserve::avec(vec![cable(1, true, defaut)]);
        appliquer_les_cables(&mut r, &config_de(vec![section(1, Some(96_000), None, 6)]));
        assert!(r.gestes.is_empty(), "{:?}", r.gestes);
        assert_eq!(
            r.cables[0].format, defaut,
            "le cÃ¢ble sert toujours son format"
        );

        // ConnectÃ© et conforme : rien non plus, Ã©videmment.
        let mut r = Reserve::avec(vec![cable(1, true, six)]);
        appliquer_les_cables(&mut r, &config_de(vec![section(1, None, None, 6)]));
        assert!(r.gestes.is_empty(), "{:?}", r.gestes);
    }

    /// Une section muette sur la frÃ©quence **ne la ramÃ¨ne pas au dÃ©faut**.
    ///
    /// Le piÃ¨ge Ã©vident de la convergence : un cÃ¢ble rÃ©glÃ© en 96 kHz PCM 24, un fichier
    /// qui ne parle que des canaux, et un redÃ©marrage de pÃ©riphÃ©rique par dÃ©marrage du
    /// dÃ©mon.
    #[test]
    fn une_section_muette_ne_touche_a_rien() {
        let exotique = CableFormat {
            sample_rate: SampleRate::HZ_96000,
            depth: SampleDepth::Pcm24,
            channels: ChannelCount::new(4).unwrap(),
        };
        let mut r = Reserve::avec(vec![cable(1, false, exotique)]);
        appliquer_les_cables(&mut r, &config_de(vec![section(1, None, None, 4)]));
        assert_eq!(r.gestes, vec!["activer 1".to_string()]);
        assert_eq!(r.cables[0].format, exotique);
    }

    /// L'alias vient **aprÃ¨s** l'activation : renommer un cÃ¢ble demande des endpoints
    /// publiÃ©s, donc un cÃ¢ble connectÃ©.
    #[test]
    fn l_alias_vient_apres_l_activation() {
        let mut r = Reserve::avec(vec![cable(1, false, CableFormat::default())]);
        let mut s = section(1, Some(44_100), None, 2);
        s.alias = Some("Musique".into());
        appliquer_les_cables(&mut r, &config_de(vec![s]));
        assert_eq!(
            r.gestes,
            vec![
                "format 1 44100 Hz float 32, 2 canaux".to_string(),
                "activer 1".to_string(),
                "renommer 1 Musique".to_string(),
            ]
        );
    }
}
