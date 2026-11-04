// Test-only Steam-named DLL. No Valve code or proprietary binaries.
#include <windows.h>
__declspec(dllexport) int __cdecl IntimatrFixtureValue(void) { return 731; }
