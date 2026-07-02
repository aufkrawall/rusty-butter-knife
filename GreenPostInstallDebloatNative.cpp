// GreenPostInstallDebloatNative-TI-Menu-Logging-20260702-safe-match-virtualaudio-optin.cpp
//
// Native Windows C++17 NVIDIA post-install debloater.
// Intended compiler targets: MinGW-w64 GCC or LLVM/clang++ for Windows.
//
// Build examples:
//   clang++ -std=c++17 -municode -O2 -Wall -Wextra GreenPostInstallDebloatNative.cpp -o GreenPostInstallDebloatNative.exe -ladvapi32 -lshell32
//   g++     -std=c++17 -municode -O2 -Wall -Wextra GreenPostInstallDebloatNative.cpp -o GreenPostInstallDebloatNative.exe -ladvapi32 -lshell32
//
// Safety model:
//   - Dry-run by default.
//   - Logs are written beside the .exe.
//   - Destructive execution requires NT SERVICE\TrustedInstaller unless --allow-admin-fallback is set.
//   - The program can attempt the same scheduled-task TrustedInstaller relaunch approach as the prior PowerShell version.
//   - It does not steal tokens, inject into processes, or force-unload DLLs.
//   - Locked files can be scheduled for deletion on reboot with MoveFileEx.
//
// Typical default cleanup:
//   GreenPostInstallDebloatNative.exe --execute --kill-lockers --disable-services --delete-scheduled-tasks --schedule-reboot-delete
//
// Interactive menu:
//   GreenPostInstallDebloatNative.exe --menu

#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#define NOMINMAX

#include <windows.h>
#include <shellapi.h>
#include <tlhelp32.h>
#include <sddl.h>
#include <winsvc.h>
#include <taskschd.h>
#include <comdef.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cwctype>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <system_error>
#include <map>
#include <optional>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

namespace fs = std::filesystem;

struct Options {
    bool execute = false;
    bool menu = false;
    bool tiChild = false;
    bool allowAdminFallback = false;
    bool attemptTiRelaunch = true;
    bool killLockers = false;
    bool preserveNvContainers = true;
    bool disableServices = false;
    bool deleteServices = false;
    bool disableScheduledTasks = true;
    bool deleteScheduledTasks = false;
    bool scheduleLockedForReboot = false;
    bool takeOwnership = false;
    bool includeNGX = false;
    bool includeHDAudio = false;
    bool includePhysX = false;
    bool includeNotebookOptimus = false;
    bool includeVirtualAudio = false;
    bool includeNvWmi = false;
    bool includeCaptureSdk = false;
    bool noPause = false;
    bool noColor = false;
    int tiWaitSeconds = 600;
    std::wstring statusFile;
    std::wstring logDirOverride;
};

struct Component {
    std::wstring key;
    std::wstring displayName;
    bool defaultEnabled;
    bool optional;
    std::vector<std::wstring> leafGlobs;
    std::vector<std::wstring> exactLeafNames;
};

struct Candidate {
    fs::path path;
    std::wstring componentKey;
    std::wstring componentName;
    bool isDirectory = false;
};

struct ActionRecord {
    std::wstring type;
    std::wstring status;
    std::wstring component;
    std::wstring path;
    std::wstring detail;
};

struct RunState {
    fs::path exePath;
    fs::path exeDir;
    std::wstring runId;
    fs::path logPath;
    fs::path reportPath;
    fs::path candidateCsvPath;
    std::vector<ActionRecord> actions;
    std::vector<Candidate> candidates;
};

static Options g_options;
static RunState g_state;
static std::map<std::wstring, bool> g_componentEnabled;

// -----------------------------
// String helpers
// -----------------------------

std::wstring toLower(std::wstring s) {
    std::transform(s.begin(), s.end(), s.begin(), [](wchar_t c) { return static_cast<wchar_t>(std::towlower(c)); });
    return s;
}

bool containsNoCase(const std::wstring& haystack, const std::wstring& needle) {
    return toLower(haystack).find(toLower(needle)) != std::wstring::npos;
}

std::wstring trim(const std::wstring& s) {
    size_t first = s.find_first_not_of(L" \t\r\n");
    if (first == std::wstring::npos) return L"";
    size_t last = s.find_last_not_of(L" \t\r\n");
    return s.substr(first, last - first + 1);
}

std::string utf8(const std::wstring& w) {
    if (w.empty()) return {};
    int needed = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), static_cast<int>(w.size()), nullptr, 0, nullptr, nullptr);
    if (needed <= 0) return {};
    std::string out(static_cast<size_t>(needed), '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.c_str(), static_cast<int>(w.size()), out.data(), needed, nullptr, nullptr);
    return out;
}

std::wstring widen(const std::string& s) {
    if (s.empty()) return {};
    int needed = MultiByteToWideChar(CP_UTF8, 0, s.c_str(), static_cast<int>(s.size()), nullptr, 0);
    if (needed > 0) {
        std::wstring out(static_cast<size_t>(needed), L'\0');
        MultiByteToWideChar(CP_UTF8, 0, s.c_str(), static_cast<int>(s.size()), out.data(), needed);
        return out;
    }
    needed = MultiByteToWideChar(CP_ACP, 0, s.c_str(), static_cast<int>(s.size()), nullptr, 0);
    if (needed <= 0) return {};
    std::wstring out(static_cast<size_t>(needed), L'\0');
    MultiByteToWideChar(CP_ACP, 0, s.c_str(), static_cast<int>(s.size()), out.data(), needed);
    return out;
}

std::wstring quoteArg(const std::wstring& arg) {
    std::wstring out = L"\"";
    size_t backslashes = 0;
    for (wchar_t c : arg) {
        if (c == L'\\') {
            backslashes++;
        } else if (c == L'\"') {
            out.append(backslashes * 2 + 1, L'\\');
            out.push_back(c);
            backslashes = 0;
        } else {
            out.append(backslashes, L'\\');
            backslashes = 0;
            out.push_back(c);
        }
    }
    out.append(backslashes * 2, L'\\');
    out.push_back(L'\"');
    return out;
}

std::wstring joinCommand(const std::vector<std::wstring>& args) {
    std::wstring out;
    for (size_t i = 0; i < args.size(); ++i) {
        if (i) out += L" ";
        const auto& a = args[i];
        bool needsQuotes = a.empty() || a.find_first_of(L" \t\"&|<>^") != std::wstring::npos;
        out += needsQuotes ? quoteArg(a) : a;
    }
    return out;
}

bool wildcardMatchNoCase(const std::wstring& textIn, const std::wstring& patternIn) {
    std::wstring text = toLower(textIn);
    std::wstring pattern = toLower(patternIn);
    size_t t = 0, p = 0, star = std::wstring::npos, match = 0;
    while (t < text.size()) {
        if (p < pattern.size() && (pattern[p] == L'?' || pattern[p] == text[t])) {
            ++t; ++p;
        } else if (p < pattern.size() && pattern[p] == L'*') {
            star = p++;
            match = t;
        } else if (star != std::wstring::npos) {
            p = star + 1;
            t = ++match;
        } else {
            return false;
        }
    }
    while (p < pattern.size() && pattern[p] == L'*') ++p;
    return p == pattern.size();
}

std::wstring nowStamp() {
    SYSTEMTIME st;
    GetLocalTime(&st);
    wchar_t buf[64];
    swprintf(buf, 64, L"%04u%02u%02u-%02u%02u%02u", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond);
    return buf;
}

std::wstring formatWinError(DWORD err) {
    LPWSTR msg = nullptr;
    DWORD flags = FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
    DWORD len = FormatMessageW(flags, nullptr, err, 0, reinterpret_cast<LPWSTR>(&msg), 0, nullptr);
    std::wstring out = L"0x";
    wchar_t hex[16];
    swprintf(hex, 16, L"%08X", err);
    out += hex;
    if (len && msg) {
        out += L" ";
        out += trim(msg);
        LocalFree(msg);
    }
    return out;
}

std::wstring jsonEscape(const std::wstring& in) {
    std::wstring out;
    for (wchar_t c : in) {
        switch (c) {
            case L'\\': out += L"\\\\"; break;
            case L'\"': out += L"\\\""; break;
            case L'\b': out += L"\\b"; break;
            case L'\f': out += L"\\f"; break;
            case L'\n': out += L"\\n"; break;
            case L'\r': out += L"\\r"; break;
            case L'\t': out += L"\\t"; break;
            default:
                if (c < 0x20) {
                    wchar_t buf[8];
                    swprintf(buf, 8, L"\\u%04x", c);
                    out += buf;
                } else {
                    out += c;
                }
        }
    }
    return out;
}

// -----------------------------
// Logging
// -----------------------------

