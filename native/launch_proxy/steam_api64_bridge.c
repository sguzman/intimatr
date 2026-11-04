// A normal PE startup import of Intimatr, never post-launch injection.
// The DLL's game-facing exports are forwarded via a generated .def table.
// There is deliberately no DllMain code under the loader lock.
#include <windows.h>
__declspec(dllimport) unsigned int __stdcall intimatr_lifecycle_state(void);
__declspec(dllexport) unsigned int __stdcall IntimatrProxyRuntimeState(void) {
    return intimatr_lifecycle_state();
}
