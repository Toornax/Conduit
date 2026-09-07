//! Journal d'événements système (M1b-01) : `IoAllocateErrorLogEntry` /
//! `IoWriteErrorLogEntry`.
//!
//! # Pourquoi pas `kmd_log!`
//!
//! [`crate::log`] est **vide en release** (`#[cfg(debug_assertions)]`) et n'atteint de
//! toute façon qu'un débogueur noyau attaché. Le critère de M1b-01 est « valeurs hors
//! bornes → valeurs par défaut et **journal d'événements** » : il demande une trace que
//! l'administrateur d'un poste retrouve après coup, sur un pilote signé, sans débogueur.
//! Seul le journal système la fournit. Les deux coexistent : `kmd_log!` raconte tout le
//! déroulement à qui débogue, ce module ne consigne que les anomalies.
//!
//! # Ce que l'Observateur d'événements affichera vraiment
//!
//! **Une description générique, pas notre phrase.** C'est une limite connue et assumée,
//! pas un oubli.
//!
//! La documentation (« Writing to the System Event Log ») décrit la chaîne complète :
//! `ErrorCode` est un `NTSTATUS` auquel est associé un *texte de message* ; l'Observateur
//! affiche ce texte, y remplace `%1` par le nom du périphérique et `%2`, `%3`… par les
//! *chaînes d'insertion* que le paquet transporte. Mais ce texte ne vient pas du paquet :
//! il vient d'un **fichier de messages**, que le service doit avoir déclaré au préalable
//! (« Registering as a Source of Error Messages ») par deux valeurs sous
//! `HKLM\SYSTEM\CurrentControlSet\Services\EventLog\System\<pilote>` — `EventMessageFile`
//! (`iologmsg.dll` pour les codes prédéfinis `IO_ERR_*`) et `TypesSupported`.
//!
//! Conduit **ne les écrit pas**, et c'est délibéré : ces valeurs vivent sous `Services`,
//! hors de la clé matérielle du périphérique, donc hors de portée des `HKR` que PnP
//! retire à la désinstallation (F-52) ; les inscrire demanderait un `AddReg` `HKLM` dans
//! l'INF, et fournir notre propre texte demanderait de fabriquer une ressource de
//! messages dans le binaire du pilote. C'est une tâche à part entière, pas un détail de
//! M1b-01.
//!
//! Conséquence, **d'après la documentation citée ci-dessus et non mesurée** (aucune VM
//! n'a été lancée pour cette tâche) : l'entrée apparaît bien dans le journal *Système*,
//! datée, avec sa source et son périphérique, mais sa description est le texte de repli
//! de l'Observateur (« La description de l'ID d'événement … est introuvable »), suivi des
//! chaînes d'insertion brutes qu'il recopie faute de mieux. C'est donc **la chaîne
//! d'insertion qui porte l'information**, et elle seule — d'où le soin mis à la rendre
//! lisible telle quelle, préfixe « Conduit : » compris, plutôt qu'à empiler des
//! `DumpData` binaires que personne ne décodera. Elle se lit sans ambiguïté par :
//!
//! ```text
//! Get-WinEvent -LogName System -MaxEvents 200 |
//!   Where-Object ProviderName -eq conduit_kmd |
//!   Select-Object TimeCreated, Id, @{n='Texte';e={$_.Properties[-1].Value}}
//! ```
//!
//! # Sévérité
//!
//! `IO_ERR_CONFIGURATION_ERROR` (`ntiologc.h`) porte le texte « Driver or device is
//! incorrectly configured for %1 », qui décrit exactement la situation. Sa sévérité est
//! « erreur » (`0xC…`), alors que le pilote, lui, s'en remet et charge : l'entrée
//! s'affichera donc en erreur pour un incident dont on a récupéré. C'est le prix des
//! codes prédéfinis — aucun `IO_WARNING_*` de `ntiologc.h` ne parle de configuration — et
//! le champ `FinalStatus`, laissé à `STATUS_SUCCESS`, enregistre la récupération.
//!
//! # Contraintes
//!
//! Aucune allocation de notre part (le paquet vient du noyau et `IoWriteErrorLogEntry` le
//! libère), aucune panique, message formaté dans un tampon de pile. Un échec d'allocation
//! est ignoré : la documentation l'exige — « Drivers must not treat
//! **IoAllocateErrorLogEntry** returning **NULL** as a fatal error ».
//!
//! IRQL : `IoAllocateErrorLogEntry` et `IoWriteErrorLogEntry` s'appellent jusqu'à
//! `DISPATCH_LEVEL` ; nos appels ont lieu à `PASSIVE_LEVEL` (`StartDevice`).

