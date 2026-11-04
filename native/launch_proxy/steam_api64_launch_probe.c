// Fixture checks export forwarding and normal pre-main DLL dependency load.
#include <windows.h>
#include <stdio.h>
__declspec(dllimport) int __cdecl IntimatrFixtureValue(void);
typedef unsigned int (__stdcall *state_fn)(void);
int main(void) {
    if (IntimatrFixtureValue() != 731) { fputs("Original forward failed\n",stderr); return 1; }
    HMODULE proxy=GetModuleHandleW(L"steam_api64.dll");
    HMODULE dll=GetModuleHandleW(L"intimatr.dll");
    if (!proxy || !dll) { fputs("Launch DLL dependency missing\n",stderr); return 2; }
    state_fn state=(state_fn)GetProcAddress(dll,"intimatr_lifecycle_state");
    if(!state) return 3;
    for(int i=0;i<300;i++) {
        unsigned int s=state();
        if(s==3) {puts("Intimatr launch proxy startup verified: original export forwarded, runtime Running");return 0;}
        if(s==6) {fputs("Intimatr bootstrap failed\n",stderr);return 4;}
        Sleep(25);
    }
    fprintf(stderr,"Intimatr never reached Running: %u\n",state());return 5;
}
