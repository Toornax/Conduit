//! Enveloppes fines des interfaces **reçues** de PortCls (`IResourceList`,
//! `IPortTopology`, `IRegistryKey`) : une [`ComRef`] et des méthodes sûres qui appellent
//! les slots de la vtable (`(*vtbl).Slot.ok_or(STATUS_NOT_IMPLEMENTED)?`).
//!
//! Chaque enveloppe possède sa référence (`AddRef` à la construction par le thunk qui la
//! reçoit, `Release` au `Drop`) ; `Clone` prend une référence de plus. Les méthodes qui
//! n'ont pas encore d'enveloppe sûre (`IPort::NewRegistryKey`, avec ses
//! `OBJECT_ATTRIBUTES` ; `IRegistryKey::*`) restent accessibles par
//! [`com_ref`](ResourceList::com_ref) puis la vtable : elles arriveront avec les tâches
//! qui en ont besoin (M1b-01 pour le registre).

use conduit_com::{ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED};
use portcls_sys::{
    CM_RESOURCE_TYPE, DEVICE_REGISTRY_PROPERTY, IPortTopology, IRegistryKey, IResourceList, ULONG,
};

/// Déclare une enveloppe `struct $nom(ComRef<$iface>)` avec ses conversions.
macro_rules! enveloppe {
    ($(#[$doc:meta])* $nom:ident($iface:ident)) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        pub struct $nom(ComRef<$iface>);

        impl $nom {
            /// Enveloppe une référence déjà possédée.
            pub fn from_ref(r: ComRef<$iface>) -> Self {
                Self(r)
            }

            /// La référence sous-jacente (pour appeler un slot sans enveloppe sûre).
            pub fn com_ref(&self) -> &ComRef<$iface> {
                &self.0
            }

            /// Rend la référence sous-jacente.
            pub fn into_ref(self) -> ComRef<$iface> {
                self.0
            }
        }

        impl From<ComRef<$iface>> for $nom {
            fn from(r: ComRef<$iface>) -> Self {
                Self(r)
            }
        }
    };
}

enveloppe! {
    /// `IResourceList` : la liste de ressources matérielles que PortCls remet au
    /// miniport dans `Init` (vide pour un périphérique énuméré à la racine).
    ResourceList(IResourceList)
}

impl ResourceList {
    /// `NumberOfEntries` : nombre d'entrées de la liste.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn count(&self) -> Result<u32, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .NumberOfEntries
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable est un
        // `IResourceListVtbl` (contrat `ComInterface`) ; le slot est non nul.
        Ok(unsafe { slot(self.0.as_raw()) })
    }

    /// `NumberOfEntriesOfType` : nombre d'entrées du type `ty` (`CmResourceTypePort`,
    /// `CmResourceTypeInterrupt`…).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn count_of_type(&self, ty: CM_RESOURCE_TYPE) -> Result<u32, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .NumberOfEntriesOfType
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: voir `count`.
        Ok(unsafe { slot(self.0.as_raw(), ty) })
    }
}

enveloppe! {
    /// `IPortTopology` : le port topologie que PortCls remet au miniport dans `Init`
    /// (méthodes `IPort` seulement : `IPortTopology` n'en ajoute aucune).
    PortTopology(IPortTopology)
}

impl PortTopology {
    /// `IPort::GetDeviceProperty` : lit une propriété PnP de l'adaptateur
    /// (`DevicePropertyFriendlyName`…) dans `buffer` ; renvoie le nombre d'octets
    /// écrits, ou le `NTSTATUS` du noyau (`STATUS_BUFFER_TOO_SMALL` avec la taille
    /// requise perdue : réessayer avec un tampon plus grand).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn get_device_property(
        &self,
        property: DEVICE_REGISTRY_PROPERTY::Type,
        buffer: &mut [u8],
    ) -> Result<u32, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .GetDeviceProperty
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        let len = ULONG::try_from(buffer.len()).map_err(|_| STATUS_INVALID_PARAMETER)?;
        let mut written: ULONG = 0;
        // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable est un
        // `IPortTopologyVtbl` (contrat `ComInterface`) ; le slot est non nul ; `buffer`
        // est inscriptible sur `len` octets et `written` est une variable locale.
        let status = unsafe {
            slot(
                self.0.as_raw(),
                property,
                len,
                buffer.as_mut_ptr().cast(),
                &mut written,
            )
        };
        if conduit_com::nt_success(status) {
            Ok(written)
        } else {
            Err(status)
        }
    }
}

enveloppe! {
    /// `IRegistryKey` : clé de registre ouverte par `IPort::NewRegistryKey` ou
    /// `PcNewRegistryKey`. Aucune méthode sûre encore (M1b-01) : `com_ref()` puis la
    /// vtable.
    RegistryKey(IRegistryKey)
}
