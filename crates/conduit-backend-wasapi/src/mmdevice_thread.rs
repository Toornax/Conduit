//! Le fil dédié qui possède l'`IMMDeviceEnumerator`.
//!
//! COM est initialisé ici (MTA) et tout ce qui touche à MMDevice s'exécute ici :
//! énumération, recherche du périphérique par défaut, description d'un endpoint
//! signalé par une notification, ouverture d'un flux (création et initialisation
//! des objets WASAPI, qui partent ensuite vers le fil du flux). Le reste du
//! programme parle à ce fil par `std::sync::mpsc` ([`Command`]) ; le client de
//! notification y poste ses rappels ([`Notification`]) par le même canal. Les
//! [`DeviceEvent`] produits sont diffusés par un [`EventBroadcaster`] que ce fil
//! possède aussi.

use std::collections::BTreeSet;
use std::sync::mpsc;

use conduit_backend::event::EventBroadcaster;
use conduit_backend::{
    BackendError, DeviceDirection, DeviceEvent, DeviceId, EventReceiver, StreamFormat,
};
use windows::Win32::Media::Audio::{
    eConsole, IMMDeviceEnumerator, IMMNotificationClient, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::com::{platform_error, ComApartment};
use crate::devices::{
    default_endpoint_id, describe_id, direction_from_flow, enumerate, EndpointInfo,
};
use crate::exclusive::ExclusivePolicy;
use crate::lowlat::SharedPeriod;
use crate::notify::{Notification, NotificationClient};
use crate::open::Opened;

/// Ordre envoyé par le [`WasapiBackend`](crate::WasapiBackend).
pub(crate) enum Command {
    /// Énumérer les endpoints actifs.
    ///
    /// La réponse porte des [`EndpointInfo`] et non des `DeviceInfo` : le dorsal a
    /// besoin de la **description** de chaque endpoint pour rendre à un câble renommé le
    /// nom qu'il affiche, et une seconde énumération pour aller la chercher coûterait un
    /// aller-retour COM par appel.
    Enumerate {
        reply: mpsc::Sender<Result<Vec<EndpointInfo>, BackendError>>,
    },
    /// Périphérique par défaut (`eConsole`) d'un sens.
    DefaultDevice {
        direction: DeviceDirection,
        reply: mpsc::Sender<Result<Option<DeviceId>, BackendError>>,
    },
    /// Ouvrir un flux (objets WASAPI créés ici, consommés par le fil du flux).
    /// `policy` décide du mode de partage (partagé par défaut) ; `period` décide de
    /// la période d'un flux partagé (au choix du moteur par défaut, module
    /// `lowlat`) ; `loopback` demande une capture d'écho sur un endpoint de rendu
    /// (module `loopback`).
    Open {
        id: DeviceId,
        format: StreamFormat,
        policy: ExclusivePolicy,
        period: SharedPeriod,
        loopback: bool,
        reply: mpsc::Sender<Result<Opened, BackendError>>,
    },
    /// Créer un abonnement aux événements.
    Subscribe { reply: mpsc::Sender<EventReceiver> },
    /// Désenregistrer le client de notification et arrêter le fil.
    Shutdown,
}

/// Ce que reçoit le fil : une commande du backend ou un rappel COM.
pub(crate) enum Message {
    Command(Command),
    Notification(Notification),
}

/// Corps du fil MMDevice. Ne retourne qu'à l'arrêt.
///
/// `ready` reçoit `Ok(())` une fois COM initialisé, l'énumérateur créé, le client
/// enregistré et l'énumération initiale faite ; sinon l'erreur qui a tout arrêté.
pub(crate) fn run(
    receiver: mpsc::Receiver<Message>,
    sender: mpsc::Sender<Message>,
    ready: mpsc::Sender<Result<(), BackendError>>,
) {
    // Déclaré en premier : détruit en dernier, après toutes les interfaces COM.
    let _apartment = match ComApartment::initialize_mta() {
        Ok(apartment) => apartment,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: COM est initialisé sur ce fil ; `MMDeviceEnumerator` est le CLSID
    // documenté de l'énumérateur, sans agrégation.
    let enumerator: IMMDeviceEnumerator =
        match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(enumerator) => enumerator,
            Err(e) => {
                let _ = ready.send(Err(platform_error(
                    "CoCreateInstance(MMDeviceEnumerator)",
                    &e,
                )));
                return;
            }
        };
    let client = NotificationClient::create(sender);
    // SAFETY: énumérateur et client valides ; l'enregistrement est apparié au
    // `UnregisterEndpointNotificationCallback` de la fin du fil.
    if let Err(e) = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) } {
        let _ = ready.send(Err(platform_error(
            "IMMDeviceEnumerator::RegisterEndpointNotificationCallback",
            &e,
        )));
        return;
    }

    let mut state = State {
        enumerator,
        client,
        events: EventBroadcaster::default(),
        known: BTreeSet::new(),
    };
    match enumerate(&state.enumerator) {
        Ok(devices) => state.remember(&devices),
        Err(e) => {
            let _ = ready.send(Err(e));
            state.unregister();
            return;
        }
    }
    let _ = ready.send(Ok(()));

    while let Ok(message) = receiver.recv() {
        match message {
            Message::Command(Command::Shutdown) => break,
            Message::Command(command) => state.handle_command(command),
            Message::Notification(notification) => state.handle_notification(notification),
        }
    }
    state.unregister();
}