void appendUtf8File(const fs::path& path, const std::string& text) {
    HANDLE h = CreateFileW(path.wstring().c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return;
    DWORD written = 0;
    WriteFile(h, text.data(), static_cast<DWORD>(text.size()), &written, nullptr);
    CloseHandle(h);
}

void consoleColor(WORD color) {
    if (g_options.noColor) return;
    HANDLE h = GetStdHandle(STD_OUTPUT_HANDLE);
    if (h != INVALID_HANDLE_VALUE) SetConsoleTextAttribute(h, color);
}

void logLine(const std::wstring& level, const std::wstring& message) {
    SYSTEMTIME st;
    GetLocalTime(&st);
    wchar_t prefix[128];
    swprintf(prefix, 128, L"[%04u-%02u-%02u %02u:%02u:%02u] [%ls] ",
             st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, level.c_str());
    std::wstring line = std::wstring(prefix) + message + L"\n";

    if (level == L"ERROR" || level == L"FATAL") consoleColor(FOREGROUND_RED | FOREGROUND_INTENSITY);
    else if (level == L"WARN") consoleColor(FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_INTENSITY);
    else if (level == L"DRYRUN") consoleColor(FOREGROUND_BLUE | FOREGROUND_GREEN | FOREGROUND_INTENSITY);
    else consoleColor(FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE);

    std::wcout << line;
    consoleColor(FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE);
    appendUtf8File(g_state.logPath, utf8(line));
}

void addAction(const std::wstring& type, const std::wstring& status, const std::wstring& component,
               const std::wstring& path, const std::wstring& detail = L"") {
    g_state.actions.push_back({type, status, component, path, detail});
               }

void logAction(const std::wstring& type, const std::wstring& status, const std::wstring& component,
               const std::wstring& path, const std::wstring& detail = L"") {
    addAction(type, status, component, path, detail);
    std::wstring msg = type + L" [" + component + L"] " + path;
    if (!detail.empty()) msg += L" :: " + detail;
    logLine(status, msg);
               }

// -----------------------------
// Process helpers
// -----------------------------

struct ProcessResult {
    DWORD exitCode = 0xFFFFFFFF;
    std::wstring output;
    bool started = false;
};

ProcessResult runProcessCapture(const std::wstring& commandLine, DWORD timeoutMs = 120000) {
    ProcessResult result;
    SECURITY_ATTRIBUTES sa{};
    sa.nLength = sizeof(sa);
    sa.bInheritHandle = TRUE;

    HANDLE readPipe = nullptr, writePipe = nullptr;
    if (!CreatePipe(&readPipe, &writePipe, &sa, 0)) {
        result.output = L"CreatePipe failed: " + formatWinError(GetLastError());
        return result;
    }
    SetHandleInformation(readPipe, HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOW si{};
    PROCESS_INFORMATION pi{};
    si.cb = sizeof(si);
    si.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    si.hStdOutput = writePipe;
    si.hStdError = writePipe;
    si.wShowWindow = SW_HIDE;

    std::wstring mutableCmd = commandLine;
    BOOL ok = CreateProcessW(nullptr, mutableCmd.data(), nullptr, nullptr, TRUE, CREATE_NO_WINDOW, nullptr, nullptr, &si, &pi);
    CloseHandle(writePipe);

    if (!ok) {
        result.output = L"CreateProcess failed: " + formatWinError(GetLastError()) + L" Command=" + commandLine;
        CloseHandle(readPipe);
        return result;
    }

    result.started = true;
    std::string bytes;
    char buffer[4096];
    DWORD read = 0;
    auto start = GetTickCount64();

    while (true) {
        while (PeekNamedPipe(readPipe, nullptr, 0, nullptr, &read, nullptr) && read > 0) {
            DWORD toRead = std::min<DWORD>(read, sizeof(buffer));
            DWORD got = 0;
            if (ReadFile(readPipe, buffer, toRead, &got, nullptr) && got > 0) {
                bytes.append(buffer, buffer + got);
            } else {
                break;
            }
        }

        DWORD wait = WaitForSingleObject(pi.hProcess, 50);
        if (wait == WAIT_OBJECT_0) break;
        if (GetTickCount64() - start > timeoutMs) {
            TerminateProcess(pi.hProcess, 0xFFFF0001);
            result.output += L"Timed out. ";
            break;
        }
    }

    while (ReadFile(readPipe, buffer, sizeof(buffer), &read, nullptr) && read > 0) {
        bytes.append(buffer, buffer + read);
    }

    GetExitCodeProcess(pi.hProcess, &result.exitCode);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    CloseHandle(readPipe);
    result.output += widen(bytes);
    return result;
}

bool runShellCommand(const std::wstring& commandLine, const std::wstring& label, bool dryRunOk = false) {
    if (!g_options.execute && dryRunOk) {
        logLine(L"DRYRUN", L"Would run: " + commandLine);
        return true;
    }
    auto res = runProcessCapture(commandLine);
    std::wstring msg = label + L" exit=" + std::to_wstring(res.exitCode);
    if (!trim(res.output).empty()) msg += L" output=" + trim(res.output);
    logLine(res.exitCode == 0 ? L"INFO" : L"WARN", msg);
    return res.exitCode == 0;
}

// -----------------------------
// Identity / elevation
// -----------------------------

bool isAdmin() {
    BOOL isMember = FALSE;
    PSID adminGroup = nullptr;
    SID_IDENTIFIER_AUTHORITY NtAuthority = SECURITY_NT_AUTHORITY;
    if (AllocateAndInitializeSid(&NtAuthority, 2, SECURITY_BUILTIN_DOMAIN_RID, DOMAIN_ALIAS_RID_ADMINS,
        0, 0, 0, 0, 0, 0, &adminGroup)) {
        CheckTokenMembership(nullptr, adminGroup, &isMember);
    FreeSid(adminGroup);
        }
        return isMember == TRUE;
}

std::wstring currentTokenAccount() {
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return L"<unknown>";
    DWORD needed = 0;
    GetTokenInformation(token, TokenUser, nullptr, 0, &needed);
    std::vector<BYTE> buf(needed);
    if (!GetTokenInformation(token, TokenUser, buf.data(), needed, &needed)) {
        CloseHandle(token);
        return L"<unknown>";
    }
    TOKEN_USER* tu = reinterpret_cast<TOKEN_USER*>(buf.data());
    wchar_t name[256], domain[256];
    DWORD nameLen = 256, domainLen = 256;
    SID_NAME_USE use;
    if (!LookupAccountSidW(nullptr, tu->User.Sid, name, &nameLen, domain, &domainLen, &use)) {
        LPWSTR sidStr = nullptr;
        ConvertSidToStringSidW(tu->User.Sid, &sidStr);
        std::wstring sid = sidStr ? sidStr : L"<sid unknown>";
        if (sidStr) LocalFree(sidStr);
        CloseHandle(token);
        return sid;
    }
    CloseHandle(token);
    return std::wstring(domain) + L"\\" + name;
}

// TrustedInstaller approach:
//
// This program does NOT use undocumented APIs (NtImpersonateThread, token
// duplication, etc.) to seize TrustedInstaller's token.  Instead it requests
// a scheduled task via the Task Scheduler COM API, asking the system to run
// as NT SERVICE\TrustedInstaller.  Windows does not grant this request --
// the task always runs as SYSTEM -- but SYSTEM owns SeBackupPrivilege and
// SeRestorePrivilege, which are sufficient to delete or take ownership of
// the NVIDIA bloat files that a normal Administrator cannot touch.
//
// The task is created by an already-elevated Administrator.  It runs the
// same .exe with the --ti-child flag; after completion it writes a status
// file that the parent process polls.  The task is always deleted after use.
bool isTrustedInstaller() {
    std::wstring acct = toLower(currentTokenAccount());
    return acct == L"nt service\\trustedinstaller" || acct.find(L"trustedinstaller") != std::wstring::npos;
}

// -----------------------------
// Components / options
// -----------------------------

std::vector<Component> buildComponents() {
    return {
        {L"Telemetry", L"Telemetry, DisplayDriverRAS, container telemetry plugins", true, false,
            {L"*Telemetry*", L"_DisplayDriverRAS.dll", L"_NvMsgBusBroadcast.dll", L"_nvtopps.dll", L"_NvGSTPlugin.dll", L"NvTelemetry*.dll", L"NvTelemetry*.exe"},
            {L"DisplayDriverRAS", L"GameSessionTelemetry", L"NvTelemetry"}},

            {L"UpdateAndProfileUpdater", L"Driver update and profile-updater components", true, false,
                {L"nvprofileupdaterplugin.dll", L"*ProfileUpdater*", L"*DriverUpdate*", L"*NvOTA*", L"*NvBackend*", L"*NvModuleTracker*", L"*NvGFTrayPlugin*", L"*NvNodeLauncher*"},
                {L"Display.Update", L"Update.Core", L"NvProfileUpdaterPlugin", L"NvBackend", L"NvDriverUpdateCheck", L"NvModuleTracker", L"NvOTA"}},

                {L"Installer2Cache", L"Installer2 unpacked installer cache", true, false,
                    {}, {L"Installer2"}},

                    {L"GeForceExperienceAndNvidiaApp", L"NVIDIA App / GeForce Experience userland extras", true, false,
                        {L"*GeForce Experience*", L"*GFExperience*", L"*NVIDIA Web Helper*", L"NVIDIA App*.exe", L"*NvBackend*"},
                        {L"NVIDIA App", L"NVIDIA GeForce Experience", L"GeForce Experience", L"GFExperience"}},

                        {L"ShadowPlayShare", L"ShadowPlay / NVIDIA Share userland extras", true, false,
                            // Keep this tight: broad '*Share*.dll' matched Nsight InterfaceShared*.dll.
                            // NvFBC/NvIFR are capture SDK/runtime APIs and are opt-in via CaptureSDK.
                            {L"*ShadowPlay*", L"NVIDIA Share*.exe", L"nvsphelper*.exe"},
                            {L"ShadowPlay", L"NVIDIA Share"}},

                            {L"CaptureSDK", L"NvFBC / NvIFR capture SDK runtime components", false, true,
                                {L"NvIFR*.dll", L"NvFBC*.dll"},
                                {}},

                                {L"AnselCamera", L"Ansel / NvCamera", true, false,
                                    {L"NvCamera*.dll", L"*Ansel*"}, {L"NvCamera", L"Ansel"}},

                                    {L"FrameView", L"FrameView SDK / PresentMon extras", true, false,
                                        {L"*FrameView*", L"*PresentMon*", L"NvFv*.dll", L"NvFrameView*.dll"},
                                        {L"FrameViewSDK", L"FrameView", L"PresentMon"}},

                                        {L"Shield", L"SHIELD / streaming / wireless controller support", true, false,
                                            // Do not match NvWGF2UMX*.dll: nvwgf2umx.dll is a core display user-mode driver.
                                            {L"*NvStream*", L"*Shield*", L"*SHIELD*", L"*WirelessController*"},
                                            {L"NvStream", L"SHIELD", L"Shield", L"WirelessController"}},

                                            {L"VirtualAudio", L"NVIDIA virtual audio device", false, true,
                                                // ProgramData\NvVAD is matched by exact leaf. Avoid broad '*NvVAD*' because it matched DriverStore package sidecars.
                                                {L"NvVAD*.dll", L"NvVAD*.exe", L"NvVAD*.sys", L"nvvad*.sys"}, {L"NvVAD"}},

                                                {L"USBTypeC", L"USB-C / virtual host controller support", true, false,
                                                    {L"*USB-C*", L"*NvUSB*", L"*NvvHCI*", L"nvvhci*.inf", L"nvvhci*.sys", L"*nvppc*"},
                                                    {L"NvvHCI", L"USB-C", L"NvUSB"}},

                                                    {L"Legacy3DVisionVR", L"Legacy 3D Vision / Stereo extras", true, false,
                                                        // Do not use broad '*VR*': it matched CUDA nvrtc*, NvRules*, and VRAM documentation.
                                                        // NVWMI is a management interface and is opt-in via NvWMI.
                                                        {L"*3D Vision*", L"*Stereo*", L"nvst*.dll"}, {L"3D Vision", L"Stereo"}},

                                                        {L"NvWMI", L"NVIDIA WMI management interface", false, true,
                                                            {L"nvwmi*.dll", L"nvwmi*.exe", L"nvwmi*.mof"},
                                                            {L"NVWMI"}},

                                                            {L"NGX", L"NGX / DLSS runtime cache and plugins", false, true,
                                                                {L"nvngx*.dll", L"*NGX*"}, {L"NGX", L"NvNGX"}},

                                                                {L"HDAudio", L"NVIDIA HD Audio", false, true,
                                                                    {L"nvhda*.inf", L"nvhda*.sys", L"*HDAudio*", L"*HD Audio*", L"*NvHDA*"}, {L"HDAudio", L"HD Audio", L"NvHDA"}},

                                                                    {L"PhysX", L"NVIDIA PhysX", false, true,
                                                                        {L"*PhysX*"}, {L"PhysX"}},

                                                                        {L"NotebookOptimus", L"Notebook/Optimus/MSI helper components", false, true,
                                                                            {L"*Optimus*", L"*NvOptimus*", L"nvdmi*.inf", L"*NvMsi*"}, {L"Optimus", L"NvOptimus", L"NvMsi"}}
    };
}

void initializeComponentSelection() {
    for (const auto& c : buildComponents()) {
        bool enabled = c.defaultEnabled;
        if (c.key == L"NGX") enabled = g_options.includeNGX;
        if (c.key == L"HDAudio") enabled = g_options.includeHDAudio;
        if (c.key == L"PhysX") enabled = g_options.includePhysX;
        if (c.key == L"NotebookOptimus") enabled = g_options.includeNotebookOptimus;
        if (c.key == L"VirtualAudio") enabled = g_options.includeVirtualAudio;
        if (c.key == L"NvWMI") enabled = g_options.includeNvWmi;
        if (c.key == L"CaptureSDK") enabled = g_options.includeCaptureSdk;
        g_componentEnabled[c.key] = enabled;
    }
}

void printUsage() {
    std::wcout << L"\nNVIDIA post-install debloater native C++ build\n\n"
    << L"Usage:\n"
    << L"  GreenPostInstallDebloatNative.exe --menu\n"
    << L"  GreenPostInstallDebloatNative.exe --dry-run\n"
    << L"  GreenPostInstallDebloatNative.exe --execute --kill-lockers --disable-services --delete-scheduled-tasks --schedule-reboot-delete\n\n"
    << L"Core switches:\n"
    << L"  --execute                       Actually delete/disable. Default is dry-run.\n"
    << L"  --dry-run                       Explicit dry-run.\n"
    << L"  --menu                          Interactive component selection.\n"
    << L"  --kill-lockers                  Stop bloat services/processes before deleting.\n"
    << L"  --preserve-nvcontainers=false   Allow killing NVDisplay.Container/nvcontainer if matched. Default preserves them.\n"
    << L"  --disable-services              Disable matching NVIDIA bloat services.\n"
    << L"  --delete-services               Delete matching NVIDIA bloat services.\n"
    << L"  --disable-scheduled-tasks       Disable matching NVIDIA scheduled tasks. Default true.\n"
    << L"  --delete-scheduled-tasks        Delete matching NVIDIA scheduled tasks.\n"
    << L"  --schedule-reboot-delete        Use MoveFileEx for locked files/folders.\n"
    << L"  --take-ownership                Run takeown/icacls before deletion. Usually unnecessary under TrustedInstaller.\n\n"
    << L"TrustedInstaller behavior:\n"
    << L"  --no-ti-relaunch                Do not attempt automatic TrustedInstaller scheduled-task relaunch.\n"
    << L"  --allow-admin-fallback          Permit destructive execution as Administrator if TI relaunch fails/skipped.\n"
    << L"  --ti-wait-seconds N             Parent wait timeout for TI child. Default 600.\n\n"
    << L"Optional component inclusions:\n"
    << L"  --include-ngx --include-hdaudio --include-physx --include-notebook-optimus\n"
    << L"  --include-virtual-audio        Include NvVAD/NVIDIA Virtual Audio Device cleanup. Off by default.\n"
    << L"  --include-nvwmi                Include NVIDIA WMI management interface cleanup. Off by default.\n"
    << L"  --include-capture-sdk          Include NvFBC/NvIFR capture SDK runtime cleanup. Off by default.\n\n";
}

bool parseBoolAssignment(const std::wstring& arg, const std::wstring& name, bool& out) {
    std::wstring low = toLower(arg);
    std::wstring prefix = toLower(name) + L"=";
    if (low.rfind(prefix, 0) != 0) return false;
    std::wstring v = low.substr(prefix.size());
    out = !(v == L"false" || v == L"0" || v == L"no" || v == L"off");
    return true;
}

Options parseArgs(int argc, wchar_t** argv) {
    Options opt;
    if (argc <= 1) opt.menu = true;
    for (int i = 1; i < argc; ++i) {
        std::wstring a = argv[i];
        std::wstring low = toLower(a);
        if (low == L"--help" || low == L"-h" || low == L"/?") { printUsage(); ExitProcess(0); }
        else if (low == L"--menu") opt.menu = true;
        else if (low == L"--no-menu") opt.menu = false;
        else if (low == L"--execute" || low == L"-execute") opt.execute = true;
        else if (low == L"--dry-run" || low == L"-dryrun") opt.execute = false;
        else if (low == L"--allow-admin-fallback") opt.allowAdminFallback = true;
        else if (low == L"--no-ti-relaunch") opt.attemptTiRelaunch = false;
        else if (low == L"--ti-child") opt.tiChild = true;
        else if (low == L"--kill-lockers") opt.killLockers = true;
        else if (low == L"--disable-services") opt.disableServices = true;
        else if (low == L"--delete-services") opt.deleteServices = true;
        else if (low == L"--disable-scheduled-tasks") opt.disableScheduledTasks = true;
        else if (low == L"--no-disable-scheduled-tasks") opt.disableScheduledTasks = false;
        else if (low == L"--delete-scheduled-tasks") opt.deleteScheduledTasks = true;
        else if (low == L"--schedule-reboot-delete") opt.scheduleLockedForReboot = true;
        else if (low == L"--take-ownership") opt.takeOwnership = true;
        else if (low == L"--include-ngx") opt.includeNGX = true;
        else if (low == L"--include-hdaudio") opt.includeHDAudio = true;
        else if (low == L"--include-physx") opt.includePhysX = true;
        else if (low == L"--include-notebook-optimus") opt.includeNotebookOptimus = true;
        else if (low == L"--include-virtual-audio") opt.includeVirtualAudio = true;
        else if (low == L"--include-nvwmi") opt.includeNvWmi = true;
        else if (low == L"--include-capture-sdk") opt.includeCaptureSdk = true;
        else if (low == L"--no-pause") opt.noPause = true;
        else if (low == L"--no-color") opt.noColor = true;
        else if (parseBoolAssignment(a, L"--preserve-nvcontainers", opt.preserveNvContainers)) {}
        else if ((low == L"--status-file" || low == L"--log-dir" || low == L"--ti-wait-seconds") && i + 1 < argc) {
            std::wstring v = argv[++i];
            if (low == L"--status-file") opt.statusFile = v;
            else if (low == L"--log-dir") opt.logDirOverride = v;
            else if (low == L"--ti-wait-seconds") opt.tiWaitSeconds = std::max(15, _wtoi(v.c_str()));
        } else if (low.rfind(L"--component=", 0) == 0) {
            // Parsed after component map exists. Format: --component=Key:on/off
            continue;
        } else {
            std::wcerr << L"Unknown option: " << a << L"\n";
        }
    }
    return opt;
}

void applyComponentArgs(int argc, wchar_t** argv) {
    for (int i = 1; i < argc; ++i) {
        std::wstring a = argv[i];
        std::wstring low = toLower(a);
        if (low.rfind(L"--component=", 0) == 0) {
            std::wstring spec = a.substr(std::wstring(L"--component=").size());
            size_t sep = spec.find_first_of(L":=");
            if (sep == std::wstring::npos) continue;
            std::wstring key = spec.substr(0, sep);
            std::wstring val = toLower(spec.substr(sep + 1));
            bool on = !(val == L"false" || val == L"0" || val == L"off" || val == L"no");
            for (auto& kv : g_componentEnabled) {
                if (toLower(kv.first) == toLower(key)) kv.second = on;
            }
        }
    }
}

void interactiveMenu() {
    auto comps = buildComponents();
    std::wcout << L"\nNVIDIA post-install debloater\n"
    << L"Logs will be written next to this executable.\n"
    << L"Dry-run is recommended before destructive execution.\n\n";

    while (true) {
        std::wcout << L"Mode: " << (g_options.execute ? L"EXECUTE" : L"DRY RUN") << L"\n";
        std::wcout << L"Preserve NVIDIA Container processes: " << (g_options.preserveNvContainers ? L"yes" : L"no") << L"\n";
        std::wcout << L"Kill lockers: " << (g_options.killLockers ? L"yes" : L"no") << L"\n";
        std::wcout << L"Disable services: " << (g_options.disableServices ? L"yes" : L"no") << L"\n";
        std::wcout << L"Delete scheduled tasks: " << (g_options.deleteScheduledTasks ? L"yes" : L"no") << L"\n";
        std::wcout << L"Schedule locked files for reboot deletion: " << (g_options.scheduleLockedForReboot ? L"yes" : L"no") << L"\n\n";

        for (size_t i = 0; i < comps.size(); ++i) {
            bool on = g_componentEnabled[comps[i].key];
            std::wcout << std::setw(2) << (i + 1) << L") [" << (on ? L"x" : L" ") << L"] "
            << comps[i].key << L" - " << comps[i].displayName
            << (comps[i].optional ? L" (optional)" : L"") << L"\n";
        }
        std::wcout << L"\nCommands:\n"
        << L"  number = toggle component\n"
        << L"  e = toggle execute/dry-run\n"
        << L"  k = toggle kill-lockers\n"
        << L"  s = toggle disable-services\n"
        << L"  t = toggle delete-scheduled-tasks\n"
        << L"  r = toggle reboot-delete for locked files\n"
        << L"  c = toggle preserve NVIDIA containers\n"
        << L"  y = start\n"
        << L"  q = quit\n"
        << L"> ";
        std::wstring input;
        std::getline(std::wcin, input);
        input = trim(input);
        if (input.empty()) continue;
        std::wstring low = toLower(input);
        if (low == L"q") ExitProcess(0);
        if (low == L"y") break;
        if (low == L"e") { g_options.execute = !g_options.execute; continue; }
        if (low == L"k") { g_options.killLockers = !g_options.killLockers; continue; }
        if (low == L"s") { g_options.disableServices = !g_options.disableServices; continue; }
        if (low == L"t") { g_options.deleteScheduledTasks = !g_options.deleteScheduledTasks; continue; }
        if (low == L"r") { g_options.scheduleLockedForReboot = !g_options.scheduleLockedForReboot; continue; }
        if (low == L"c") { g_options.preserveNvContainers = !g_options.preserveNvContainers; continue; }
        int n = _wtoi(input.c_str());
        if (n >= 1 && n <= static_cast<int>(comps.size())) {
            auto& key = comps[n - 1].key;
            g_componentEnabled[key] = !g_componentEnabled[key];
        }
        std::wcout << L"\n";
    }
}

// -----------------------------
// Filesystem/log setup
// -----------------------------

fs::path getExePath() {
    std::wstring buf(32768, L'\0');
    DWORD len = GetModuleFileNameW(nullptr, buf.data(), static_cast<DWORD>(buf.size()));
    buf.resize(len);
    return fs::path(buf);
}

void initializeRunState() {
    g_state.exePath = getExePath();
    g_state.exeDir = g_options.logDirOverride.empty() ? g_state.exePath.parent_path() : fs::path(g_options.logDirOverride);
    g_state.runId = nowStamp();
    std::error_code ec;
    fs::create_directories(g_state.exeDir, ec);
    g_state.logPath = g_state.exeDir / (L"debloat-" + g_state.runId + L".log");
    g_state.reportPath = g_state.exeDir / L"last-report.json";
    g_state.candidateCsvPath = g_state.exeDir / L"last-candidates.csv";
}

// -----------------------------
// TrustedInstaller scheduled task relaunch
// -----------------------------

std::vector<std::wstring> getCurrentArgsNoExe() {
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    std::vector<std::wstring> out;
    if (!argv) return out;
    for (int i = 1; i < argc; ++i) {
        std::wstring a = argv[i];
        std::wstring low = toLower(a);
        if (low == L"--menu") continue;
        if (low == L"--ti-child") continue;
        if (low == L"--status-file" && i + 1 < argc) { ++i; continue; }
        if (low == L"--log-dir" && i + 1 < argc) { ++i; continue; }
        out.push_back(a);
    }
    LocalFree(argv);
    return out;
}

void writeStatusJson(const fs::path& statusFile, int exitCode, const std::wstring& status, const std::wstring& detail) {
    if (statusFile.empty()) return;
    std::wstringstream ss;
    ss << L"{\n"
    << L"  \"runId\": \"" << jsonEscape(g_state.runId) << L"\",\n"
    << L"  \"exitCode\": " << exitCode << L",\n"
    << L"  \"status\": \"" << jsonEscape(status) << L"\",\n"
    << L"  \"detail\": \"" << jsonEscape(detail) << L"\",\n"
    << L"  \"logPath\": \"" << jsonEscape(g_state.logPath.wstring()) << L"\",\n"
    << L"  \"reportPath\": \"" << jsonEscape(g_state.reportPath.wstring()) << L"\"\n"
    << L"}\n";
    HANDLE h = CreateFileW(statusFile.wstring().c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h != INVALID_HANDLE_VALUE) {
        std::string bytes = utf8(ss.str());
        DWORD written = 0;
        WriteFile(h, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr);
        CloseHandle(h);
    }
}

// Attempts to register and run a one-shot scheduled task as
// NT SERVICE\TrustedInstaller using the Task Scheduler COM API.
// Windows does not grant this request -- the task runs as SYSTEM --
// but SYSTEM has sufficient privileges for the debloat operations
// (SeBackupPrivilege, SeRestorePrivilege).
bool attemptTrustedInstallerRelaunch(int argc, wchar_t** argv) {
    (void)argc; (void)argv;
    if (!g_options.execute || g_options.tiChild || g_options.allowAdminFallback || isTrustedInstaller()) return false;
    if (!g_options.attemptTiRelaunch) return false;

    if (!isAdmin()) {
        logLine(L"ERROR", L"Administrator context is required to create the TrustedInstaller scheduled task.");
        return false;
    }

    logLine(L"INFO", L"TrustedInstaller context required for destructive execution. Attempting automatic TI scheduled-task relaunch via COM API.");

    std::wstring taskName = L"NvDebloatTI-" + std::to_wstring(GetCurrentProcessId());
    fs::path statusPath = g_state.exeDir / (taskName + L"-status.json");
    fs::path childLogDir = g_state.exeDir;

    std::vector<std::wstring> childArgs;
    auto original = getCurrentArgsNoExe();
    for (const auto& a : original) childArgs.push_back(a);
    childArgs.push_back(L"--ti-child");
    childArgs.push_back(L"--no-menu");
    childArgs.push_back(L"--status-file");
    childArgs.push_back(statusPath.wstring());
    childArgs.push_back(L"--log-dir");
    childArgs.push_back(childLogDir.wstring());

    // --- COM: ITaskService --------------------------------------------------
    HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(hr)) { logLine(L"ERROR", L"CoInitializeEx failed: 0x" + std::to_wstring(hr)); return false; }

    ITaskService* pService = nullptr;
    hr = CoCreateInstance(CLSID_TaskScheduler, nullptr, CLSCTX_INPROC_SERVER,
                          IID_ITaskService, reinterpret_cast<void**>(&pService));
    if (FAILED(hr) || !pService) {
        logLine(L"ERROR", L"CoCreateInstance(ITaskService) failed: 0x" + std::to_wstring(hr));
        CoUninitialize(); return false;
    }

    hr = pService->Connect(_variant_t(), _variant_t(), _variant_t(), _variant_t());
    if (FAILED(hr)) {
        logLine(L"ERROR", L"ITaskService::Connect failed: 0x" + std::to_wstring(hr));
        pService->Release(); CoUninitialize(); return false;
    }

    ITaskFolder* pRootFolder = nullptr;
    hr = pService->GetFolder(_bstr_t(L"\\"), &pRootFolder);
    if (FAILED(hr) || !pRootFolder) {
        logLine(L"ERROR", L"GetFolder(\\\\) failed: 0x" + std::to_wstring(hr));
        pService->Release(); CoUninitialize(); return false;
    }

    // Remove previous instance if present
    {
        IRegisteredTask* pReg = nullptr;
        hr = pRootFolder->GetTask(_bstr_t(taskName.c_str()), &pReg);
        if (SUCCEEDED(hr) && pReg) {
            pReg->Stop(0); pReg->Release();
        }
        pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
    }

    // Create task definition
    ITaskDefinition* pDef = nullptr;
    hr = pService->NewTask(0, &pDef);
    if (FAILED(hr) || !pDef) {
        logLine(L"ERROR", L"NewTask failed: 0x" + std::to_wstring(hr));
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return false;
    }

    // --- Registration info ---
    IRegistrationInfo* pRegInfo = nullptr;
    pDef->get_RegistrationInfo(&pRegInfo);
    if (pRegInfo) {
        pRegInfo->put_Author(_bstr_t(L"NT AUTHORITY\\SYSTEM"));
        pRegInfo->Release();
    }

    // --- Principal ---
    IPrincipal* pPrincipal = nullptr;
    pDef->get_Principal(&pPrincipal);
    if (pPrincipal) {
        pPrincipal->put_UserId(_bstr_t(L"NT SERVICE\\TrustedInstaller"));
        pPrincipal->put_LogonType(TASK_LOGON_S4U);
        pPrincipal->put_RunLevel(TASK_RUNLEVEL_HIGHEST);
        pPrincipal->Release();
    }

    // --- Settings ---
    ITaskSettings* pSettings = nullptr;
    pDef->get_Settings(&pSettings);
    if (pSettings) {
        pSettings->put_Enabled(VARIANT_TRUE);
        pSettings->put_AllowDemandStart(VARIANT_TRUE);
        pSettings->put_DisallowStartIfOnBatteries(VARIANT_FALSE);
        pSettings->put_StopIfGoingOnBatteries(VARIANT_FALSE);
        pSettings->put_MultipleInstances(TASK_INSTANCES_IGNORE_NEW);
        pSettings->Release();
    }

    // --- Action (exec) ---
    IActionCollection* pActions = nullptr;
    pDef->get_Actions(&pActions);
    if (pActions) {
        IAction* pAction = nullptr;
        hr = pActions->Create(TASK_ACTION_EXEC, &pAction);
        if (SUCCEEDED(hr) && pAction) {
            IExecAction* pExec = nullptr;
            hr = pAction->QueryInterface(IID_IExecAction, reinterpret_cast<void**>(&pExec));
            if (SUCCEEDED(hr) && pExec) {
                pExec->put_Path(_bstr_t(g_state.exePath.wstring().c_str()));
                pExec->put_Arguments(_bstr_t(joinCommand(childArgs).c_str()));
                pExec->Release();
            }
            pAction->Release();
        }
        pActions->Release();
    }

    // --- Register (create) ---
    IRegisteredTask* pRegistered = nullptr;
    hr = pRootFolder->RegisterTaskDefinition(
        _bstr_t(taskName.c_str()),
        pDef,
        TASK_CREATE_OR_UPDATE,
        _variant_t(L"NT SERVICE\\TrustedInstaller"),
        _variant_t(),
        TASK_LOGON_S4U,
        _variant_t(L""),
        &pRegistered);
    pDef->Release();

    if (FAILED(hr) || !pRegistered) {
        logLine(L"ERROR", L"RegisterTaskDefinition failed: 0x" + std::to_wstring(hr));
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return false;
    }
    logLine(L"INFO", L"Task registered: " + taskName);

    // --- Run ---
    IRunningTask* pRunning = nullptr;
    hr = pRegistered->Run(_variant_t(), &pRunning);
    if (FAILED(hr)) {
        logLine(L"ERROR", L"Run task failed: 0x" + std::to_wstring(hr));
        pRegistered->Stop(0); pRegistered->Release();
        pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return false;
    }
    if (pRunning) pRunning->Release();
    pRegistered->Release();
    logLine(L"INFO", L"Task launched. Waiting for child status file: " + statusPath.wstring());

    // --- Wait for status file ---
    int waited = 0;
    while (waited < g_options.tiWaitSeconds) {
        if (fs::exists(statusPath)) {
            logLine(L"INFO", L"Child status file detected: " + statusPath.wstring());
            std::ifstream in(statusPath, std::ios::binary);
            std::string content((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
            logLine(L"INFO", L"Child status: " + widen(content));
            pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
            pRootFolder->Release(); pService->Release(); CoUninitialize();
            return true;
        }
        if (waited % 15 == 0) {
            logLine(L"INFO", L"Still waiting for TI child... (" + std::to_wstring(waited) + L"s)");
        }
        std::this_thread::sleep_for(std::chrono::seconds(3));
        waited += 3;
    }

    logLine(L"ERROR", L"Timed out waiting for TI child status after " + std::to_wstring(g_options.tiWaitSeconds) + L" seconds.");
    pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
    pRootFolder->Release(); pService->Release(); CoUninitialize();
    return false;
}

// -----------------------------
// NVIDIA matching helpers
// -----------------------------

bool isNvidiaContextString(const std::wstring& s) {
    std::wstring l = toLower(s);
    return l.find(L"nvidia") != std::wstring::npos ||
           l.find(L"geforce") != std::wstring::npos ||
           l.find(L"nv") != std::wstring::npos ||
           l.find(L"frameview") != std::wstring::npos ||
           l.find(L"shadowplay") != std::wstring::npos ||
           l.find(L"displaydriverras") != std::wstring::npos;
}

bool isPreservedContainerName(const std::wstring& nameOrPath) {
    std::wstring l = toLower(nameOrPath);
    return l.find(L"nvdisplay.container") != std::wstring::npos ||
           l.find(L"nvcontainer.exe") != std::wstring::npos ||
           l.find(L"nvdisplay.container.exe") != std::wstring::npos;
}

bool isBloatProcessName(const std::wstring& exe) {
    std::wstring l = toLower(exe);
    static const std::vector<std::wstring> names = {
        L"nvtelemetrycontainer.exe", L"nvidia share.exe", L"nvidia web helper.exe",
        L"nvidia app.exe", L"nvbackend.exe", L"nvnodejslauncher.exe", L"nvsphelper64.exe",
        L"nvstreamservice.exe", L"nvstreamnetworkservice.exe", L"nvstreamuseragent.exe",
        L"nvidia overlay.exe", L"nvidia broadcast.exe", L"frameview.exe", L"presentmon.exe"
    };
    for (const auto& n : names) if (l == n) return true;
    return containsNoCase(exe, L"telemetry") || containsNoCase(exe, L"shadowplay") || containsNoCase(exe, L"frameview");
}

bool moduleIsTelemetryOrUpdater(const std::wstring& path) {
    std::wstring leaf = fs::path(path).filename().wstring();
    static const std::vector<std::wstring> globs = {
        L"*Telemetry*", L"_DisplayDriverRAS.dll", L"_NvMsgBusBroadcast.dll", L"_nvtopps.dll",
        L"_NvGSTPlugin.dll", L"nvprofileupdaterplugin.dll", L"*ProfileUpdater*", L"*DriverUpdate*"
    };
    for (const auto& g : globs) if (wildcardMatchNoCase(leaf, g)) return true;
    return false;
}


bool isDriverStoreSidecarIni(const fs::path& p) {
    std::wstring full = toLower(p.wstring());
    std::wstring leaf = toLower(p.filename().wstring());
    return full.find(L"\\windows\\system32\\driverstore\\filerepository\\") != std::wstring::npos &&
    leaf.find(L".inf_amd64") != std::wstring::npos &&
    leaf.size() >= 4 &&
    leaf.substr(leaf.size() - 4) == L".ini";
}

bool isDriverStorePackageRootName(const fs::path& p) {
    std::wstring parent = toLower(p.parent_path().wstring());
    std::wstring leaf = toLower(p.filename().wstring());
    return parent.find(L"\\windows\\system32\\driverstore\\filerepository") != std::wstring::npos &&
    leaf.find(L".inf_amd64") != std::wstring::npos;
}

bool isDeveloperToolPath(const fs::path& p) {
    std::wstring full = toLower(p.wstring());
    return full.find(L"\\nvidia gpu computing toolkit\\") != std::wstring::npos ||
    full.find(L"\\cuda\\") != std::wstring::npos ||
    full.find(L"\\nsight compute") != std::wstring::npos ||
    full.find(L"\\nsight systems") != std::wstring::npos;
}

bool isNgxPathWithoutNgxSelection(const fs::path& p) {
    std::wstring full = toLower(p.wstring());
    if (full.find(L"\\programdata\\nvidia\\ngx\\") == std::wstring::npos &&
        full.find(L"\\nvidia corporation\\ngx\\") == std::wstring::npos) {
        return false;
        }
        auto it = g_componentEnabled.find(L"NGX");
    return it == g_componentEnabled.end() || !it->second;
}

bool isCoreDisplayDriverLeaf(const fs::path& p) {
    std::wstring leaf = toLower(p.filename().wstring());
    static const std::set<std::wstring> coreLeaves = {
        L"nvwgf2umx.dll", L"nvwgf2um.dll",
        L"nvd3dumx.dll", L"nvd3dum.dll",
        L"nvldumdx.dll", L"nvapi.dll", L"nvapi64.dll",
        L"nvlddmkm.sys", L"nvdispig.inf"
    };
    return coreLeaves.find(leaf) != coreLeaves.end();
}

bool isExcludedCandidatePath(const fs::path& p) {
    return isDeveloperToolPath(p) ||
    isDriverStorePackageRootName(p) ||
    isDriverStoreSidecarIni(p) ||
    isNgxPathWithoutNgxSelection(p) ||
    isCoreDisplayDriverLeaf(p);
}

bool shouldPruneTraversal(const fs::path& p) {
    return isDeveloperToolPath(p) || isNgxPathWithoutNgxSelection(p);
}

std::optional<Component> matchComponentForPath(const fs::path& p, bool isDir) {
    if (isExcludedCandidatePath(p)) return std::nullopt;

    std::wstring leaf = p.filename().wstring();
    std::wstring full = p.wstring();
    auto comps = buildComponents();

    for (const auto& c : comps) {
        auto it = g_componentEnabled.find(c.key);
        if (it == g_componentEnabled.end() || !it->second) continue;

        for (const auto& exact : c.exactLeafNames) {
            if (_wcsicmp(leaf.c_str(), exact.c_str()) == 0) {
                // Installer2 is only safe as NVIDIA Corporation\Installer2, not any arbitrary Installer2.
                if (c.key == L"Installer2Cache" && !containsNoCase(full, L"NVIDIA Corporation")) continue;
                return c;
            }
        }
        for (const auto& glob : c.leafGlobs) {
            if (wildcardMatchNoCase(leaf, glob)) {
                return c;
            }
        }
    }
    (void)isDir;
    return std::nullopt;
}

bool isDriverStorePackageRoot(const fs::path& p) {
    std::wstring parent = toLower(p.parent_path().wstring());
    std::wstring leaf = toLower(p.filename().wstring());
    return parent.find(L"\\windows\\system32\\driverstore\\filerepository") != std::wstring::npos &&
    leaf.find(L".inf_amd64") != std::wstring::npos;
}

bool unsafeRecursiveDirectoryTarget(const fs::path& p) {
    std::error_code dirEc;
    if (!fs::is_directory(p, dirEc)) return false;
    if (isDriverStorePackageRoot(p)) return true;
    std::wstring full = toLower(p.wstring());
    // Do not recursively delete whole driver payload folders except explicit known bloat folders.
    if (full.find(L"\\windows\\system32\\driverstore\\filerepository\\") != std::wstring::npos) {
        std::wstring leaf = toLower(p.filename().wstring());
        static const std::set<std::wstring> allowedDriverStoreDirs = {
            L"nvcamera", L"nvwmi", L"ansel", L"display.update", L"update.core"
        };
        if (allowedDriverStoreDirs.find(leaf) == allowedDriverStoreDirs.end()) return true;
    }
    return false;
}

std::vector<fs::path> getExistingRoots() {
    std::vector<fs::path> roots;
    auto addEnvRoot = [&](const wchar_t* env, const std::wstring& suffix) {
        wchar_t buf[32767];
        DWORD len = GetEnvironmentVariableW(env, buf, 32767);
        if (len > 0 && len < 32767) {
            fs::path p = fs::path(buf) / suffix;
            if (fs::exists(p)) roots.push_back(p);
        }
    };

    addEnvRoot(L"ProgramFiles", L"NVIDIA Corporation");
    addEnvRoot(L"ProgramFiles(x86)", L"NVIDIA Corporation");
    addEnvRoot(L"ProgramFiles", L"NVIDIA Corporation\\Installer2");
    // CUDA Toolkit is developer tooling, not NVIDIA driver bloat. Do not traverse it by default.
    addEnvRoot(L"ProgramData", L"NVIDIA");
    addEnvRoot(L"ProgramData", L"NVIDIA Corporation");

    wchar_t systemRoot[32767];
    DWORD srLen = GetEnvironmentVariableW(L"SystemRoot", systemRoot, 32767);
    if (srLen > 0 && srLen < 32767) {
        fs::path repo = fs::path(systemRoot) / L"System32" / L"DriverStore" / L"FileRepository";
        std::error_code ec;
        if (fs::exists(repo, ec)) {
            for (auto it = fs::directory_iterator(repo, fs::directory_options::skip_permission_denied, ec);
                 !ec && it != fs::directory_iterator(); it.increment(ec)) {
                std::wstring leaf = toLower(it->path().filename().wstring());
            if (leaf.rfind(L"nv", 0) == 0 && leaf.find(L".inf_amd64") != std::wstring::npos) {
                roots.push_back(it->path());
            }
                 }
        }
    }

    fs::path users = L"C:\\Users";
    std::error_code ec;
    if (fs::exists(users, ec)) {
        for (auto it = fs::directory_iterator(users, fs::directory_options::skip_permission_denied, ec);
             !ec && it != fs::directory_iterator(); it.increment(ec)) {
            if (!it->is_directory(ec)) continue;
            std::wstring name = toLower(it->path().filename().wstring());
            if (name == L"default" || name == L"default user" || name == L"public" || name == L"all users") continue;
            std::vector<fs::path> userRoots = {
                it->path() / L"AppData" / L"Local" / L"NVIDIA",
                it->path() / L"AppData" / L"Local" / L"NVIDIA Corporation",
                it->path() / L"AppData" / L"Roaming" / L"NVIDIA",
                it->path() / L"AppData" / L"Roaming" / L"NVIDIA Corporation"
            };
        for (const auto& p : userRoots) if (fs::exists(p, ec)) roots.push_back(p);
             }
    }

    std::sort(roots.begin(), roots.end());
    roots.erase(std::unique(roots.begin(), roots.end()), roots.end());
    return roots;
}

bool isParentOfOrEqual(const fs::path& parent, const fs::path& child) {
    auto p = fs::weakly_canonical(parent).wstring();
    auto c = fs::weakly_canonical(child).wstring();
    p = toLower(p); c = toLower(c);
    if (p == c) return true;
    if (!p.empty() && p.back() != L'\\') p += L"\\";
    return c.rfind(p, 0) == 0;
}

void collapseNestedCandidates(std::vector<Candidate>& candidates) {
    std::sort(candidates.begin(), candidates.end(), [](const Candidate& a, const Candidate& b) {
        auto as = a.path.wstring(), bs = b.path.wstring();
        if (as.size() != bs.size()) return as.size() < bs.size();
        return as < bs;
    });
    std::vector<Candidate> out;
    for (const auto& c : candidates) {
        bool nested = false;
        for (const auto& existing : out) {
            if (existing.isDirectory && isParentOfOrEqual(existing.path, c.path)) {
                nested = true; break;
            }
        }
        if (!nested) out.push_back(c);
    }
    candidates.swap(out);
}

void discoverCandidates() {
    auto roots = getExistingRoots();
    std::wstring rootsLine;
    for (size_t i = 0; i < roots.size(); ++i) {
        if (i) rootsLine += L"; ";
        rootsLine += roots[i].wstring();
    }
    logLine(L"INFO", L"Search roots: " + rootsLine);

    std::set<std::wstring> seen;
    for (const auto& root : roots) {
        std::error_code ec;
        auto processPath = [&](const fs::path& p) {
            std::error_code innerEc;
            bool isDir = fs::is_directory(p, innerEc);
            auto comp = matchComponentForPath(p, isDir);
            if (!comp) return;
            std::wstring canon = toLower(fs::weakly_canonical(p, innerEc).wstring());
            if (canon.empty()) canon = toLower(p.wstring());
            if (seen.insert(canon).second) {
                g_state.candidates.push_back({p, comp->key, comp->displayName, isDir});
            }
        };

        processPath(root);
        if (fs::is_directory(root, ec)) {
            fs::recursive_directory_iterator it(root, fs::directory_options::skip_permission_denied, ec), end;
            while (!ec && it != end) {
                const fs::path current = it->path();
                if (shouldPruneTraversal(current)) {
                    std::error_code dirEc;
                    if (fs::is_directory(current, dirEc)) it.disable_recursion_pending();
                    it.increment(ec);
                    continue;
                }
                processPath(current);
                it.increment(ec);
            }
            if (ec) logLine(L"WARN", L"Traversal warning for " + root.wstring() + L": " + widen(ec.message()));
        }
    }
    collapseNestedCandidates(g_state.candidates);
    logLine(L"INFO", L"Candidate count: " + std::to_wstring(g_state.candidates.size()));
}

void writeCandidatesCsv() {
    std::wstringstream ss;
    ss << L"component,path,type\n";
    for (const auto& c : g_state.candidates) {
        ss << L"\"" << c.componentKey << L"\",\"" << c.path.wstring() << L"\",\"" << (c.isDirectory ? L"directory" : L"file") << L"\"\n";
    }
    HANDLE h = CreateFileW(g_state.candidateCsvPath.wstring().c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h != INVALID_HANDLE_VALUE) {
        std::string bytes = utf8(ss.str());
        DWORD written = 0;
        WriteFile(h, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr);
        CloseHandle(h);
    }
    logLine(L"INFO", L"Candidate CSV: " + g_state.candidateCsvPath.wstring());
}

// -----------------------------
// Process/service/task handling
// -----------------------------

void inspectNvContainerModules() {
    logLine(L"INFO", L"Inspecting NVIDIA Container processes and loaded telemetry/profile-update modules.");
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snap == INVALID_HANDLE_VALUE) {
        logLine(L"WARN", L"CreateToolhelp32Snapshot(process) failed: " + formatWinError(GetLastError()));
        return;
    }
    PROCESSENTRY32W pe{};
    pe.dwSize = sizeof(pe);
    if (Process32FirstW(snap, &pe)) {
        do {
            std::wstring exe = pe.szExeFile;
            if (!isPreservedContainerName(exe)) continue;
            logLine(L"INFO", L"Container running: PID=" + std::to_wstring(pe.th32ProcessID) + L" Name=" + exe);
            addAction(L"ContainerProcess", L"Running", L"NvContainer", exe, L"PID=" + std::to_wstring(pe.th32ProcessID));

            HANDLE modSnap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pe.th32ProcessID);
            if (modSnap == INVALID_HANDLE_VALUE) {
                logLine(L"WARN", L"Module inspection failed for PID=" + std::to_wstring(pe.th32ProcessID) + L": " + formatWinError(GetLastError()));
                continue;
            }
            MODULEENTRY32W me{};
            me.dwSize = sizeof(me);
            if (Module32FirstW(modSnap, &me)) {
                do {
                    std::wstring path = me.szExePath;
                    if (moduleIsTelemetryOrUpdater(path)) {
                        logLine(L"WARN", L"Container has matching module loaded; deletion may require reboot: PID=" + std::to_wstring(pe.th32ProcessID) + L" Module=" + path);
                        addAction(L"LoadedTelemetryModule", L"Loaded", L"NvContainer", path, L"PID=" + std::to_wstring(pe.th32ProcessID));
                    }
                } while (Module32NextW(modSnap, &me));
            }
            CloseHandle(modSnap);
        } while (Process32NextW(snap, &pe));
    }
    CloseHandle(snap);
}

void killLockerProcesses() {
    if (!g_options.killLockers) {
        logLine(L"INFO", L"--kill-lockers not specified; process stopping skipped.");
        return;
    }
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snap == INVALID_HANDLE_VALUE) {
        logLine(L"WARN", L"Process snapshot failed: " + formatWinError(GetLastError()));
        return;
    }
    PROCESSENTRY32W pe{};
    pe.dwSize = sizeof(pe);
    if (Process32FirstW(snap, &pe)) {
        do {
            std::wstring exe = pe.szExeFile;
            bool isContainer = isPreservedContainerName(exe);
            if (isContainer && g_options.preserveNvContainers) continue;
            if (!isBloatProcessName(exe)) continue;
            if (!g_options.execute) {
                logAction(L"KillProcess", L"DRYRUN", L"Process", exe, L"PID=" + std::to_wstring(pe.th32ProcessID));
                continue;
            }
            HANDLE h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pe.th32ProcessID);
            if (!h) {
                logAction(L"KillProcess", L"WARN", L"Process", exe, L"OpenProcess failed: " + formatWinError(GetLastError()));
                continue;
            }
            BOOL ok = TerminateProcess(h, 0);
            CloseHandle(h);
            logAction(L"KillProcess", ok ? L"INFO" : L"WARN", L"Process", exe, ok ? L"Terminated" : formatWinError(GetLastError()));
        } while (Process32NextW(snap, &pe));
    }
    CloseHandle(snap);
}

