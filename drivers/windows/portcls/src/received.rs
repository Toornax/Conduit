//! Enveloppes fines des interfaces **reçues** de PortCls (`IResourceList`,
//! `IPortTopology`, `IPortWaveRT`, `IPortWaveRTStream`, `IRegistryKey`) : une [`ComRef`]
//! et des méthodes sûres qui appellent les slots de la vtable
//! (`(*vtbl).Slot.ok_or(STATUS_NOT_IMPLEMENTED)?`).
//!
//! Chaque enveloppe possède sa référence (`AddRef` à la construction par le thunk qui la
//! reçoit, `Release` au `Drop`) ; `Clone` prend une référence de plus. Les méthodes qui
//! n'ont pas encore d'enveloppe sûre (`IPort::NewRegistryKey`, avec ses
//! `OBJECT_ATTRIBUTES` ; `IRegistryKey::*`) restent accessibles par
//! [`com_ref`](ResourceList::com_ref) puis la vtable : elles arriveront avec les tâches
//! qui en ont besoin (M1b-01 pour le registre). `IPort::Init` est enveloppé côté
//! adaptateur ([`crate::adapter::port_init`]) : c'est le pilote qui l'appelle sur un port
//! fraîchement créé par `PcNewPort`, jamais un miniport sur le port reçu.

use core::ffi::c_void;