/// Tout ce que le fil possède, une fois démarré.
struct State {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
    events: EventBroadcaster,
    /// Endpoints actifs déjà annoncés (ou présents à la dernière énumération) : sert
    /// à n'émettre `Added` et `Removed` qu'une fois chacun, quel que soit l'ordre
    /// des rappels de Windows (`OnDeviceAdded` puis `OnDeviceStateChanged`, ou
    /// l'inverse).
    known: BTreeSet<DeviceId>,
}

impl State {
    fn remember(&mut self, devices: &[EndpointInfo]) {
        self.known = devices.iter().map(|d| d.info.id.clone()).collect();
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Enumerate { reply } => {
                let result = enumerate(&self.enumerator);
                if let Ok(devices) = &result {
                    self.remember(devices);
                }
                let _ = reply.send(result);
            }
            Command::DefaultDevice { direction, reply } => {
                let result = default_endpoint_id(&self.enumerator, direction)
                    .map(|id| id.map(DeviceId::new));
                let _ = reply.send(result);
            }
            Command::Open {
                id,
                format,
                policy,
                period,
                loopback,
                reply,
            } => {
                let _ = reply.send(crate::open::open(
                    &self.enumerator,
                    &id,
                    format,
                    policy,
                    period,
                    loopback,
                ));
            }
            Command::Subscribe { reply } => {
                let _ = reply.send(self.events.subscribe());
            }
            Command::Shutdown => {}
        }
    }

    fn handle_notification(&mut self, notification: Notification) {
        match notification {
            Notification::Added { id } => self.endpoint_appeared(id),
            Notification::StateChanged { id, state } if state == DEVICE_STATE_ACTIVE => {
                self.endpoint_appeared(id);
            }
            Notification::Removed { id } | Notification::StateChanged { id, .. } => {
                self.endpoint_vanished(id);
            }
            Notification::DefaultChanged { flow, role, id } => {
                // Seul le rôle `eConsole` définit « le périphérique par défaut » de
                // Conduit ; `eMultimedia` et `eCommunications` le suivent presque
                // toujours et produiraient des doublons.
                if role != eConsole {
                    return;
                }
                let Some(direction) = direction_from_flow(flow) else {
                    return;
                };
                self.events.send(DeviceEvent::DefaultChanged {
                    direction,
                    id: id.map(DeviceId::new),
                });
            }
        }
    }

    /// Un endpoint est (peut-être) devenu actif : le décrire et l'annoncer s'il est
    /// nouveau. Un endpoint indescriptible (retiré entre-temps, propriété illisible)
    /// est ignoré : il n'y a personne à qui signaler l'erreur depuis ce fil.
    fn endpoint_appeared(&mut self, id: String) {
        let device_id = DeviceId::new(id.as_str());
        if self.known.contains(&device_id) {
            return;
        }
        if let Ok(Some(endpoint)) = describe_id(&self.enumerator, &id) {
            self.known.insert(endpoint.info.id.clone());
            self.events.send(DeviceEvent::Added(endpoint.info));
        }
    }

    /// Un endpoint n'est plus actif : l'annoncer si on le connaissait.
    fn endpoint_vanished(&mut self, id: String) {
        let device_id = DeviceId::new(id);
        if self.known.remove(&device_id) {
            self.events.send(DeviceEvent::Removed { id: device_id });
        }
    }

    /// Désenregistre le client : plus aucun rappel après le retour.
    fn unregister(&self) {
        // SAFETY: apparié au `RegisterEndpointNotificationCallback` réussi de `run`,
        // avec le même client ; les deux interfaces sont encore valides.
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.client)
        };
    }
}