use core::fmt::{self, Write as _};

use portcls_sys::PDEVICE_OBJECT;
use wdk_sys::ntddk::{IoAllocateErrorLogEntry, IoWriteErrorLogEntry};
use wdk_sys::{
    IO_ERROR_LOG_MESSAGE, IO_ERROR_LOG_PACKET, LARGE_INTEGER, NTSTATUS, STATUS_SUCCESS, USHORT,
    WCHAR,
};

/// `IO_ERR_CONFIGURATION_ERROR` (`ntiologc.h`) : « Driver or device is incorrectly
/// configured for %1 ».
///
/// `ntiologc.h` est dans `shared\` et n'est dans l'allowlist d'aucun de nos deux jeux de
/// bindings : la valeur est recopiée, avec le nom qui permet de la retrouver. Voir la
/// note de sévérité en tête de module.
const IO_ERR_CONFIGURATION_ERROR: NTSTATUS = 0xC004_0003_u32 as NTSTATUS;

/// Taille de l'en-tête du paquet, et donc décalage de la première chaîne d'insertion.
///
/// `DumpDataSize` est nul : les chaînes commencent juste après le paquet. Prendre
/// `size_of` plutôt que le décalage de `DumpData` fait perdre les quelques octets de
/// remplissage de fin de structure, mais garantit `entry_size >= size_of(paquet)`,
/// c'est-à-dire que l'écriture du paquet ne déborde jamais de l'allocation.
const PACKET_SIZE: usize = size_of::<IO_ERROR_LOG_PACKET>();

/// `PORT_MAXIMUM_MESSAGE_LENGTH` (`wdm.h`), qui dépend de `_WIN64`.
const PORT_MAXIMUM_MESSAGE_LENGTH: usize = if size_of::<usize>() == 8 { 512 } else { 256 };

/// `ERROR_LOG_LIMIT_SIZE` (`wdm.h`) : `256 - 16`.
const ERROR_LOG_LIMIT_SIZE: usize = 240;

/// `IO_ERROR_LOG_MESSAGE_HEADER_LENGTH` (`wdm.h`) : ce que le message LPC ajoute autour
/// du paquet, plus quarante caractères de nom de pilote.
const IO_ERROR_LOG_MESSAGE_HEADER_LENGTH: usize = size_of::<IO_ERROR_LOG_MESSAGE>()
    .saturating_sub(PACKET_SIZE)
    .saturating_add(size_of::<WCHAR>().saturating_mul(40));

/// `ERROR_LOG_MESSAGE_LIMIT_SIZE` (`wdm.h`).
const ERROR_LOG_MESSAGE_LIMIT_SIZE: usize =
    ERROR_LOG_LIMIT_SIZE.saturating_add(IO_ERROR_LOG_MESSAGE_HEADER_LENGTH);

/// `IO_ERROR_LOG_MESSAGE_LENGTH` (`wdm.h`) : le plus petit des deux plafonds.
const IO_ERROR_LOG_MESSAGE_LENGTH: usize =
    if PORT_MAXIMUM_MESSAGE_LENGTH > ERROR_LOG_MESSAGE_LIMIT_SIZE {
        ERROR_LOG_MESSAGE_LIMIT_SIZE
    } else {
        PORT_MAXIMUM_MESSAGE_LENGTH
    };

/// `ERROR_LOG_MAXIMUM_SIZE` (`wdm.h`) : plafond de l'`EntrySize` d'un paquet.
///
/// La chaîne de `#define` est recopiée en constantes Rust plutôt que devinée, parce que
/// la documentation insiste : `EntrySize` est un `UCHAR`, « if you specify a larger
/// value, the compiler will silently truncate that value to a (wrong) UCHAR », et la
/// routine « cannot reliably detect if the passed value is too large ». Une taille
/// dépassée ne se voit donc pas — d'où les assertions à la compilation plus bas.
const ERROR_LOG_MAXIMUM_SIZE: usize =
    IO_ERROR_LOG_MESSAGE_LENGTH.saturating_sub(IO_ERROR_LOG_MESSAGE_HEADER_LENGTH);

/// Décalage de la chaîne d'insertion dans le paquet, tel que `StringOffset` l'attend.
const STRING_OFFSET: USHORT = PACKET_SIZE as USHORT;

/// Unités UTF-16 du message, **terminateur nul compris** : tout ce qui reste sous le
/// plafond une fois le paquet placé.
const MESSAGE_UNITS: usize = ERROR_LOG_MAXIMUM_SIZE
    .saturating_sub(PACKET_SIZE)
    .saturating_div(size_of::<WCHAR>());

