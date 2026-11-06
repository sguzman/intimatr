// Synthetic game imports four clocks and the forwarded Steam-style DLL.
// Real launch-time DLL bootstrap and explicitly opted-in 1x/2x/1x verification.
#include <windows.h>
#include <mmsystem.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

__declspec(dllimport) int __cdecl IntimatrFixtureValue(void);
typedef unsigned int (__stdcall *state_fn)(void);
typedef unsigned int (__stdcall *remote_fn)(void *);

// Check the live opt-in rate bridge exported for MangoHud. This is the
// *observed* hook rate, not a declared user preset.
static int await_hud_rate(HMODULE dll, const char *expected) {
    char path[MAX_PATH];
    DWORD length = GetModuleFileNameA(dll, path, MAX_PATH);
    if (length == 0 || length >= MAX_PATH) return 0;
    char *backslash = strrchr(path, '\\\\');
    char *slash = strrchr(path, '/');
    char *last = backslash > slash ? backslash : slash;
    if (!last) return 0;
    last[1] = '\\0';
    if (strlen(path) + strlen("intimatr-hud-rate.txt") >= MAX_PATH) return 0;
    strcat(path, "intimatr-hud-rate.txt");
    for (int i = 0; i < 50; ++i) {
        FILE *file = fopen(path, "rb");
        if (file) {
            char text[32] = {0};
            size_t read = fread(text, 1, sizeof(text) - 1, file);
            fclose(file);
            if (read && strcmp(text, expected) == 0) return 1;
        }
        Sleep(50);
    }
    return 0;
}

int main(void) {
    LARGE_INTEGER qpc;
    if (!QueryPerformanceCounter(&qpc)) return 10;
    volatile unsigned long long sample =
        (unsigned long long)GetTickCount() + GetTickCount64() +
        timeGetTime() + (unsigned long long)qpc.QuadPart;
    (void)sample;
    if (IntimatrFixtureValue() != 731) {
        fputs("Original export forwarding failed\n", stderr);
        return 1;
    }
    HMODULE bridge = GetModuleHandleW(L"steam_api64.dll");
    HMODULE dll = GetModuleHandleW(L"intimatr.dll");
    if (!bridge || !dll) {
        fputs("Proxy or Intimatr was not loaded at process startup\n", stderr);
        return 2;
    }
    state_fn state = (state_fn)GetProcAddress(dll, "intimatr_lifecycle_state");
    remote_fn hooks = (remote_fn)GetProcAddress(dll, "intimatr_remote_timer_hook_state");
    remote_fn rate = (remote_fn)GetProcAddress(dll, "intimatr_remote_game_rate_milli");
    remote_fn preset = (remote_fn)GetProcAddress(dll, "intimatr_remote_game_preset");
    if (!state || !hooks || !rate || !preset) return 3;
    const char *flag = getenv("INTIMATR_EXPERIMENTAL_STARTUP_TIMERS");
    int opted_in = flag && strcmp(flag, "1") == 0;
    for (int i = 0; i < 300; ++i) {
        if (state() == 6) {
            fputs("Intimatr startup failed\n", stderr);
            return 4;
        }
        if (state() == 3 && (!opted_in || hooks(NULL) == 1)) {
            if (!opted_in) {
                puts("Intimatr launch proxy startup verified: original export forwarded, runtime Running");
                return 0;
            }
            if (rate(NULL) != 1000) {
                fputs("Startup hooks did not begin at 1x\n", stderr);
                return 5;
            }
            if (preset((void *)(uintptr_t)3) != 1 || rate(NULL) != 2000) {
                fputs("Startup hooks failed 2x preset\n", stderr);
                return 6;
            }
            if (!await_hud_rate(dll, "2.00x")) {
                fputs("HUD exporter failed to publish observed 2.00x rate\n", stderr);
                return 9;
            }
            if (preset((void *)(uintptr_t)2) != 1 || rate(NULL) != 1000) {
                fputs("Startup hooks failed 1x reset\n", stderr);
                return 7;
            }
            if (!await_hud_rate(dll, "1.00x")) {
                fputs("HUD exporter failed to publish observed reset to 1.00x\n", stderr);
                return 10;
            }
            puts("Intimatr load-time startup hooks verified: four timers, 1x/2x/1x, HUD exported live rates, original export forwarded");
            return 0;
        }
        Sleep(25);
    }
    fprintf(stderr, "Startup did not reach expected state: runtime %u hooks %u\n", state(), hooks(NULL));
    return 8;
}