bool serviceMatchesBloat(const std::wstring& serviceName, const std::wstring& displayName) {
    std::wstring combined = serviceName + L" " + displayName;
    if (g_options.preserveNvContainers && isPreservedContainerName(combined)) return false;
    if (!isNvidiaContextString(combined)) return false;
    static const std::vector<std::wstring> terms = {
        L"telemetry", L"displaydriverras", L"update", L"profileupdater", L"frameview", L"nvstream",
        L"shadowplay", L"share", L"ansel", L"nvidia app", L"geforce experience", L"broadcast"
    };
    for (const auto& t : terms) if (containsNoCase(combined, t)) return true;
    auto va = g_componentEnabled.find(L"VirtualAudio");
    if (va != g_componentEnabled.end() && va->second && containsNoCase(combined, L"nvvad")) return true;
    return false;
}

void handleServices() {
    if (!g_options.killLockers && !g_options.disableServices && !g_options.deleteServices) {
        logLine(L"INFO", L"Service stopping/disable/delete skipped.");
        return;
    }

    SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT);
    if (!scm) {
        logLine(L"WARN", L"OpenSCManager failed: " + formatWinError(GetLastError()));
        return;
    }

    DWORD bytesNeeded = 0, count = 0, resume = 0;
    EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, nullptr, 0, &bytesNeeded, &count, &resume, nullptr);
    std::vector<BYTE> buf(bytesNeeded + 4096);
    if (!EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.data(), static_cast<DWORD>(buf.size()), &bytesNeeded, &count, &resume, nullptr)) {
        logLine(L"WARN", L"EnumServicesStatusEx failed: " + formatWinError(GetLastError()));
        CloseServiceHandle(scm);
        return;
    }

    auto services = reinterpret_cast<ENUM_SERVICE_STATUS_PROCESSW*>(buf.data());
    for (DWORD i = 0; i < count; ++i) {
        std::wstring name = services[i].lpServiceName ? services[i].lpServiceName : L"";
        std::wstring disp = services[i].lpDisplayName ? services[i].lpDisplayName : L"";
        if (!serviceMatchesBloat(name, disp)) continue;

        std::wstring full = name + L" (" + disp + L")";
        if (!g_options.execute) {
            if (g_options.killLockers) logAction(L"StopService", L"DRYRUN", L"Service", full);
            if (g_options.disableServices) logAction(L"DisableService", L"DRYRUN", L"Service", full);
            if (g_options.deleteServices) logAction(L"DeleteService", L"DRYRUN", L"Service", full);
            continue;
        }

        SC_HANDLE svc = OpenServiceW(scm, name.c_str(), SERVICE_STOP | SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG | DELETE);
        if (!svc) {
            logAction(L"Service", L"WARN", L"Service", full, L"OpenService failed: " + formatWinError(GetLastError()));
            continue;
        }

        if (g_options.killLockers) {
            SERVICE_STATUS status{};
            BOOL ok = ControlService(svc, SERVICE_CONTROL_STOP, &status);
            DWORD err = GetLastError();
            logAction(L"StopService", ok || err == ERROR_SERVICE_NOT_ACTIVE ? L"INFO" : L"WARN", L"Service", full,
                      ok ? L"Stop requested" : formatWinError(err));
        }

        if (g_options.disableServices) {
            BOOL ok = ChangeServiceConfigW(svc, SERVICE_NO_CHANGE, SERVICE_DISABLED, SERVICE_NO_CHANGE,
                                           nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr);
            logAction(L"DisableService", ok ? L"INFO" : L"WARN", L"Service", full, ok ? L"Disabled" : formatWinError(GetLastError()));
        }

        if (g_options.deleteServices) {
            BOOL ok = DeleteService(svc);
            logAction(L"DeleteService", ok ? L"INFO" : L"WARN", L"Service", full, ok ? L"Deleted" : formatWinError(GetLastError()));
        }
        CloseServiceHandle(svc);
    }
    CloseServiceHandle(scm);
}