/// Marqueur ajouté quand le message n'a pas tenu.
const ELLIPSIS: &str = "...";

/// Unités laissées au texte : le reste est réservé au marqueur de troncature et au
/// terminateur nul, qui doivent tenir **même** sur un message tronqué.
const TEXT_UNITS: usize = MESSAGE_UNITS.saturating_sub(ELLIPSIS.len().saturating_add(1));

// Le paquet et son message tiennent sous le plafond, qui tient lui-même dans le `UCHAR`
// d'`EntrySize` ; le décalage de chaîne survit à sa conversion en `USHORT` ; et il reste
// de la place pour une ligne utile.
const _: () = assert!(ERROR_LOG_MAXIMUM_SIZE <= u8::MAX as usize);
const _: () = assert!(PACKET_SIZE < ERROR_LOG_MAXIMUM_SIZE);
const _: () = assert!(STRING_OFFSET as usize == PACKET_SIZE);
const _: () = assert!(
    PACKET_SIZE.saturating_add(MESSAGE_UNITS.saturating_mul(size_of::<WCHAR>()))
        <= ERROR_LOG_MAXIMUM_SIZE
);
const _: () = assert!(TEXT_UNITS >= 64, "message trop court pour une correction");

/// Tampon de pile où le message est encodé en UTF-16, puis terminé par un NUL.
struct Utf16Writer {
    units: [WCHAR; MESSAGE_UNITS],
    /// Unités utiles écrites, toujours ≤ [`TEXT_UNITS`] avant [`Utf16Writer::finish`].
    used: usize,
    /// Vrai dès qu'un caractère n'est pas entré : plus rien n'est ajouté ensuite.
    truncated: bool,
}

impl fmt::Write for Utf16Writer {
    /// Encode ce qui tient encore, **caractère par caractère**.
    ///
    /// Le découpage se fait sur des points de code entiers : une paire de substitution
    /// (deux unités UTF-16) est ajoutée en entier ou pas du tout, jamais coupée en deux,
    /// ce qui laisserait une unité isolée et une chaîne UTF-16 mal formée dans le
    /// journal. Passé une troncature, plus rien n'est ajouté : la fin du message est
    /// perdue, jamais recollée à ce qui la suivait.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for caractere in s.chars() {
            if self.truncated {
                break;
            }
            let mut paire = [0; 2];
            let encode = caractere.encode_utf16(&mut paire);
            if self.used.saturating_add(encode.len()) > TEXT_UNITS {
                self.truncated = true;
                break;
            }
            for unite in encode.iter() {
                self.push(*unite);
            }
        }
        Ok(())
    }
}

impl Utf16Writer {
    /// Tampon vide.
    const fn new() -> Self {
        Self {
            units: [0; MESSAGE_UNITS],
            used: 0,
            truncated: false,
        }
    }

    /// Ajoute une unité ; une position hors du tampon rend `None` et l'ajout est
    /// simplement abandonné (l'indexation directe est interdite par les lints).
    fn push(&mut self, unite: WCHAR) {
        if let Some(slot) = self.units.get_mut(self.used) {
            *slot = unite;
            self.used = self.used.saturating_add(1);
        }
    }

    /// Termine la chaîne par un NUL et rend les unités à copier dans le paquet.
    ///
    /// Le résultat compte toujours au moins une unité (le NUL) et au plus
    /// [`MESSAGE_UNITS`] : [`TEXT_UNITS`] a réservé la place du marqueur et du NUL.
    fn finish(&mut self) -> &[WCHAR] {
        if self.truncated {
            for unite in ELLIPSIS.encode_utf16() {
                self.push(unite);
            }
        }
        self.push(0);
        self.units.get(..self.used).unwrap_or(&[])
    }
}

/// Destination des entrées de journal : l'objet de périphérique de l'adaptateur.
///
/// L'invariant de validité du pointeur est établi une seule fois, à la construction,
/// plutôt qu'à chacun des appels à [`EventLog::report`] — qui restent donc sûrs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EventLog(PDEVICE_OBJECT);

impl EventLog {
    /// # Safety
    ///
    /// `device` est un objet de périphérique vivant de ce pilote — celui que PortCls
    /// remet à `StartDevice` — valide au moins aussi longtemps que l'`EventLog` rendu.
    pub(crate) const unsafe fn new(device: PDEVICE_OBJECT) -> Self {
        Self(device)
    }

