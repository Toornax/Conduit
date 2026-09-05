//! Le `IMMNotificationClient` : rappels COM → messages pour le fil MMDevice.
//!
//! Windows appelle ces méthodes depuis un fil de son choix, pendant que
//! l'énumérateur peut être en train de servir une commande. Les rappels ne font donc
//! **rien d'autre que copier leurs arguments et poster** une [`Notification`] : le
//! fil MMDevice décide seul de ce qu'il en fait (voir `mmdevice_thread`).

use std::sync::mpsc;

use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl, DEVICE_STATE,
};

use crate::mmdevice_thread::Message;

/// Rappel reçu, tel quel, sans interprétation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notification {
    /// `OnDeviceStateChanged`.
    StateChanged {
        /// Identifiant d'endpoint.
        id: String,
        /// Nouvel état (`DEVICE_STATE_ACTIVE`, …).
        state: DEVICE_STATE,
    },
    /// `OnDeviceAdded`.
    Added {
        /// Identifiant d'endpoint.
        id: String,
    },
    /// `OnDeviceRemoved`.
    Removed {
        /// Identifiant d'endpoint.
        id: String,
    },
    /// `OnDefaultDeviceChanged`.
    DefaultChanged {
        /// Sens.
        flow: EDataFlow,
        /// Rôle (`eConsole`, `eMultimedia`, `eCommunications`).
        role: ERole,
        /// Nouvel endpoint par défaut, `None` s'il n'y en a plus.
        id: Option<String>,
    },
}

/// Copie une chaîne d'identifiant passée par COM ; `None` si elle est nulle.
///
/// # Safety
///
/// `id` est nul ou pointe une chaîne UTF-16 terminée par NUL valide pendant l'appel.
unsafe fn id_string(id: &PCWSTR) -> Option<String> {
    if id.is_null() {
        return None;
    }
    // SAFETY: garantie de l'appelant, reprise du contrat COM du rappel.
    unsafe { id.to_string() }.ok()
}

/// Objet COM enregistré auprès de l'`IMMDeviceEnumerator`.
#[implement(IMMNotificationClient)]
pub(crate) struct NotificationClient {
    sender: mpsc::Sender<Message>,
}

impl NotificationClient {
    /// Construit l'objet et le convertit en interface COM.
    pub(crate) fn create(sender: mpsc::Sender<Message>) -> IMMNotificationClient {
        Self { sender }.into()
    }

    fn post(&self, notification: Notification) {
        // Si le fil MMDevice est parti, plus personne n'écoute : rien à faire.
        let _ = self.sender.send(Message::Notification(notification));
    }
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, state: DEVICE_STATE) -> windows::core::Result<()> {
        // SAFETY: contrat COM de `IMMNotificationClient::OnDeviceStateChanged` :
        // `pwstrDeviceId` est une chaîne terminée par NUL valide pendant l'appel.
        if let Some(id) = unsafe { id_string(id) } {
            self.post(Notification::StateChanged { id, state });
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
        // SAFETY: contrat COM de `OnDeviceAdded`, comme ci-dessus.
        if let Some(id) = unsafe { id_string(id) } {
            self.post(Notification::Added { id });
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        // SAFETY: contrat COM de `OnDeviceRemoved`, comme ci-dessus.
        if let Some(id) = unsafe { id_string(id) } {
            self.post(Notification::Removed { id });
        }
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        id: &PCWSTR,
    ) -> windows::core::Result<()> {
        // SAFETY: contrat COM de `OnDefaultDeviceChanged` : la chaîne est nulle
        // quand il n'y a plus de périphérique par défaut, valide sinon.
        let id = unsafe { id_string(id) };
        self.post(Notification::DefaultChanged { flow, role, id });
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        // Ignoré : un changement de nom ou de format sera vu à la prochaine
        // énumération.
        Ok(())
    }
}
