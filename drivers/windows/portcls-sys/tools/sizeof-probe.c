/*
 * Oracle C des bindings portcls-sys (driver-design.md §2.2).
 *
 * Compilé par cl.exe (tools/regen-layout.ps1) sur les MÊMES en-têtes du WDK que
 * wrapper.h, avec les mêmes défines (_AMD64_, AMD64, _WIN64, _KERNEL_MODE,
 * INTERFACE=void avant portcls.h), mais par un second compilateur : ce que clang (via
 * bindgen) et cl.exe s'accordent à dire sur la disposition est tenu pour vrai.
 * Programme en mode utilisateur : il n'appelle rien du noyau, il imprime seulement.
 *
 * Sortie (tests/layout.golden), une ligne par mesure, champs séparés par une tabulation :
 *   sizeof<TAB>Nom<TAB>octets
 *   offset<TAB>Nom.Champ<TAB>octets
 *   guid<TAB>Nom<TAB>XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
 *   devpropkey<TAB>Nom<TAB>XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX,pid
 * Les sizeof des vtables (`IXxxVtbl`) valent nombre de slots × 8.
 */
#define PUT_GUIDS_HERE
#include <initguid.h>
#include <ntddk.h>
#include <windef.h>
#define NOBITMAP
#include <mmreg.h>
#undef NOBITMAP
#include <ks.h>
#include <ksmedia.h>
#include <punknown.h>
#include <drmk.h>
#define INTERFACE void
#include <portcls.h>

/* Pas de <stdio.h> : les en-têtes km/crt et ceux de l'UCRT ne se mélangent pas ;
 * printf est fourni par la bibliothèque C statique (libcmt/libucrt). */
int __cdecl printf(const char *format, ...);

#define TAILLE(type) printf("sizeof\t%s\t%u\n", #type, (unsigned)sizeof(type))
/* Décalage d'un champ : ce que M1b doit connaître pour sérialiser à la main, octet par
 * octet, dans un tampon du mode utilisateur (écrire à travers un pointeur de structure
 * potentiellement désaligné serait un comportement indéfini côté Rust). */