    /// Consigne une anomalie dans le journal d'événements *Système*.
    ///
    /// `unique` part dans `UniqueErrorValue` : il désigne le point d'appel, seule
    /// information exploitable si le texte n'arrivait pas (voir la note en tête de
    /// module). Un échec d'allocation n'interrompt rien.
    ///
    /// Hors ligne à dessein : le tampon de message n'existe que dans ce cadre de pile, et
    /// non dans celui de chacun de ses appelants.
    ///
    /// IRQL : ≤ `DISPATCH_LEVEL`.
    #[inline(never)]
    pub(crate) fn report(self, unique: u32, args: fmt::Arguments<'_>) {
        let mut writer = Utf16Writer::new();
        // Préfixe : dans l'Observateur, la chaîne d'insertion apparaît seule, sans le
        // texte de message qui l'aurait introduite. Elle doit se suffire.
        let _ = writer.write_str("Conduit : ");
        // `write_str` ne rend jamais d'erreur : la troncature est un état du tampon, pas
        // un échec, et une trace perdue ne doit rien interrompre.
        let _ = fmt::write(&mut writer, args);
        let message = writer.finish();

        let string_bytes = message.len().saturating_mul(size_of::<WCHAR>());
        let entry_size = PACKET_SIZE.saturating_add(string_bytes);
        // Les assertions à la compilation garantissent la conversion ; le journal n'est
        // jamais un motif d'échec, d'où l'abandon silencieux du cas impossible.
        let Ok(entry_size) = u8::try_from(entry_size) else {
            kmd_log!("journal d'événements : taille de paquet impossible ({entry_size})");
            return;
        };

        // SAFETY: `self.0` est un objet de périphérique valide (contrat de `new`) ;
        // `entry_size` est borné par `ERROR_LOG_MAXIMUM_SIZE` (assertions ci-dessus).
        let entry = unsafe { IoAllocateErrorLogEntry(self.0.cast(), entry_size) };
        if entry.is_null() {
            // Documenté comme non fatal : le pilote continue sans sa trace.
            kmd_log!("journal d'événements : allocation refusée, message perdu");
            return;
        }

        let packet = IO_ERROR_LOG_PACKET {
            // Le paquet ne décrit pas un IRP : ni fonction majeure, ni code de contrôle,
            // ni décalage, ni tentative.
            MajorFunctionCode: 0,
            RetryCount: 0,
            IoControlCode: 0,
            DeviceOffset: LARGE_INTEGER::default(),
            // Numéroté par le système, catégorie inutilisée (pas de fichier de messages).
            SequenceNumber: 0,
            EventCategory: 0,
            // Toute l'information est dans la chaîne d'insertion (voir en tête de
            // module) : aucune donnée binaire, `DumpData` reste un remplissage.
            DumpDataSize: 0,
            DumpData: [0],
            NumberOfStrings: 1,
            StringOffset: STRING_OFFSET,
            ErrorCode: IO_ERR_CONFIGURATION_ERROR,
            UniqueErrorValue: unique,
            // Le pilote s'en est remis et charge : la récupération est enregistrée ici.
            FinalStatus: STATUS_SUCCESS,
        };

        // SAFETY: `IoAllocateErrorLogEntry` a rendu un bloc de `entry_size` octets, aligné
        // pour un `IO_ERROR_LOG_PACKET` (allocation de pool) ; `entry_size` vaut
        // `PACKET_SIZE + string_bytes`, donc l'écriture du paquet puis celle des
        // `message.len()` unités à `PACKET_SIZE` restent dans le bloc, sans recouvrement.
        // `PACKET_SIZE` est un multiple de l'alignement d'un `WCHAR`.
        unsafe {
            entry.cast::<IO_ERROR_LOG_PACKET>().write(packet);
            core::ptr::copy_nonoverlapping(
                message.as_ptr(),
                entry.cast::<u8>().add(PACKET_SIZE).cast::<WCHAR>(),
                message.len(),
            );
        }

        // SAFETY: `entry` est le paquet que l'on vient de remplir en entier ;
        // `IoWriteErrorLogEntry` en prend la propriété et libère le bloc.
        unsafe { IoWriteErrorLogEntry(entry) };
    }
}

/// Consigne une anomalie dans le journal système (syntaxe de `format!`), sous le
/// `UniqueErrorValue` donné.
macro_rules! kmd_event {
    ($log:expr, $unique:expr, $($arg:tt)*) => {
        $crate::eventlog::EventLog::report($log, $unique, ::core::format_args!($($arg)*))
    };
}

pub(crate) use kmd_event;