std::vector<std::wstring> parseCsvLine(const std::wstring& line) {
    std::vector<std::wstring> fields;
    std::wstring cur;
    bool inQuotes = false;
    for (size_t i = 0; i < line.size(); ++i) {
        wchar_t c = line[i];
        if (c == L'\"') {
            if (inQuotes && i + 1 < line.size() && line[i + 1] == L'\"') { cur.push_back(L'\"'); ++i; }
            else inQuotes = !inQuotes;
        } else if (c == L',' && !inQuotes) {
            fields.push_back(cur); cur.clear();
        } else {
            cur.push_back(c);
        }
    }
    fields.push_back(cur);
    return fields;
}

bool taskMatchesBloat(const std::wstring& taskName) {
    std::wstring l = toLower(taskName);
    if (!(l.find(L"nvidia") != std::wstring::npos || l.find(L"geforce") != std::wstring::npos ||
        l.find(L"nv") != std::wstring::npos || l.find(L"frameview") != std::wstring::npos ||
        l.find(L"shadowplay") != std::wstring::npos || l.find(L"displaydriverras") != std::wstring::npos)) {
        return false;
        }
        static const std::vector<std::wstring> terms = {
            L"telemetry", L"update", L"profile", L"frameview", L"shadowplay", L"share", L"nvstream", L"geforce", L"nvidia app", L"displaydriverras"
        };
    for (const auto& t : terms) if (l.find(t) != std::wstring::npos) return true;
    return false;
}