#define DECALAGE(type, champ) \
    printf("offset\t%s.%s\t%u\n", #type, #champ, (unsigned)FIELD_OFFSET(type, champ))

static void guid(const char *nom, const GUID *g)
{
    printf("guid\t%s\t%08lX-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X\n", nom,
           (unsigned long)g->Data1, (unsigned)g->Data2, (unsigned)g->Data3,
           g->Data4[0], g->Data4[1], g->Data4[2], g->Data4[3],
           g->Data4[4], g->Data4[5], g->Data4[6], g->Data4[7]);
}

/* GUID KS : valeurs des macros STATIC_* (source indépendante de la chaîne textuelle de
 * DEFINE_GUIDSTRUCT que build.rs analyse). */
#define GUID_KS(nom) do { static const GUID g_ = { STATIC_##nom }; guid(#nom, &g_); } while (0)
/* GUID PortCls (DEFINE_GUID) : définis dans cette unité par initguid.h. */
#define GUID_PC(nom) guid(#nom, &nom)

/* DEVPROPKEY : GUID **et** pid. Les DEFINE_DEVPROPKEY de ksmedia.h ne produisent une
 * définition que sous INITGUID, que PUT_GUIDS_HERE arme en tête de ce fichier ; côté
 * Rust, ils sont en liste de blocage et recopiés à la main (src/fixups.rs), d'où l'oracle. */
static void devpropkey(const char *nom, const DEVPROPKEY *k)
{
    printf("devpropkey\t%s\t%08lX-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X,%u\n", nom,
           (unsigned long)k->fmtid.Data1, (unsigned)k->fmtid.Data2, (unsigned)k->fmtid.Data3,
           k->fmtid.Data4[0], k->fmtid.Data4[1], k->fmtid.Data4[2], k->fmtid.Data4[3],
           k->fmtid.Data4[4], k->fmtid.Data4[5], k->fmtid.Data4[6], k->fmtid.Data4[7],
           (unsigned)k->pid);
}
#define DEVPROPKEY_(nom) devpropkey(#nom, &nom)

int main(void)
{
    /* Structures KS / PortCls / NT. */
    TAILLE(GUID);
    TAILLE(KSDATAFORMAT);
    TAILLE(KSDATARANGE);
    TAILLE(KSDATARANGE_AUDIO);
    TAILLE(WAVEFORMATEX);
    TAILLE(WAVEFORMATEXTENSIBLE);
    TAILLE(KSDATAFORMAT_WAVEFORMATEX);
    TAILLE(KSDATAFORMAT_WAVEFORMATEXTENSIBLE);
    TAILLE(KSPIN_DESCRIPTOR);
    TAILLE(PCPIN_DESCRIPTOR);
    TAILLE(PCNODE_DESCRIPTOR);
    TAILLE(PCCONNECTION_DESCRIPTOR);
    TAILLE(PCFILTER_DESCRIPTOR);
    TAILLE(PCPROPERTY_ITEM);
    TAILLE(PCPROPERTY_REQUEST);
    TAILLE(PCEVENT_ITEM);
    TAILLE(PCEVENT_REQUEST);
    TAILLE(PCAUTOMATION_TABLE);
    TAILLE(KSJACK_DESCRIPTION);
    TAILLE(KSRTAUDIO_BUFFER);
    TAILLE(KSRTAUDIO_BUFFER_PROPERTY);
    TAILLE(KSRTAUDIO_HWLATENCY);
    TAILLE(KSRTAUDIO_NOTIFICATION_EVENT_PROPERTY);
    TAILLE(KSAUDIO_POSITION);
    TAILLE(KSSTATE);
    TAILLE(DEVICE_OBJECT);
    TAILLE(IRP);
    TAILLE(UNICODE_STRING);

    /* Structures de propriétés KS que M1b sérialise à la main (BasicSupport et
     * propriétés de nœud : volume, mute) puis, pour M1b-03, la propriété de broche
     * KSPROPERTY_JACK_DESCRIPTION : sa réponse est un KSMULTIPLE_ITEM suivi de N
     * KSJACK_DESCRIPTION, et son instance est la queue d'un KSP_PIN. */
    TAILLE(KSPROPERTY_DESCRIPTION);
    TAILLE(KSPROPERTY_MEMBERSHEADER);
    TAILLE(KSPROPERTY_STEPPING_LONG);
    TAILLE(KSNODEPROPERTY);
    TAILLE(KSNODEPROPERTY_AUDIO_CHANNEL);
    TAILLE(KSMULTIPLE_ITEM);
    TAILLE(KSP_PIN);

    /* M1b-04, événements KS. Le miniport ne construit AUCUNE de ces trois structures :
     * PortCls lui passe une PCEVENT_REQUEST qui porte l'entrée d'événement, et le
     * miniport se contente de la rendre à IPortEvents::AddEventToEventList. Les mesurer
     * sert donc à autre chose que de la sérialisation : à prouver que les bindings
     * décrivent les MÊMES structures que le WDK, là où une divergence de disposition
     * ferait lire le Verb ou le MajorTarget au mauvais décalage — c'est-à-dire un
     * déréférencement d'un champ pour un autre, au noyau. KSEVENTDATA est la structure
     * que le client remplit et que KS place derrière KSEVENT_ENTRY::EventData ; on ne
     * l'écrit pas non plus, on la mesure pour la même raison. */
    TAILLE(KSEVENTDATA);
    TAILLE(KSEVENT_ENTRY);

    /* Contraintes de taille de paquet WaveRT : la structure que le pilote pose en valeur
     * de DEVPKEY_KsAudio_PacketSize_Constraints2, et l'entrée de son tableau de queue.
     * Elle est écrite par le pilote et lue par le moteur audio : une divergence de
     * disposition ferait annoncer la période minimale à la place de l'alignement. Attention
     * à sizeof(KSAUDIO_PACKETSIZE_CONSTRAINTS2) : le ANYSIZE_ARRAY du WDK vaut 1, donc la
     * structure C mesure TOUJOURS une contrainte de mode de plus qu'elle n'en porte — c'est
     * le DÉCALAGE de ProcessingModeConstraints, et non cette taille, qui donne la longueur
     * à passer quand NumProcessingModeConstraints vaut zéro. */
    TAILLE(KSAUDIO_PACKETSIZE_CONSTRAINTS2);
    TAILLE(KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT);

    /* Vtables COM plates : slots × 8. */
    TAILLE(IUnknownVtbl);
    TAILLE(IMiniportVtbl);
    TAILLE(IMiniportWaveRTVtbl);
    TAILLE(IMiniportWaveRTStreamVtbl);
    TAILLE(IMiniportWaveRTStreamNotificationVtbl);
    TAILLE(IMiniportTopologyVtbl);
    TAILLE(IAdapterPowerManagementVtbl);
    TAILLE(IPortVtbl);
    TAILLE(IPortWaveRTVtbl);
    TAILLE(IPortTopologyVtbl);
    TAILLE(IPortWaveRTStreamVtbl);
    TAILLE(IResourceListVtbl);
    TAILLE(IRegistryKeyVtbl);
    /* M1b-04 : IPortEvents, obtenue par QueryInterface sur le port. Cinq slots
     * (IUnknown, AddEventToEventList, GenerateEventList) : c'est par cette vtable que
     * l'événement de changement de prise part. */
    TAILLE(IPortEventsVtbl);
    /* Piège du WDK 26100 : IPortClsVersion ne recopie pas DEFINE_ABSTRACT_UNKNOWN(), sa
     * vtable C n'a qu'un slot (GetVersion) alors que l'objet COM réel en a quatre
     * (IUnknown puis GetVersion). portcls-sys la corrige à la main (src/fixups.rs) ; la
     * référence est donc IUnknown + la déclaration C. IPortClsPower, IPortClsRuntimePower
     * et IPortClsEtwHelper (ni THIS_ ni IUnknown en C) sont exclus des bindings. */
    printf("sizeof\t%s\t%u\n", "IPortClsVersionVtbl",
           (unsigned)(sizeof(IUnknownVtbl) + sizeof(IPortClsVersionVtbl)));

    /* Décalages des champs des structures que M1b écrit champ par champ, plus ceux des
     * descripteurs qui portent les nœuds et les propriétés de la topologie. Une taille
     * juste ne suffit pas : c'est le décalage qui rend la sérialisation manuelle sûre. */
    DECALAGE(KSPROPERTY_DESCRIPTION, AccessFlags);
    DECALAGE(KSPROPERTY_DESCRIPTION, DescriptionSize);
    DECALAGE(KSPROPERTY_DESCRIPTION, PropTypeSet);
    DECALAGE(KSPROPERTY_DESCRIPTION, MembersListCount);
    DECALAGE(KSPROPERTY_DESCRIPTION, Reserved);
    DECALAGE(KSPROPERTY_MEMBERSHEADER, MembersFlags);
    DECALAGE(KSPROPERTY_MEMBERSHEADER, MembersSize);
    DECALAGE(KSPROPERTY_MEMBERSHEADER, MembersCount);
    DECALAGE(KSPROPERTY_MEMBERSHEADER, Flags);
    DECALAGE(KSPROPERTY_STEPPING_LONG, SteppingDelta);
    DECALAGE(KSPROPERTY_STEPPING_LONG, Reserved);
    DECALAGE(KSPROPERTY_STEPPING_LONG, Bounds);
    DECALAGE(KSNODEPROPERTY, Property);
    DECALAGE(KSNODEPROPERTY, NodeId);
    DECALAGE(KSNODEPROPERTY, Reserved);
    DECALAGE(KSNODEPROPERTY_AUDIO_CHANNEL, NodeProperty);
    DECALAGE(KSNODEPROPERTY_AUDIO_CHANNEL, Channel);
    DECALAGE(KSNODEPROPERTY_AUDIO_CHANNEL, Reserved);
    /* M1b-03. KSP_PIN : PortCls en retire l'en-tête KSPROPERTY et ne laisse dans
     * Instance que la queue — PinId d'abord —, comme il le fait du Channel des
     * propriétés de nœud. Le décalage 24 de PinId le démontre : il vaut exactement
     * sizeof(KSPROPERTY). (Reserved est dans une union anonyme, que offset_of! ne sait
     * pas nommer côté Rust : non mesuré.) */
    DECALAGE(KSMULTIPLE_ITEM, Size);
    DECALAGE(KSMULTIPLE_ITEM, Count);
    DECALAGE(KSJACK_DESCRIPTION, ChannelMapping);
    DECALAGE(KSJACK_DESCRIPTION, Color);
    DECALAGE(KSJACK_DESCRIPTION, ConnectionType);
    DECALAGE(KSJACK_DESCRIPTION, GeoLocation);
    DECALAGE(KSJACK_DESCRIPTION, GenLocation);
    DECALAGE(KSJACK_DESCRIPTION, PortConnection);
    DECALAGE(KSJACK_DESCRIPTION, IsConnected);
    DECALAGE(KSP_PIN, Property);
    DECALAGE(KSP_PIN, PinId);
    DECALAGE(PCNODE_DESCRIPTOR, Flags);
    DECALAGE(PCNODE_DESCRIPTOR, AutomationTable);
    DECALAGE(PCNODE_DESCRIPTOR, Type);
    DECALAGE(PCNODE_DESCRIPTOR, Name);
    DECALAGE(PCPROPERTY_ITEM, Set);
    DECALAGE(PCPROPERTY_ITEM, Id);
    DECALAGE(PCPROPERTY_ITEM, Flags);
    DECALAGE(PCPROPERTY_ITEM, Handler);
    /* M1b-04. PCEVENT_ITEM a la MÊME forme que PCPROPERTY_ITEM (Set, Id, Flags,
     * Handler aux mêmes décalages) : c'est ce que ces quatre lignes établissent, et
     * c'est ce qui rend le constructeur d'entrée d'événement le jumeau de celui de
     * propriété. PCEVENT_REQUEST, elle, DIFFÈRE de PCPROPERTY_REQUEST : le Verb y est
     * au décalage 40, pas 32, parce qu'un champ EventEntry s'intercale — se tromper
     * là lirait le pointeur d'entrée d'événement en guise de verbe. */
    DECALAGE(PCEVENT_ITEM, Set);
    DECALAGE(PCEVENT_ITEM, Id);
    DECALAGE(PCEVENT_ITEM, Flags);
    DECALAGE(PCEVENT_ITEM, Handler);
    DECALAGE(PCEVENT_REQUEST, MajorTarget);
    DECALAGE(PCEVENT_REQUEST, MinorTarget);
    DECALAGE(PCEVENT_REQUEST, Node);
    DECALAGE(PCEVENT_REQUEST, EventItem);
    DECALAGE(PCEVENT_REQUEST, EventEntry);
    DECALAGE(PCEVENT_REQUEST, Verb);
    DECALAGE(PCEVENT_REQUEST, Irp);
    /* La table d'automatisation porte propriétés, méthodes ET événements ; ses trois
     * triplets (ItemSize, Count, pointeur) se suivent. Les décalages du dernier
     * triplet sont ce que M1b-04 remplit. */
    DECALAGE(PCAUTOMATION_TABLE, PropertyItemSize);
    DECALAGE(PCAUTOMATION_TABLE, PropertyCount);
    DECALAGE(PCAUTOMATION_TABLE, Properties);
    DECALAGE(PCAUTOMATION_TABLE, MethodItemSize);
    DECALAGE(PCAUTOMATION_TABLE, MethodCount);
    DECALAGE(PCAUTOMATION_TABLE, Methods);
    DECALAGE(PCAUTOMATION_TABLE, EventItemSize);
    DECALAGE(PCAUTOMATION_TABLE, EventCount);
    DECALAGE(PCAUTOMATION_TABLE, Events);
    DECALAGE(PCAUTOMATION_TABLE, Reserved);
    /* KSEVENTDATA : seul NotificationType est nommable des deux côtés (le reste est une
     * union anonyme, que offset_of! ne sait pas désigner en Rust). */
    DECALAGE(KSEVENTDATA, NotificationType);
    /* Les cinq champs des contraintes de paquet, et les trois d'une contrainte de mode.
     * Le décalage de ProcessingModeConstraints est celui qui compte le plus : c'est la
     * longueur exacte de la valeur quand NumProcessingModeConstraints vaut zéro. */
    DECALAGE(KSAUDIO_PACKETSIZE_CONSTRAINTS2, MinPacketPeriodInHns);
    DECALAGE(KSAUDIO_PACKETSIZE_CONSTRAINTS2, PacketSizeFileAlignment);
    DECALAGE(KSAUDIO_PACKETSIZE_CONSTRAINTS2, MaxPacketSizeInBytes);
    DECALAGE(KSAUDIO_PACKETSIZE_CONSTRAINTS2, NumProcessingModeConstraints);
    DECALAGE(KSAUDIO_PACKETSIZE_CONSTRAINTS2, ProcessingModeConstraints);
    DECALAGE(KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT, ProcessingMode);
    DECALAGE(KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT, SamplesPerProcessingPacket);
    DECALAGE(KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT, ProcessingPacketDurationInHns);

    /* GUID. */
    GUID_PC(IID_IUnknown);
    GUID_PC(IID_IMiniportWaveRT);
    GUID_PC(IID_IMiniportTopology);
    GUID_PC(IID_IAdapterPowerManagement);
    GUID_PC(IID_IPortWaveRT);
    GUID_PC(IID_IPortTopology);
    /* M1b-04 : l'IID que QueryInterface sur le port doit recevoir pour rendre
     * l'interface par laquelle l'événement part. */
    GUID_PC(IID_IPortEvents);
    GUID_KS(KSCATEGORY_AUDIO);
    GUID_KS(KSDATAFORMAT_SUBTYPE_PCM);
    GUID_KS(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
    GUID_KS(KSNODETYPE_SPEAKER);
    GUID_KS(KSNODETYPE_VOLUME);
    GUID_KS(KSNODETYPE_MUTE);
    GUID_KS(KSPROPSETID_Jack);
    /* M1b-04 : le jeu d'événement de KSEVENT_PINCAPS_JACKINFOCHANGE. */
    GUID_KS(KSEVENTSETID_PinCapsChange);
    GUID_KS(KSPROPSETID_Audio);
    GUID_KS(KSPROPTYPESETID_General);

    /* DEVPROPKEY : la clé sur laquelle le pilote pose ses contraintes de taille de paquet.
     * Recopiée à la main côté Rust (les DEFINE_DEVPROPKEY sont en liste de blocage de
     * bindgen, faute de définition sans INITGUID) : c'est donc la seule vérification qui
     * existe, GUID ET pid. */
    DEVPROPKEY_(DEVPKEY_KsAudio_PacketSize_Constraints2);
    return 0;
}
