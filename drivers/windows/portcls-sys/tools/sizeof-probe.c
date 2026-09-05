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
 *   guid<TAB>Nom<TAB>XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
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
    /* Piège du WDK 26100 : IPortClsVersion ne recopie pas DEFINE_ABSTRACT_UNKNOWN(), sa
     * vtable C n'a qu'un slot (GetVersion) alors que l'objet COM réel en a quatre
     * (IUnknown puis GetVersion). portcls-sys la corrige à la main (src/fixups.rs) ; la
     * référence est donc IUnknown + la déclaration C. IPortClsPower, IPortClsRuntimePower
     * et IPortClsEtwHelper (ni THIS_ ni IUnknown en C) sont exclus des bindings. */
    printf("sizeof\t%s\t%u\n", "IPortClsVersionVtbl",
           (unsigned)(sizeof(IUnknownVtbl) + sizeof(IPortClsVersionVtbl)));

    /* GUID. */
    GUID_PC(IID_IUnknown);
    GUID_PC(IID_IMiniportWaveRT);
    GUID_PC(IID_IMiniportTopology);
    GUID_PC(IID_IAdapterPowerManagement);
    GUID_PC(IID_IPortWaveRT);
    GUID_PC(IID_IPortTopology);
    GUID_KS(KSCATEGORY_AUDIO);
    GUID_KS(KSDATAFORMAT_SUBTYPE_PCM);
    GUID_KS(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
    GUID_KS(KSNODETYPE_SPEAKER);
    GUID_KS(KSPROPSETID_Jack);
    return 0;
}