void handleScheduledTasks() {
    if (!g_options.disableScheduledTasks && !g_options.deleteScheduledTasks) {
        logLine(L"INFO", L"Scheduled task handling skipped.");
        return;
    }
    auto res = runProcessCapture(L"schtasks.exe /Query /FO CSV /NH", 120000);
    if (res.exitCode != 0) {
        logLine(L"WARN", L"schtasks query failed: " + trim(res.output));
        return;
    }
    std::wstringstream input(res.output);
    std::wstring line;
    while (std::getline(input, line)) {
        line = trim(line);
        if (line.empty()) continue;
        auto fields = parseCsvLine(line);
        if (fields.empty()) continue;
        std::wstring taskName = fields[0];
        if (!taskMatchesBloat(taskName)) continue;

        if (g_options.deleteScheduledTasks) {
            if (!g_options.execute) logAction(L"DeleteTask", L"DRYRUN", L"ScheduledTask", taskName);
            else {
                auto del = runProcessCapture(joinCommand({L"schtasks.exe", L"/Delete", L"/TN", taskName, L"/F"}), 60000);
                logAction(L"DeleteTask", del.exitCode == 0 ? L"INFO" : L"WARN", L"ScheduledTask", taskName, trim(del.output));
            }
        } else if (g_options.disableScheduledTasks) {
            if (!g_options.execute) logAction(L"DisableTask", L"DRYRUN", L"ScheduledTask", taskName);
            else {
                auto dis = runProcessCapture(joinCommand({L"schtasks.exe", L"/Change", L"/TN", taskName, L"/Disable"}), 60000);
                logAction(L"DisableTask", dis.exitCode == 0 ? L"INFO" : L"WARN", L"ScheduledTask", taskName, trim(dis.output));
            }
        }
    }
}