use conduit_com::{
    ComRef, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED,
};
use portcls_sys::{
    CM_RESOURCE_TYPE, DEVICE_REGISTRY_PROPERTY, IPortTopology, IPortWaveRT, IPortWaveRTStream,
    IRegistryKey, IResourceList, MEMORY_CACHING_TYPE, PHYSICAL_ADDRESS, PMDL, SIZE_T, ULONG,
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

/// Méthodes `IPort` communes aux ports reçus (`IPortTopology`, `IPortWaveRT` : mêmes six
/// slots, aucune méthode propre).
macro_rules! methodes_iport {
    ($nom:ident) => {
        impl $nom {
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
                // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable
                // a la forme attendue (contrat `ComInterface`) ; le slot est non nul ;
                // `buffer` est inscriptible sur `len` octets et `written` est une variable
                // locale.
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
methodes_iport!(PortTopology);

enveloppe! {
    /// `IPortWaveRT` : le port WaveRT que PortCls remet au miniport dans
    /// `IMiniportWaveRT::Init` (méthodes `IPort` seulement : `IPortWaveRT` n'en ajoute
    /// aucune).
    PortWaveRT(IPortWaveRT)
}
methodes_iport!(PortWaveRT);

enveloppe! {
    /// `IPortWaveRTStream` : l'objet d'aide que le port WaveRT remet à
    /// `IMiniportWaveRT::NewStream` pour chaque flux ; ses méthodes allouent, mappent et
    /// libèrent les pages du tampon cyclique (driver-design.md §5.2 : un tampon par flux
    /// par `AllocatePagesForMdl`, libéré à `FreeAudioBuffer`). Les `MmXxx` du noyau
    /// **ne remplacent pas** ces méthodes : PortCls doit connaître les MDL qu'il mappera
    /// dans l'espace du client.
    ///
    /// Toutes les méthodes sont à `PASSIVE_LEVEL` (contexte des propriétés
    /// `KSPROPERTY_RTAUDIO_BUFFER*` que PortCls traite en appelant le miniport).
    PortWaveRTStream(IPortWaveRTStream)
}

impl PortWaveRTStream {
    /// `AllocatePagesForMdl` : alloue des pages non paginées, physiquement quelconques,
    /// d'adresse inférieure ou égale à `high_address` (voir
    /// [`physical_address`]), et renvoie la MDL qui les décrit. `total_bytes` est arrondi
    /// à la page par le noyau ; si toute la mémoire n'est pas disponible, la MDL peut
    /// décrire **moins** de pages que demandé (`physical_pages_count` le dit).
    ///
    /// Erreurs : `STATUS_INSUFFICIENT_RESOURCES` si PortCls renvoie une MDL nulle,
    /// `STATUS_NOT_IMPLEMENTED` si le slot est vide (faux port des tests).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn allocate_pages_for_mdl(
        &self,
        high_address: PHYSICAL_ADDRESS,
        total_bytes: usize,
    ) -> Result<PMDL, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .AllocatePagesForMdl
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        let total = SIZE_T::try_from(total_bytes).map_err(|_| STATUS_INVALID_PARAMETER)?;
        // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable est un
        // `IPortWaveRTStreamVtbl` (contrat `ComInterface`) ; le slot est non nul ; les
        // arguments sont des valeurs.
        let mdl = unsafe { slot(self.0.as_raw(), high_address, total) };
        if mdl.is_null() {
            Err(STATUS_INSUFFICIENT_RESOURCES)
        } else {
            Ok(mdl)
        }
    }

    /// `AllocateContiguousPagesForMdl` : comme
    /// [`allocate_pages_for_mdl`](Self::allocate_pages_for_mdl) mais les pages sont
    /// physiquement contiguës, dans `[low_address ; high_address]`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn allocate_contiguous_pages_for_mdl(
        &self,
        low_address: PHYSICAL_ADDRESS,
        high_address: PHYSICAL_ADDRESS,
        total_bytes: usize,
    ) -> Result<PMDL, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .AllocateContiguousPagesForMdl
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        let total = SIZE_T::try_from(total_bytes).map_err(|_| STATUS_INVALID_PARAMETER)?;
        // SAFETY: voir `allocate_pages_for_mdl`.
        let mdl = unsafe { slot(self.0.as_raw(), low_address, high_address, total) };
        if mdl.is_null() {
            Err(STATUS_INSUFFICIENT_RESOURCES)
        } else {
            Ok(mdl)
        }
    }

    /// `MapAllocatedPages` : mappe les pages de `mdl` en un bloc virtuel contigu visible
    /// du noyau (adresse de base renvoyée), avec le type de cache `cache_type`. À
    /// défaire par [`unmap_allocated_pages`](Self::unmap_allocated_pages) avant
    /// [`free_pages_from_mdl`](Self::free_pages_from_mdl).
    ///
    /// Erreurs : `STATUS_INSUFFICIENT_RESOURCES` si le mappage échoue (adresse nulle).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `mdl` a été renvoyée par [`allocate_pages_for_mdl`](Self::allocate_pages_for_mdl)
    /// ou [`allocate_contiguous_pages_for_mdl`](Self::allocate_contiguous_pages_for_mdl)
    /// de **ce** flux et n'a pas encore été libérée.
    pub unsafe fn map_allocated_pages(
        &self,
        mdl: PMDL,
        cache_type: MEMORY_CACHING_TYPE,
    ) -> Result<*mut c_void, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .MapAllocatedPages
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: objet vivant et slot non nul (voir `allocate_pages_for_mdl`) ; `mdl`
        // est une MDL vivante de ce flux (contrat de la fonction).
        let base = unsafe { slot(self.0.as_raw(), mdl, cache_type) };
        if base.is_null() {
            Err(STATUS_INSUFFICIENT_RESOURCES)
        } else {
            Ok(base)
        }
    }

    /// `UnmapAllocatedPages` : défait le mappage `base` obtenu par
    /// [`map_allocated_pages`](Self::map_allocated_pages).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `base` a été renvoyé par `map_allocated_pages(mdl, …)` de ce flux et n'a pas encore
    /// été démappé ; plus aucune référence Rust au bloc mappé ne subsiste.
    pub unsafe fn unmap_allocated_pages(
        &self,
        base: *mut c_void,
        mdl: PMDL,
    ) -> Result<(), NtStatus> {
        let slot = self
            .0
            .vtbl()
            .UnmapAllocatedPages
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: objet vivant et slot non nul ; `base`/`mdl` forment un mappage vivant de
        // ce flux (contrat de la fonction).
        unsafe { slot(self.0.as_raw(), base, mdl) };
        Ok(())
    }

    /// `FreePagesFromMdl` : libère les pages et la MDL. Appelé par
    /// `IMiniportWaveRTStream::FreeAudioBuffer` (driver-design.md §5.2).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `mdl` a été renvoyée par une méthode d'allocation de ce flux, tout mappage en a été
    /// défait, et elle n'est plus utilisée après l'appel.
    pub unsafe fn free_pages_from_mdl(&self, mdl: PMDL) -> Result<(), NtStatus> {
        let slot = self
            .0
            .vtbl()
            .FreePagesFromMdl
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: objet vivant et slot non nul ; `mdl` est une MDL vivante de ce flux que
        // l'appelant cède (contrat de la fonction).
        unsafe { slot(self.0.as_raw(), mdl) };
        Ok(())
    }

    /// `GetPhysicalPagesCount` : nombre de pages physiques décrites par `mdl` (à comparer
    /// à la demande : l'allocation peut être partielle).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `mdl` est une MDL vivante de ce flux.
    pub unsafe fn physical_pages_count(&self, mdl: PMDL) -> Result<u32, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .GetPhysicalPagesCount
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: objet vivant et slot non nul ; `mdl` vivante (contrat de la fonction).
        Ok(unsafe { slot(self.0.as_raw(), mdl) })
    }

    /// `GetPhysicalPageAddress` : adresse physique de la page `index` de `mdl`
    /// (`index < physical_pages_count`).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `mdl` est une MDL vivante de ce flux.
    pub unsafe fn physical_page_address(
        &self,
        mdl: PMDL,
        index: u32,
    ) -> Result<PHYSICAL_ADDRESS, NtStatus> {
        let slot = self
            .0
            .vtbl()
            .GetPhysicalPageAddress
            .ok_or(STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: objet vivant et slot non nul ; `mdl` vivante (contrat de la fonction).
        Ok(unsafe { slot(self.0.as_raw(), mdl, index) })
    }
}

/// `PHYSICAL_ADDRESS` (`LARGE_INTEGER`) depuis sa valeur 64 bits, pour les bornes
/// d'allocation de [`PortWaveRTStream`] (`i64::MAX`, ou `u32::MAX as i64` pour rester
/// sous 4 Gio quand `Dma32BitAddresses` est annoncé).
pub const fn physical_address(quad_part: i64) -> PHYSICAL_ADDRESS {
    PHYSICAL_ADDRESS {
        QuadPart: quad_part,
    }
}

enveloppe! {
    /// `IRegistryKey` : clé de registre ouverte par `IPort::NewRegistryKey` ou
    /// `PcNewRegistryKey`. Aucune méthode sûre encore (M1b-01) : `com_ref()` puis la
    /// vtable.
    RegistryKey(IRegistryKey)
}
