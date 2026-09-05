/*
 * En-tête d'entrée de bindgen pour portcls-sys : compilé en mode C (pas C++), pour que
 * DECLARE_INTERFACE_/STDMETHOD_ (basetyps.h) produisent des vtables plates.
 * Voir docs/driver-design.md §2.2 et docs/windows-drivers-rs.md.
 *
 * Ordre imposé :
 *  - ntddk.h : types NT (DRIVER_OBJECT, IRP, UNICODE_STRING…) référencés par PortCls ;
 *  - windef.h puis mmreg.h (NOBITMAP, comme dans portcls.h) : WAVEFORMATEX doit précéder
 *    ksmedia.h, sinon KSDATAFORMAT_WAVEFORMATEX n'existe pas et portcls.h ne compile pas ;
 *  - ks.h, ksmedia.h, punknown.h, drmk.h ;
 *  - `#define INTERFACE void` juste avant portcls.h : en mode C, THIS_ s'expanse en
 *    `INTERFACE *This,` et portcls.h ne définit jamais INTERFACE (les autres en-têtes le
 *    définissent puis l'annulent eux-mêmes).
 *
 * Pas d'initguid.h/PUT_GUIDS_HERE : bindgen n'évalue pas les initialiseurs de structure
 * et sortirait les GUID en `extern static` de toute façon ; build.rs les extrait
 * lui-même des en-têtes en `const GUID` (tools/sizeof-probe.c, lui, définit
 * PUT_GUIDS_HERE pour pouvoir lier les IID_*).
 */
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