// -----------------------------
// Deletion helpers
// -----------------------------

void takeOwnershipIfRequested(const fs::path& p) {
    if (!g_options.takeOwnership) return;
    std::wstring target = p.wstring();
    runShellCommand(joinCommand({L"takeown.exe", L"/F", target, L"/A", L"/R", L"/D", L"Y"}), L"takeown", true);
    runShellCommand(joinCommand({L"icacls.exe", target, L"/grant", L"Administrators:F", L"/T", L"/C"}), L"icacls", true);
}

bool scheduleDeleteOne(const fs::path& p) {
    BOOL ok = MoveFileExW(p.wstring().c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
    if (!ok) {
        logAction(L"ScheduleDelete", L"WARN", L"RebootDelete", p.wstring(), formatWinError(GetLastError()));
        return false;
    }
    logAction(L"ScheduleDelete", L"INFO", L"RebootDelete", p.wstring(), L"Pending delete at reboot");
    return true;
}

bool scheduleDeleteTree(const fs::path& p) {
    bool allOk = true;
    std::error_code ec;
    if (fs::is_directory(p, ec)) {
        std::vector<fs::path> paths;
        fs::recursive_directory_iterator it(p, fs::directory_options::skip_permission_denied, ec), end;
        while (!ec && it != end) {
            paths.push_back(it->path());
            it.increment(ec);
        }
        std::sort(paths.begin(), paths.end(), [](const fs::path& a, const fs::path& b) {
            return a.wstring().size() > b.wstring().size();
        });
        for (const auto& child : paths) allOk = scheduleDeleteOne(child) && allOk;
    }
    allOk = scheduleDeleteOne(p) && allOk;
    return allOk;
}

void deleteCandidate(const Candidate& c) {
    if (unsafeRecursiveDirectoryTarget(c.path)) {
        logAction(L"DeletePath", L"SKIP", c.componentKey, c.path.wstring(), L"Unsafe recursive directory target");
        return;
    }

    if (!g_options.execute) {
        logAction(L"DeletePath", L"DRYRUN", c.componentKey, c.path.wstring(), c.isDirectory ? L"directory" : L"file");
        return;
    }

    std::error_code ec;
    if (!fs::exists(c.path, ec)) {
        logAction(L"DeletePath", L"SKIP", c.componentKey, c.path.wstring(), L"Path no longer exists");
        return;
    }

    takeOwnershipIfRequested(c.path);

    if (c.isDirectory) {
        uintmax_t removed = fs::remove_all(c.path, ec);
        if (!ec) {
            logAction(L"DeletePath", L"INFO", c.componentKey, c.path.wstring(), L"Removed entries=" + std::to_wstring(removed));
            return;
        }
    } else {
        bool ok = DeleteFileW(c.path.wstring().c_str()) == TRUE;
        if (ok) {
            logAction(L"DeletePath", L"INFO", c.componentKey, c.path.wstring(), L"Deleted file");
            return;
        }
        ec = std::error_code(GetLastError(), std::system_category());
    }

    std::wstring detail = L"Delete failed";
    if (ec) detail += L": " + widen(ec.message());
    logAction(L"DeletePath", L"WARN", c.componentKey, c.path.wstring(), detail);

    if (g_options.scheduleLockedForReboot) {
        scheduleDeleteTree(c.path);
    }
}

// -----------------------------
// Reports
// -----------------------------

void writeReport() {
    std::map<std::wstring, int> byStatus, byType;
    for (const auto& a : g_state.actions) { byStatus[a.status]++; byType[a.type]++; }

    std::wstringstream ss;
    ss << L"{\n";
    ss << L"  \"runId\": \"" << jsonEscape(g_state.runId) << L"\",\n";
    ss << L"  \"exePath\": \"" << jsonEscape(g_state.exePath.wstring()) << L"\",\n";
    ss << L"  \"identity\": \"" << jsonEscape(currentTokenAccount()) << L"\",\n";
    ss << L"  \"isAdmin\": " << (isAdmin() ? L"true" : L"false") << L",\n";
    ss << L"  \"isTrustedInstaller\": " << (isTrustedInstaller() ? L"true" : L"false") << L",\n";
    ss << L"  \"execute\": " << (g_options.execute ? L"true" : L"false") << L",\n";
    ss << L"  \"candidateCount\": " << g_state.candidates.size() << L",\n";

    ss << L"  \"summaryByStatus\": {";
    bool first = true;
    for (const auto& kv : byStatus) { if (!first) ss << L", "; first = false; ss << L"\"" << jsonEscape(kv.first) << L"\": " << kv.second; }
    ss << L"},\n";
    ss << L"  \"summaryByType\": {";
    first = true;
    for (const auto& kv : byType) { if (!first) ss << L", "; first = false; ss << L"\"" << jsonEscape(kv.first) << L"\": " << kv.second; }
    ss << L"},\n";

    ss << L"  \"actions\": [\n";
    for (size_t i = 0; i < g_state.actions.size(); ++i) {
        const auto& a = g_state.actions[i];
        ss << L"    {\"type\": \"" << jsonEscape(a.type) << L"\", \"status\": \"" << jsonEscape(a.status)
        << L"\", \"component\": \"" << jsonEscape(a.component) << L"\", \"path\": \"" << jsonEscape(a.path)
        << L"\", \"detail\": \"" << jsonEscape(a.detail) << L"\"}" << (i + 1 < g_state.actions.size() ? L"," : L"") << L"\n";
    }
    ss << L"  ]\n";
    ss << L"}\n";

    HANDLE h = CreateFileW(g_state.reportPath.wstring().c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h != INVALID_HANDLE_VALUE) {
        std::string bytes = utf8(ss.str());
        DWORD written = 0;
        WriteFile(h, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr);
        CloseHandle(h);
    }

    std::wstring statusSummary;
    for (const auto& kv : byStatus) {
        if (!statusSummary.empty()) statusSummary += L", ";
        statusSummary += kv.first + L"=" + std::to_wstring(kv.second);
    }
    logLine(L"INFO", L"Action summary by status: " + statusSummary);
    logLine(L"INFO", L"Report: " + g_state.reportPath.wstring());
}

// -----------------------------
// Main cleanup flow
// -----------------------------

void runCleanup() {
    logLine(L"INFO", L"==== NVIDIA post-install debloat run header ====");
    logLine(L"INFO", L"RunId: " + g_state.runId);
    logLine(L"INFO", L"Exe: " + g_state.exePath.wstring());
    logLine(L"INFO", L"Log: " + g_state.logPath.wstring());
    logLine(L"INFO", L"Identity: " + currentTokenAccount());
    logLine(L"INFO", L"IsAdmin: " + std::wstring(isAdmin() ? L"True" : L"False"));
    logLine(L"INFO", L"IsTrustedInstaller: " + std::wstring(isTrustedInstaller() ? L"True" : L"False"));
    logLine(L"INFO", L"Execution mode: " + std::wstring(g_options.execute ? L"EXECUTE" : L"DRY RUN"));
    if (!g_options.execute) logLine(L"INFO", L"Dry run does not request TrustedInstaller; it only scans and logs as the current user/admin.");
    if (g_options.execute && !g_options.tiChild && !g_options.allowAdminFallback && !isTrustedInstaller()) logLine(L"INFO", L"Execute mode will attempt TrustedInstaller scheduled-task relaunch before destructive actions.");
    logLine(L"INFO", L"Preserve NVIDIA Container processes/services: " + std::wstring(g_options.preserveNvContainers ? L"True" : L"False"));

    if (g_options.execute && !g_options.tiChild && !isTrustedInstaller() && !g_options.allowAdminFallback) {
        throw std::runtime_error("Destructive execution requires TrustedInstaller. Relaunch did not occur or did not enter TI context.");
    }
    if (g_options.tiChild && !isTrustedInstaller()) {
        logLine(L"WARN", L"TI child running as " + currentTokenAccount() + L" instead of TrustedInstaller. SYSTEM-level privileges should suffice for file operations.");
    }

    handleServices();
    killLockerProcesses();
    handleScheduledTasks();
    inspectNvContainerModules();
    discoverCandidates();
    writeCandidatesCsv();

    for (const auto& c : g_state.candidates) {
        deleteCandidate(c);
    }

    inspectNvContainerModules();
    writeReport();
    logLine(L"INFO", L"Done. Log: " + g_state.logPath.wstring());
    if (!g_options.execute) logLine(L"WARN", L"Dry run only. Re-run with --execute to make changes.");
    if (g_options.scheduleLockedForReboot) logLine(L"WARN", L"Some locked-file deletions may require reboot. Review report/log.");
}

int wmain(int argc, wchar_t** argv) {
    int exitCode = 0;
    try {
        g_options = parseArgs(argc, argv);
        initializeComponentSelection();
        applyComponentArgs(argc, argv);
        initializeRunState();

        if (g_options.menu && !g_options.tiChild) {
            interactiveMenu();
        }

        logLine(L"INFO", L"Logging initialized beside executable: " + g_state.logPath.wstring());

        if (g_options.execute && !g_options.tiChild && !g_options.allowAdminFallback && !isTrustedInstaller()) {
            bool relaunched = attemptTrustedInstallerRelaunch(argc, argv);
            if (relaunched) {
                logLine(L"INFO", L"Parent process finished after TI child completion. Review child log/report beside executable.");
                return 0;
            }
            if (!g_options.allowAdminFallback) {
                logLine(L"FATAL", L"TrustedInstaller relaunch failed and --allow-admin-fallback was not specified.");
                writeStatusJson(g_options.statusFile, 10, L"failed", L"TrustedInstaller relaunch failed");
                return 10;
            }
        }

        runCleanup();
        writeStatusJson(g_options.statusFile, 0, L"ok", L"completed");
    } catch (const std::exception& ex) {
        exitCode = 1;
        std::wstring msg = widen(ex.what());
        logLine(L"FATAL", msg);
        try { writeReport(); } catch (...) {}
        writeStatusJson(g_options.statusFile, exitCode, L"fatal", msg);
    } catch (...) {
        exitCode = 2;
        logLine(L"FATAL", L"Unknown fatal exception.");
        try { writeReport(); } catch (...) {}
        writeStatusJson(g_options.statusFile, exitCode, L"fatal", L"unknown fatal exception");
    }

    if (!g_options.noPause && g_options.menu && !g_options.tiChild) {
        std::wcout << L"\nPress Enter to exit...";
        std::wstring dummy;
        std::getline(std::wcin, dummy);
    }
    return exitCode;
}
