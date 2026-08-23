// GreenPostInstallDebloatNative-TI-Menu-Logging-20260702-safe-match-virtualaudio-optin.cpp
//
// Native Windows C++17 NVIDIA post-install debloater.
// Intended compiler targets: MinGW-w64 GCC or LLVM/clang++ for Windows.
//
// Build examples:
//   Preferred: python build.py  (downloads a pinned llvm-mingw toolchain)
//   Manual:
//   clang++ -std=c++17 -municode -O2 -Wall -Wextra GreenPostInstallDebloatNative.cpp -o GreenPostInstallDebloatNative.exe \
//           -ladvapi32 -lshell32 -lole32 -loleaut32 -ltaskschd
//   g++     -std=c++17 -municode -O2 -Wall -Wextra GreenPostInstallDebloatNative.cpp -o GreenPostInstallDebloatNative.exe \
//           -ladvapi32 -lshell32 -lole32 -loleaut32 -ltaskschd
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
//
// Bare launch (no arguments, e.g. double-click in Explorer):
//   Opens the interactive wizard with the recommended cleanup pre-selected
//   (--execute, --kill-lockers, --disable-services, --delete-scheduled-tasks,
//   --schedule-reboot-delete) while keeping UpdateAndProfileUpdater disabled,
//   i.e. the NVIDIA driver-update/profile-updater stack is NOT deleted.
//   Destructive execution still requires interactive confirmation ('y' plus
//   typing EXECUTE) in the wizard; if the session is not elevated, it then
//   relaunches itself via a UAC prompt with the confirmed selection.

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
#include <atomic>
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

#define GPD_VERSION L"1.5.0"

// Exit codes
enum {
    EXIT_OK = 0,
    EXIT_FATAL_EXCEPTION = 1,
    EXIT_FATAL_UNKNOWN = 2,
    EXIT_ABORTED = 3,
    EXIT_TI_RELAUNCH_FAILED = 10,
    EXIT_TI_CHILD_FAILED = 11,
    EXIT_ALREADY_RUNNING = 12
};

static const wchar_t* kTiTaskPrefix = L"NvDebloatTI-";

struct Options {
    bool execute = false;
    bool menu = false;
    bool wizardDefaults = false; // bare launch: wizard presets applied
    bool pauseOnExit = false;    // --pause: force a final "Press Enter"
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
    std::wstring logFileOverride;
    bool showHelp = false;
    bool showVersion = false;
    bool listComponents = false;
    std::vector<std::wstring> unknownArgs;
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
    std::vector<ActionRecord> actions;
    std::vector<Candidate> candidates;
    bool aborted = false;
    // Filled by verifyCandidateRemoval() after processing.
    bool postRunCheckDone = false;
    long pathsRemainingAfterRun = -1;
    long pathsPendingReboot = -1;
    // Filled by tallyPreviousLogs() at start of cleanup.
    bool historyScanDone = false;
    long historyLogFiles = -1;
    long historyRuns = -1;
    long long historyCandidates = -1;
};

static Options g_options;
static RunState g_state;
static std::map<std::wstring, bool> g_componentEnabled;
static std::atomic<bool> g_abortRequested{false};
static bool g_userQuitRequested = false;

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

// Console tools like schtasks.exe / takeown.exe / icacls.exe write their stdout
// pipes in the OEM code page, not ANSI and not UTF-8.  Decoding with CP_ACP
// mangles non-ASCII names (e.g. German 'löschen'), which then breaks /TN lookups.
std::wstring decodeProcessOutput(const std::string& bytes) {
    if (bytes.empty()) return {};
    // Try UTF-8 strictly first (some tools do emit UTF-8).
    int needed = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, bytes.data(), static_cast<int>(bytes.size()), nullptr, 0);
    if (needed > 0) {
        std::wstring out(static_cast<size_t>(needed), L'\0');
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, bytes.data(), static_cast<int>(bytes.size()), out.data(), needed);
        return out;
    }
    UINT oemCp = GetOEMCP();
    needed = MultiByteToWideChar(oemCp, 0, bytes.data(), static_cast<int>(bytes.size()), nullptr, 0);
    if (needed <= 0) return {};
    std::wstring out(static_cast<size_t>(needed), L'\0');
    MultiByteToWideChar(oemCp, 0, bytes.data(), static_cast<int>(bytes.size()), out.data(), needed);
    return out;
}

bool endsWithNoCase(const std::wstring& s, const std::wstring& suffix) {
    if (suffix.size() > s.size()) return false;
    return toLower(s.substr(s.size() - suffix.size())) == toLower(suffix);
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

// Minimal readers for the status JSON this program itself writes.
// Good enough for fixed keys; avoids pulling in a JSON dependency.
std::wstring jsonFindStringField(const std::wstring& json, const std::wstring& key) {
    const std::wstring needle = L"\"" + key + L"\"";
    size_t pos = json.find(needle);
    while (pos != std::wstring::npos) {
        size_t colon = json.find(L':', pos + needle.size());
        if (colon == std::wstring::npos) return L"";
        size_t i = colon + 1;
        while (i < json.size() && std::iswspace(json[i])) ++i;
        if (i < json.size() && json[i] == L'"') {
            std::wstring out;
            ++i;
            while (i < json.size() && json[i] != L'"') {
                if (json[i] == L'\\' && i + 1 < json.size()) {
                    wchar_t n = json[++i];
                    switch (n) {
                        case L'\\': out += L'\\'; break;
                        case L'"': out += L'"'; break;
                        case L'/': out += L'/'; break;
                        case L'b': out += L'\b'; break;
                        case L'f': out += L'\f'; break;
                        case L'n': out += L'\n'; break;
                        case L'r': out += L'\r'; break;
                        case L't': out += L'\t'; break;
                        case L'u':
                            if (i + 4 < json.size()) {
                                out += static_cast<wchar_t>(wcstol(json.substr(i + 1, 4).c_str(), nullptr, 16));
                                i += 4;
                            }
                            break;
                        default: out += n; break;
                    }
                } else {
                    out += json[i];
                }
                ++i;
            }
            return out;
        }
        pos = json.find(needle, colon);
    }
    return L"";
}

long jsonFindIntField(const std::wstring& json, const std::wstring& key, long defaultVal) {
    const std::wstring needle = L"\"" + key + L"\"";
    size_t pos = json.find(needle);
    while (pos != std::wstring::npos) {
        size_t colon = json.find(L':', pos + needle.size());
        if (colon == std::wstring::npos) return defaultVal;
        size_t i = colon + 1;
        while (i < json.size() && std::iswspace(json[i])) ++i;
        if (i < json.size() && (json[i] == L'-' || (json[i] >= L'0' && json[i] <= L'9'))) {
            return wcstol(json.c_str() + i, nullptr, 10);
        }
        pos = json.find(needle, colon);
    }
    return defaultVal;
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
    result.output += decodeProcessOutput(bytes);
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

// Absolute path inside the real System32 directory.  Invoking system tools by
// bare name lets CreateProcess pick up a planted binary from the application
// directory or CWD first -- catastrophic in a SYSTEM-context TI child.
std::wstring systemDirFile(const std::wstring& fileName) {
    wchar_t buf[MAX_PATH];
    UINT n = GetSystemDirectoryW(buf, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) return fileName;
    return (fs::path(buf) / fileName).wstring();
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

const std::vector<Component>& buildComponents() {
    // Constructed once; this is called for every scanned path during discovery.
    static const std::vector<Component> comps = {
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
    return comps;
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
    std::wcout << L"\nNVIDIA post-install debloater native C++ build (version " << GPD_VERSION << L")\n\n"
    << L"Usage:\n"
    << L"  GreenPostInstallDebloatNative.exe                      Bare launch (double-click): wizard with recommended\n"
    << L"                                                         defaults pre-selected (execute, kill-lockers, disable-services,\n"
    << L"                                                         delete-scheduled-tasks, reboot-delete); keeps the\n"
    << L"                                                         NVIDIA profile updater installed. Requires interactive\n"
    << L"                                                         confirmation; elevates via UAC when needed.\n"
    << L"  GreenPostInstallDebloatNative.exe --menu               Interactive wizard with inert dry-run defaults.\n"
    << L"  GreenPostInstallDebloatNative.exe --dry-run            Safe scan without changes.\n"
    << L"  GreenPostInstallDebloatNative.exe --execute --kill-lockers --disable-services --delete-scheduled-tasks --schedule-reboot-delete\n\n"
    << L"Core switches:\n"
    << L"  --execute                       Actually delete/disable. Default is dry-run.\n"
    << L"                                  Exception: a bare launch (no arguments) opens the wizard\n"
    << L"                                  in EXECUTE mode with the recommended switches pre-selected.\n"
    << L"  --dry-run                       Explicit dry-run.\n"
    << L"  --menu                          Interactive component selection.\n"
    << L"  --no-menu                       Suppress the interactive menu.\n"
    << L"  --kill-lockers                  Stop bloat services/processes before deleting.\n"
    << L"  --preserve-nvcontainers[=on/off] Allow killing NVDisplay.Container/nvcontainer if matched. Default preserves them.\n"
    << L"  --disable-services              Disable matching NVIDIA bloat services.\n"
    << L"  --delete-services               Delete matching NVIDIA bloat services.\n"
    << L"  --disable-scheduled-tasks       Disable matching NVIDIA scheduled tasks. Default true.\n"
    << L"  --no-disable-scheduled-tasks    Do not disable matched scheduled tasks.\n"
    << L"  --delete-scheduled-tasks        Delete matching NVIDIA scheduled tasks.\n"
    << L"  --schedule-reboot-delete        Use MoveFileEx for locked files/folders.\n"
    << L"  --take-ownership                Run takeown/icacls before deletion. Usually unnecessary under TrustedInstaller.\n\n"
    << L"TrustedInstaller behavior:\n"
    << L"  --no-ti-relaunch                Do not attempt automatic TrustedInstaller scheduled-task relaunch.\n"
    << L"  --allow-admin-fallback          Permit destructive execution as Administrator if TI relaunch fails/skipped.\n"
    << L"  --ti-wait-seconds N             Parent wait timeout for TI child. Default 600.\n"
    << L"                                  Component selections made in the interactive menu are forwarded to the elevated child.\n\n"
    << L"Optional component inclusions:\n"
    << L"  --include-ngx --include-hdaudio --include-physx --include-notebook-optimus\n"
    << L"  --include-virtual-audio        Include NvVAD/NVIDIA Virtual Audio Device cleanup. Off by default.\n"
    << L"  --include-nvwmi                Include NVIDIA WMI management interface cleanup. Off by default.\n"
    << L"  --include-capture-sdk          Include NvFBC/NvIFR capture SDK runtime cleanup. Off by default.\n\n"
    << L"Component selection:\n"
    << L"  --component=Key:on/off         Toggle a specific component (repeatable).\n"
    << L"  --list-components              Print all component keys and their state, then exit.\n\n"
    << L"Miscellaneous:\n"
    << L"  --status-file PATH             Write child run status JSON to PATH (legacy diagnostics handoff).\n"
    << L"  --log-dir PATH                 Base directory for the log file. Default: beside the executable.\n"
    << L"  --log-file PATH                Use exactly this log file; all processes of a run (launcher, elevated\n"
    << L"                                 instance, SYSTEM worker) append to it, so one run leaves one log.\n"
    << L"  --pause                        Always pause on exit (also used by the elevated\n"
    << L"                                 instance spawned by a bare double-click launch).\n"
    << L"  --no-pause                     Do not pause on exit after an interactive menu run.\n"
    << L"  --no-color                     Monochrome console output.\n"
    << L"  --version                      Print version and exit.\n"
    << L"  --help, -h, /?                 Show this help.\n\n"
    << L"Exit codes:\n"
    << L"  0 success | 1 fatal exception | 2 unknown fatal | 3 aborted by user\n"
    << L"  10 TrustedInstaller relaunch failed/not permitted | 11 elevated child reported failure\n"
    << L"  12 another execute-mode instance is already running\n\n";
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
    if (argc <= 1) {
        // Bare launch (e.g. double-click in Explorer): open the wizard with the
        // recommended cleanup pre-selected instead of an inert dry-run.
        // UpdateAndProfileUpdater is deselected in wmain (wizardDefaults flag).
        opt.menu = true;
        opt.execute = true;
        opt.killLockers = true;
        opt.disableServices = true;
        opt.deleteScheduledTasks = true;
        opt.scheduleLockedForReboot = true;
        opt.wizardDefaults = true;
    }
    for (int i = 1; i < argc; ++i) {
        std::wstring a = argv[i];
        std::wstring low = toLower(a);
        if (low == L"--help" || low == L"-h" || low == L"/?") opt.showHelp = true;
        else if (low == L"--version") opt.showVersion = true;
        else if (low == L"--list-components") opt.listComponents = true;
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
        else if (low == L"--pause") opt.pauseOnExit = true;
        else if (low == L"--no-color") opt.noColor = true;
        else if (parseBoolAssignment(a, L"--preserve-nvcontainers", opt.preserveNvContainers)) {}
        else if (low == L"--preserve-nvcontainers") opt.preserveNvContainers = true;
        else if ((low == L"--status-file" || low == L"--log-dir" || low == L"--ti-wait-seconds" || low == L"--log-file") && i + 1 < argc) {
            std::wstring v = argv[++i];
            if (low == L"--status-file") opt.statusFile = v;
            else if (low == L"--log-dir") opt.logDirOverride = v;
            else if (low == L"--log-file") opt.logFileOverride = v;
            else if (low == L"--ti-wait-seconds") opt.tiWaitSeconds = std::max(15, _wtoi(v.c_str()));
        } else if (a.rfind(L"--ti-wait-seconds=", 0) == 0) {
            opt.tiWaitSeconds = std::max(15, _wtoi(a.substr(std::wstring(L"--ti-wait-seconds=").size()).c_str()));
        } else if (low.rfind(L"--status-file=", 0) == 0) {
            opt.statusFile = a.substr(std::wstring(L"--status-file=").size());
        } else if (low.rfind(L"--log-file=", 0) == 0) {
            opt.logFileOverride = a.substr(std::wstring(L"--log-file=").size());
        } else if (low.rfind(L"--log-dir=", 0) == 0) {
            opt.logDirOverride = a.substr(std::wstring(L"--log-dir=").size());
        } else if (low.rfind(L"--component=", 0) == 0) {
            // Parsed after component map exists. Format: --component=Key:on/off
            continue;
        } else {
            opt.unknownArgs.push_back(a);
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
            bool matched = false;
            for (auto& kv : g_componentEnabled) {
                if (toLower(kv.first) == toLower(key)) {
                    kv.second = on;
                    matched = true;
                }
            }
            if (!matched) {
                std::wstring valid;
                for (const auto& kv : g_componentEnabled) {
                    if (!valid.empty()) valid += L", ";
                    valid += kv.first;
                }
                std::wcerr << L"Warning: unknown component key in '" << a << L"'. Valid keys: " << valid << L"\n";
            }
        }
    }
}

void interactiveMenu() {
    const auto& comps = buildComponents();
    std::wcout << L"\nNVIDIA post-install debloater\n"
    << L"Logs will be written next to this executable.\n"
    << L"Dry-run is recommended before destructive execution.\n\n";

    while (true) {
        std::wcout << L"Mode: " << (g_options.execute ? L"EXECUTE" : L"DRY RUN") << L"\n";
        std::wcout << L"Preserve NVIDIA Container processes: " << (g_options.preserveNvContainers ? L"yes" : L"no") << L"\n";
        std::wcout << L"Kill lockers: " << (g_options.killLockers ? L"yes" : L"no") << L"\n";
        std::wcout << L"Disable services: " << (g_options.disableServices ? L"yes" : L"no") << L"\n";
        std::wcout << L"Disable scheduled tasks: " << (g_options.disableScheduledTasks ? L"yes" : L"no") << L"\n";
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
        << L"  d = toggle disable-scheduled-tasks\n"
        << L"  t = toggle delete-scheduled-tasks\n"
        << L"  r = toggle reboot-delete for locked files\n"
        << L"  c = toggle preserve NVIDIA containers\n"
        << L"  y = start\n"
        << L"  q = quit\n"
        << L"> ";
        std::wstring input;
        if (!std::getline(std::wcin, input)) {
            // EOF / closed stdin (e.g. non-interactive use): avoid a busy loop.
            std::wcout << L"\nInput closed; exiting.\n";
            g_userQuitRequested = true;
            return;
        }
        input = trim(input);
        if (input.empty()) continue;
        std::wstring low = toLower(input);
        if (low == L"q") { g_userQuitRequested = true; return; }
        if (low == L"y") {
            if (g_options.execute) {
                std::wcout << L"EXECUTE mode will really delete/disable items. Type EXECUTE to confirm: ";
                std::wcout.flush();
                std::wstring confirm;
                if (!std::getline(std::wcin, confirm) || trim(confirm) != L"EXECUTE") {
                    std::wcout << L"Confirmation failed; staying in menu.\n\n";
                    continue;
                }
            }
            break;
        }
        if (low == L"e") { g_options.execute = !g_options.execute; continue; }
        if (low == L"k") { g_options.killLockers = !g_options.killLockers; continue; }
        if (low == L"s") { g_options.disableServices = !g_options.disableServices; continue; }
        if (low == L"d") { g_options.disableScheduledTasks = !g_options.disableScheduledTasks; continue; }
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
    // One log file per logical run. Child processes (elevated UAC instance and
    // the TrustedInstaller/SYSTEM worker) receive --log-file and APPEND to the
    // same file, so a full run leaves exactly one log behind.
    g_state.runId = nowStamp();
    if (!g_options.logFileOverride.empty()) {
        g_state.logPath = fs::path(g_options.logFileOverride);
    } else {
        g_state.logPath = g_state.exeDir / (L"debloat-" + g_state.runId + L".log");
    }
    std::error_code ec;
    if (g_state.logPath.has_parent_path()) {
        fs::create_directories(g_state.logPath.parent_path(), ec);
    } else {
        fs::create_directories(g_state.exeDir, ec);
    }
    if (ec) {
        std::wcerr << L"Warning: could not create log directory '" << g_state.logPath.parent_path().wstring()
                   << L"': " << widen(ec.message()) << L"\n";
    }
}

// -----------------------------
// TrustedInstaller scheduled task relaunch
// -----------------------------

void writeStatusJson(const fs::path& statusFile, int exitCode, const std::wstring& status, const std::wstring& detail) {
    if (statusFile.empty()) return;
    fs::path parentDir = statusFile.parent_path();
    if (!parentDir.empty()) {
        std::error_code mkEc;
        fs::create_directories(parentDir, mkEc);
    }
    std::wstringstream ss;
    ss << L"{\n"
    << L"  \"version\": \"" << GPD_VERSION << L"\",\n"
    << L"  \"runId\": \"" << jsonEscape(g_state.runId) << L"\",\n"
    << L"  \"exitCode\": " << exitCode << L",\n"
    << L"  \"status\": \"" << jsonEscape(status) << L"\",\n"
    << L"  \"detail\": \"" << jsonEscape(detail) << L"\",\n"
    << L"  \"logPath\": \"" << jsonEscape(g_state.logPath.wstring()) << L"\"\n"
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
struct TiRelaunchResult {
    bool attempted = false;       // a relaunch flow was actually started
    bool childStatusSeen = false; // child wrote its status file
    bool childSucceeded = false;
    long childExitCode = -1;
    std::wstring detail;
};

// Removes leftover artifacts of previous crashed runs: stale status files
// beside the executable and orphaned scheduled tasks with our name prefix.
// Without this, a reused PID could make a parent read an ancient "ok" status.
void sweepStaleTiArtifacts(ITaskFolder* pRootFolder) {
    std::error_code ec;
    for (fs::directory_iterator it(g_state.exeDir, fs::directory_options::skip_permission_denied, ec), end;
         !ec && it != end; it.increment(ec)) {
        const std::wstring leaf = it->path().filename().wstring();
        if (leaf.rfind(kTiTaskPrefix, 0) == 0 && endsWithNoCase(leaf, L"-status.json")) {
            std::error_code rmEc;
            fs::remove(it->path(), rmEc);
            if (!rmEc) logLine(L"INFO", L"Removed stale TI status file: " + it->path().wstring());
        }
    }

    if (!pRootFolder) return;
    IRegisteredTaskCollection* pTasks = nullptr;
    HRESULT hr = pRootFolder->GetTasks(TASK_ENUM_HIDDEN, &pTasks);
    if (FAILED(hr) || !pTasks) return;
    LONG count = 0;
    pTasks->get_Count(&count);
    for (LONG i = count; i >= 1; --i) { // backwards: removal shifts indices
        IRegisteredTask* pTask = nullptr;
        if (FAILED(pTasks->get_Item(_variant_t(i), &pTask)) || !pTask) continue;
        BSTR bstrName = nullptr;
        if (SUCCEEDED(pTask->get_Name(&bstrName)) && bstrName) {
            std::wstring name(bstrName);
            SysFreeString(bstrName);
            if (name.rfind(kTiTaskPrefix, 0) == 0) {
                // Release BEFORE deleting: DeleteTask invalidates the
                // IRegisteredTask object of the deleted task. Calling
                // Stop()/Release() afterwards dereferences freed memory
                // (observed as an access violation / exit code 0xC0000005).
                pTask->Stop(0);
                pTask->Release();
                pTask = nullptr;
                pRootFolder->DeleteTask(_bstr_t(name.c_str()), 0);
                logLine(L"INFO", L"Removed stale TI scheduled task: " + name);
            }
        }
        if (pTask) pTask->Release();
    }
    pTasks->Release();
}

// Effective-option flags forwarded to every elevated child (UAC relaunch and
// TrustedInstaller scheduled-task child): wizard defaults from a bare launch
// and toggles made in the interactive menu exist only in this process' memory.
// Reconstructing them from raw argv would lose them and let the elevated child
// run as a dry-run with default selections.
std::vector<std::wstring> effectiveChildSwitches() {
    std::vector<std::wstring> out;
    out.push_back(g_options.execute ? L"--execute" : L"--dry-run");
    if (g_options.killLockers) out.push_back(L"--kill-lockers");
    if (!g_options.preserveNvContainers) out.push_back(L"--preserve-nvcontainers=off");
    if (g_options.disableServices) out.push_back(L"--disable-services");
    if (g_options.deleteServices) out.push_back(L"--delete-services");
    if (g_options.deleteScheduledTasks) out.push_back(L"--delete-scheduled-tasks");
    if (!g_options.disableScheduledTasks) out.push_back(L"--no-disable-scheduled-tasks");
    if (g_options.scheduleLockedForReboot) out.push_back(L"--schedule-reboot-delete");
    if (g_options.takeOwnership) out.push_back(L"--take-ownership");
    if (g_options.includeNGX) out.push_back(L"--include-ngx");
    if (g_options.includeHDAudio) out.push_back(L"--include-hdaudio");
    if (g_options.includePhysX) out.push_back(L"--include-physx");
    if (g_options.includeNotebookOptimus) out.push_back(L"--include-notebook-optimus");
    if (g_options.includeVirtualAudio) out.push_back(L"--include-virtual-audio");
    if (g_options.includeNvWmi) out.push_back(L"--include-nvwmi");
    if (g_options.includeCaptureSdk) out.push_back(L"--include-capture-sdk");
    if (g_options.allowAdminFallback) out.push_back(L"--allow-admin-fallback");
    out.push_back(L"--ti-wait-seconds=" + std::to_wstring(g_options.tiWaitSeconds));
    for (const auto& kv : g_componentEnabled) {
        out.push_back(L"--component=" + kv.first + L":" + (kv.second ? L"on" : L"off"));
    }
    return out;
}

// Attempts to register and run a one-shot scheduled task as
// NT SERVICE\TrustedInstaller using the Task Scheduler COM API.
// Windows does not grant this request -- the task runs as SYSTEM --
// but SYSTEM has sufficient privileges for the debloat operations
// (SeBackupPrivilege, SeRestorePrivilege).
TiRelaunchResult attemptTrustedInstallerRelaunch() {
    TiRelaunchResult res;
    if (!g_options.execute || g_options.tiChild || g_options.allowAdminFallback || isTrustedInstaller()) return res;
    if (!g_options.attemptTiRelaunch) return res;
    res.attempted = true;

    if (!isAdmin()) {
        logLine(L"ERROR", L"Administrator context is required to create the TrustedInstaller scheduled task.");
        res.detail = L"current process is not elevated";
        return res;
    }

    logLine(L"INFO", L"TrustedInstaller context required for destructive execution. Attempting automatic TI scheduled-task relaunch via COM API.");

    std::wstring taskName = std::wstring(kTiTaskPrefix) + std::to_wstring(GetCurrentProcessId());

    std::vector<std::wstring> childArgs = effectiveChildSwitches();
    childArgs.push_back(L"--ti-child");
    childArgs.push_back(L"--no-menu");
    // All stages of a run append to the same single log file.
    childArgs.push_back(L"--log-file");
    childArgs.push_back(g_state.logPath.wstring());

    // --- COM: ITaskService --------------------------------------------------
    HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(hr)) { logLine(L"ERROR", L"CoInitializeEx failed: 0x" + std::to_wstring(hr)); res.detail = L"CoInitializeEx failed"; return res; }

    ITaskService* pService = nullptr;
    hr = CoCreateInstance(CLSID_TaskScheduler, nullptr, CLSCTX_INPROC_SERVER,
                          IID_ITaskService, reinterpret_cast<void**>(&pService));
    if (FAILED(hr) || !pService) {
        logLine(L"ERROR", L"CoCreateInstance(ITaskService) failed: 0x" + std::to_wstring(hr));
        res.detail = L"CoCreateInstance failed";
        CoUninitialize(); return res;
    }

    hr = pService->Connect(_variant_t(), _variant_t(), _variant_t(), _variant_t());
    if (FAILED(hr)) {
        logLine(L"ERROR", L"ITaskService::Connect failed: 0x" + std::to_wstring(hr));
        res.detail = L"ITaskService::Connect failed";
        pService->Release(); CoUninitialize(); return res;
    }

    ITaskFolder* pRootFolder = nullptr;
    hr = pService->GetFolder(_bstr_t(L"\\"), &pRootFolder);
    if (FAILED(hr) || !pRootFolder) {
        logLine(L"ERROR", L"GetFolder(\\\\) failed: 0x" + std::to_wstring(hr));
        res.detail = L"GetFolder failed";
        pService->Release(); CoUninitialize(); return res;
    }

    // Clean leftovers of crashed previous runs before registering our own task.
    sweepStaleTiArtifacts(pRootFolder);

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
        res.detail = L"NewTask failed";
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return res;
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
        // Bound a runaway SYSTEM child instead of letting it live for days.
        pSettings->put_ExecutionTimeLimit(_bstr_t(L"PT2H"));
        pSettings->put_StartWhenAvailable(VARIANT_FALSE);
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
                pExec->put_WorkingDirectory(_bstr_t(g_state.exeDir.wstring().c_str()));
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
    // Exactly ONE release on every path: an extra Release underflows the
    // refcount and corrupts the heap (access violation shortly after).
    pDef->Release();

    if (FAILED(hr) || !pRegistered) {
        logLine(L"ERROR", L"RegisterTaskDefinition failed: 0x" + std::to_wstring(hr));
        res.detail = L"RegisterTaskDefinition failed";
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return res;
    }
    logLine(L"INFO", L"Task registered: " + taskName);

    // --- Run ---
    IRunningTask* pRunning = nullptr;
    hr = pRegistered->Run(_variant_t(), &pRunning);
    if (FAILED(hr)) {
        logLine(L"ERROR", L"Run task failed: 0x" + std::to_wstring(hr));
        res.detail = L"task Run failed";
        if (pRunning) pRunning->Release();
        pRegistered->Stop(0); pRegistered->Release();
        pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
        pRootFolder->Release(); pService->Release(); CoUninitialize(); return res;
    }
    if (pRunning) pRunning->Release();
    logLine(L"INFO", L"Task launched. Waiting for the SYSTEM worker to finish (exit code via LastTaskResult).");

    // --- Wait for task completion; the exit code comes from LastTaskResult.
    // No handshake file is used, so a run leaves no temporary artifacts.
    int waited = 0;
    int startGrace = 60;      // seconds to allow the task to enter Running at all
    bool sawRunning = false;
    bool timedOut = true;
    while (waited < g_options.tiWaitSeconds) {
        TASK_STATE state = TASK_STATE_UNKNOWN;
        if (pRegistered && SUCCEEDED(pRegistered->get_State(&state))) {
            if (state == TASK_STATE_RUNNING) sawRunning = true;
            if (sawRunning && state != TASK_STATE_RUNNING && state != TASK_STATE_QUEUED) {
                // Task finished. LastTaskResult holds the child's exit code.
                LONG lastResult = 0;
                HRESULT hrLast = pRegistered->get_LastTaskResult(&lastResult);
                res.childStatusSeen = true;
                std::wostringstream oss;
                oss << L"TI child finished: ";
                if (SUCCEEDED(hrLast)) {
                    oss << L"exit=0x" << std::hex << lastResult;
                    res.childExitCode = static_cast<long>(lastResult);
                    res.childSucceeded = (lastResult == 0);
                } else {
                    res.childExitCode = -1;
                    res.childSucceeded = false;
                    oss << L"LastTaskResult unreadable (hr=0x" << std::hex << hrLast << L")";
                }
                res.detail = oss.str();
                logLine(res.childSucceeded ? L"INFO" : L"ERROR", res.detail);
                timedOut = false;
                break;
            }
        }

        if (!sawRunning && waited >= startGrace) {
            timedOut = false;
            logLine(L"ERROR", L"TI task did not enter Running state within " + std::to_wstring(startGrace)
                               + L"s. Task Scheduler may have refused the S4U/TrustedInstaller principal.");
            res.detail = L"task never started";
            break;
        }

        if (waited > 0 && waited % 15 == 0) {
            logLine(L"INFO", L"Still waiting for TI child... (" + std::to_wstring(waited) + L"s)");
        }
        std::this_thread::sleep_for(std::chrono::seconds(3));
        waited += 3;
    }

    if (timedOut) {
        logLine(L"ERROR", L"Timed out waiting for TI child after " + std::to_wstring(g_options.tiWaitSeconds) + L" seconds.");
        res.detail = L"timed out waiting for child";
    }

    pRegistered->Stop(0);
    pRegistered->Release();
    pRootFolder->DeleteTask(_bstr_t(taskName.c_str()), 0);
    pRootFolder->Release(); pService->Release(); CoUninitialize();
    return res;
}

// -----------------------------
// NVIDIA matching helpers
// -----------------------------

bool isNvidiaContextString(const std::wstring& s) {
    std::wstring l = toLower(s);
    if (l.find(L"nvidia") != std::wstring::npos ||
        l.find(L"geforce") != std::wstring::npos ||
        l.find(L"frameview") != std::wstring::npos ||
        l.find(L"shadowplay") != std::wstring::npos ||
        l.find(L"displaydriverras") != std::wstring::npos) {
        return true;
    }
    // Bare 'nv' substring matching also hits words like 'Inventory',
    // 'Invoice' or 'Convergence'.  Require an alphanumeric token that starts
    // with nv instead (nvcontainer, nvstream, nvcamera, ...).
    size_t i = 0;
    while (i < l.size()) {
        while (i < l.size() && !iswalnum(l[i])) ++i;
        size_t start = i;
        while (i < l.size() && iswalnum(l[i])) ++i;
        if (i > start && l.compare(start, 2, L"nv") == 0) {
            size_t len = i - start;
            if (len == 2 || len >= 4) return true; // "NV" itself or nvXXXX tokens
        }
    }
    return false;
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


// NVIDIA ships driver packages as e.g. nv_dispi.inf_amd64_<hash> (x64) or
// ...inf_arm64_<hash> (ARM64).  Recognize both so package-root protection and
// sidecar detection work on either architecture.
bool leafHasInfArchMarker(const std::wstring& leafLower) {
    return leafLower.find(L".inf_amd64") != std::wstring::npos ||
           leafLower.find(L".inf_arm64") != std::wstring::npos;
}

bool isDriverStoreSidecarIni(const fs::path& p) {
    std::wstring full = toLower(p.wstring());
    std::wstring leaf = toLower(p.filename().wstring());
    return full.find(L"\\windows\\system32\\driverstore\\filerepository\\") != std::wstring::npos &&
    leafHasInfArchMarker(leaf) &&
    leaf.size() >= 4 &&
    leaf.substr(leaf.size() - 4) == L".ini";
}

bool isDriverStorePackageRootName(const fs::path& p) {
    std::wstring parent = toLower(p.parent_path().wstring());
    std::wstring leaf = toLower(p.filename().wstring());
    return parent.find(L"\\windows\\system32\\driverstore\\filerepository") != std::wstring::npos &&
    leafHasInfArchMarker(leaf);
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
    // Match the NGX directories themselves as well as anything below them.
    bool underNgx = full.find(L"\\programdata\\nvidia\\ngx\\") != std::wstring::npos ||
                    full.find(L"\\nvidia corporation\\ngx\\") != std::wstring::npos ||
                    endsWithNoCase(full, L"\\programdata\\nvidia\\ngx") ||
                    endsWithNoCase(full, L"\\nvidia corporation\\ngx");
    if (!underNgx) return false;
    auto it = g_componentEnabled.find(L"NGX");
    return it == g_componentEnabled.end() || !it->second;
}

bool isCoreDisplayDriverLeaf(const fs::path& p) {
    std::wstring leaf = toLower(p.filename().wstring());
    static const std::set<std::wstring> coreLeaves = {
        L"nvwgf2umx.dll", L"nvwgf2um.dll",
        L"nvd3dumx.dll", L"nvd3dum.dll",
        L"nvldumdx.dll", L"nvapi.dll", L"nvapi64.dll",
        L"nvlddmkm.sys", L"nvdispig.inf", L"nv_dispi.inf"
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
    const auto& comps = buildComponents();

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
    leafHasInfArchMarker(leaf);
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
                std::error_code dirEc;
                if (!it->is_directory(dirEc)) continue; // skip sidecar .ini files etc.
                std::wstring leaf = toLower(it->path().filename().wstring());
                if (leaf.rfind(L"nv", 0) == 0 && leafHasInfArchMarker(leaf)) {
                    roots.push_back(it->path());
                }
                 }
        }
    }

    // Use %SystemDrive% instead of hardcoding C:.
    wchar_t sysDrive[16]{};
    DWORD sdLen = GetEnvironmentVariableW(L"SystemDrive", sysDrive, 16);
    fs::path users = fs::path(sdLen > 0 && sdLen < 16 ? sysDrive : L"C:") / L"Users";
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
    std::error_code parentEc, childEc;
    auto canonicalParent = fs::weakly_canonical(parent, parentEc);
    auto canonicalChild = fs::weakly_canonical(child, childEc);
    std::wstring p = toLower((parentEc ? parent : canonicalParent).wstring());
    std::wstring c = toLower((childEc ? child : canonicalChild).wstring());
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
        if (g_abortRequested.load()) { g_state.aborted = true; break; }
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
                if (g_abortRequested.load()) { g_state.aborted = true; break; }
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
    if (g_state.aborted) logLine(L"WARN", L"Candidate scan aborted early by user request.");
    collapseNestedCandidates(g_state.candidates);
    logLine(L"INFO", L"Candidate count: " + std::to_wstring(g_state.candidates.size()));

    // Per-component breakdown so an already-clean system explicitly reports
    // 0 for every area instead of one opaque global count.
    {
        std::wstringstream ss;
        ss << L"\n==== Discovery summary by component ====\n";
        std::wstring line;
        for (const auto& c : buildComponents()) {
            size_t n = 0;
            for (const auto& cand : g_state.candidates) if (cand.componentKey == c.key) ++n;
            ss << c.key << L": " << n << L"\n";
            if (!line.empty()) line += L", ";
            line += c.key + L"=" + std::to_wstring(n);
        }
        appendUtf8File(g_state.logPath, utf8(ss.str()));
        logLine(L"INFO", L"Discovered per component: " + line);
        if (g_state.candidates.empty()) {
            logLine(L"INFO", L"Nothing matched - the system appears already clean (or only disabled optional components were scanned). For what earlier runs removed, see their logs / the previous-run history line.");
        }
    }
}

// Rough lifetime view: parses the other debloat-*.log files beside the
// executable (earlier runs) and totals how many runs they cover and how many
// candidate paths discovery saw across them. Zero extra files are created.
void tallyPreviousLogs() {
    size_t logFiles = 0, runs = 0;
    long long candidatesFound = 0;
    std::error_code ec;
    const std::wstring currentLower = toLower(g_state.logPath.wstring());
    for (fs::directory_iterator it(g_state.exeDir, fs::directory_options::skip_permission_denied, ec), end;
         !ec && it != end; it.increment(ec)) {
        const fs::path p = it->path();
        const std::wstring low = toLower(p.filename().wstring());
        if (low.rfind(L"debloat-", 0) != 0 || !endsWithNoCase(low, L".log")) continue;
        if (toLower(p.wstring()) == currentLower) continue;
        std::ifstream in(p, std::ios::binary);
        if (!in) continue;
        ++logFiles;
        std::string line;
        while (std::getline(in, line)) {
            if (line.find("run header ====") != std::string::npos) ++runs;
            const std::string marker = "Candidate count: ";
            const size_t pos = line.find(marker);
            if (pos != std::string::npos) {
                try { candidatesFound += std::stoll(trim(widen(line.substr(pos + marker.size())))); } catch (...) {}
            }
        }
    }
    g_state.historyScanDone = true;
    g_state.historyLogFiles = static_cast<long>(logFiles);
    g_state.historyRuns = static_cast<long>(runs);
    g_state.historyCandidates = candidatesFound;
    if (logFiles == 0) {
        logLine(L"INFO", L"Previous-run history: no earlier debloat-*.log files beside the executable.");
    } else {
        logLine(L"INFO", L"Previous-run history: " + std::to_wstring(logFiles)
                          + L" earlier log file(s), " + std::to_wstring(runs)
                          + L" recorded run(s), " + std::to_wstring(candidatesFound)
                          + L" candidate paths discovered across them (see those logs for the exact paths).");
    }
}

void writeCandidatesCsv() {
    // The candidates list is appended to the run log instead of a separate CSV,
    // so a full run leaves exactly one file behind.
    std::wostringstream ss;
    ss << L"==== Candidates (" << g_state.candidates.size() << L" total) ====\n";
    ss << L"component,path,type\n";
    for (const auto& c : g_state.candidates) {
        ss << c.componentKey << L",\"" << c.path.wstring() << L"\"," << (c.isDirectory ? L"directory" : L"file") << L"\n";
    }
    appendUtf8File(g_state.logPath, utf8(ss.str()));
}

// -----------------------------
// Process/service/task handling
// -----------------------------

void inspectNvContainerModules(bool postDeletePhase) {
    logLine(L"INFO", postDeletePhase
        ? L"Re-inspecting NVIDIA Container processes after deletion."
        : L"Inspecting NVIDIA Container processes and loaded telemetry/profile-update modules.");
    // This runs twice per cleanup; avoid duplicated report entries for the
    // same PID/module sighting across phases.
    static std::set<std::wstring> reportedContainers;
    static std::set<std::wstring> reportedModules;
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
            const std::wstring pidStr = std::to_wstring(pe.th32ProcessID);
            if (reportedContainers.insert(pidStr + L"|" + exe).second) {
                logLine(L"INFO", L"Container running: PID=" + pidStr + L" Name=" + exe);
                addAction(L"ContainerProcess", L"Running", L"NvContainer", exe, L"PID=" + pidStr);
            }

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
                        if (reportedModules.insert(pidStr + L"|" + path).second) {
                            logLine(L"WARN", L"Container has matching module loaded; deletion may require reboot: PID=" + pidStr + L" Module=" + path);
                            addAction(L"LoadedTelemetryModule", L"Loaded", L"NvContainer", path, L"PID=" + pidStr);
                        } else if (postDeletePhase) {
                            logLine(L"INFO", L"Module still loaded after deletion (reboot will finalize): PID=" + pidStr + L" Module=" + path);
                        }
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
            HANDLE h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, pe.th32ProcessID);
            if (!h) {
                logAction(L"KillProcess", L"WARN", L"Process", exe, L"OpenProcess failed: " + formatWinError(GetLastError()));
                continue;
            }
            BOOL ok = TerminateProcess(h, 0);
            DWORD terminateErr = GetLastError();
            std::wstring detail;
            if (ok) {
                // File handles are released asynchronously; wait briefly so
                // immediate deletions are not defeated by dying lockers.
                detail = WaitForSingleObject(h, 3000) == WAIT_OBJECT_0
                    ? std::wstring(L"Terminated")
                    : std::wstring(L"Terminate requested (still exiting)");
            } else {
                detail = formatWinError(terminateErr);
            }
            CloseHandle(h);
            logAction(L"KillProcess", ok ? L"INFO" : L"WARN", L"Process", exe, detail);
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
    // Token-aware context check: a bare 'nv' substring would also hit
    // unrelated names such as '\Inventory\...' tasks.
    if (!isNvidiaContextString(taskName)) return false;
    std::wstring l = toLower(taskName);
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
    // Absolute path: never let CreateProcess search CWD/app-dir for tools
    // while running elevated or as SYSTEM.
    const std::wstring schtasks = systemDirFile(L"schtasks.exe");
    auto res = runProcessCapture(joinCommand({schtasks, L"/Query", L"/FO", L"CSV", L"/NH"}), 120000);
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
        // The task-name column position varies between schtasks versions;
        // select the field that looks like an absolute task path ('\Foo').
        std::wstring taskName;
        for (const auto& f : fields) {
            if (!f.empty() && f[0] == L'\\') { taskName = f; break; }
        }
        if (taskName.empty()) taskName = fields[0];
        if (!taskMatchesBloat(taskName)) continue;

        if (g_options.deleteScheduledTasks) {
            if (!g_options.execute) logAction(L"DeleteTask", L"DRYRUN", L"ScheduledTask", taskName);
            else {
                auto del = runProcessCapture(joinCommand({schtasks, L"/Delete", L"/TN", taskName, L"/F"}), 60000);
                logAction(L"DeleteTask", del.exitCode == 0 ? L"INFO" : L"WARN", L"ScheduledTask", taskName, trim(del.output));
            }
        } else if (g_options.disableScheduledTasks) {
            if (!g_options.execute) logAction(L"DisableTask", L"DRYRUN", L"ScheduledTask", taskName);
            else {
                auto dis = runProcessCapture(joinCommand({schtasks, L"/Change", L"/TN", taskName, L"/Disable"}), 60000);
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
    const std::wstring takeownExe = systemDirFile(L"takeown.exe");
    const std::wstring icaclsExe = systemDirFile(L"icacls.exe");
    const std::wstring target = p.wstring();
    // '/D' expects a locale-specific letter for "Yes" (German wants J,
    // French O, ...). Try the common variants until one is accepted.
    static const std::vector<std::wstring> yesLetters = { L"Y", L"J", L"O", L"S" };
    bool takeownOk = false;
    for (const auto& letter : yesLetters) {
        if (runShellCommand(joinCommand({takeownExe, L"/F", target, L"/A", L"/R", L"/D", letter}), L"takeown", true)) {
            takeownOk = true;
            break;
        }
        if (!g_options.execute) break; // dry-run: one representative command suffices
    }
    // Grant via well-known SID; localized group names ("Administratoren", ...)
    // would fail on non-English systems.
    runShellCommand(joinCommand({icaclsExe, target, L"/grant", L"*S-1-5-32-544:F", L"/T", L"/C"}), L"icacls", true);
    if (g_options.execute && !takeownOk) {
        logLine(L"WARN", L"takeown did not report success; the '/D' yes-letter may differ on this locale.");
    }
}

// \\?\-prefixed variant to sidestep MAX_PATH limits on deep DriverStore trees.
fs::path extendedLengthPath(const fs::path& p) {
    std::wstring w = p.wstring();
    if (w.rfind(L"\\\\?\\", 0) == 0) return p; // already extended
    if (w.size() >= 2 && w[1] == L':') return fs::path(L"\\\\?\\" + w);
    return p; // UNC or relative; leave untouched
}

// Read-only/system attributes make DeleteFileW and remove_all fail; clear them.
void clearBlockingAttributes(const fs::path& p) {
    SetFileAttributesW(p.wstring().c_str(), FILE_ATTRIBUTE_NORMAL);
    std::error_code ec;
    if (!fs::is_directory(p, ec)) return;
    fs::recursive_directory_iterator it(p, fs::directory_options::skip_permission_denied, ec), end;
    while (!ec && it != end) {
        SetFileAttributesW(it->path().wstring().c_str(), FILE_ATTRIBUTE_NORMAL);
        it.increment(ec);
    }
}

bool scheduleDeleteOne(const fs::path& p) {
    BOOL ok = MoveFileExW(p.wstring().c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
    if (!ok) ok = MoveFileExW(extendedLengthPath(p).wstring().c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
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
        for (const auto& child : paths) {
            std::error_code childEc;
            if (!fs::exists(child, childEc)) continue; // vanished already
            allOk = scheduleDeleteOne(child) && allOk;
        }
    }
    std::error_code rootEc;
    if (fs::exists(p, rootEc)) allOk = scheduleDeleteOne(p) && allOk;
    return allOk;
}

void deleteCandidate(const Candidate& c) {
    if (g_abortRequested.load()) { g_state.aborted = true; return; }

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
        if (ec) {
            // Retry once: clear read-only attributes and try the extended-length form.
            clearBlockingAttributes(c.path);
            ec.clear();
            removed = fs::remove_all(extendedLengthPath(c.path), ec);
        }
        if (!ec) {
            logAction(L"DeletePath", L"INFO", c.componentKey, c.path.wstring(), L"Removed entries=" + std::to_wstring(removed));
            return;
        }
    } else {
        bool ok = DeleteFileW(c.path.wstring().c_str()) == TRUE;
        if (!ok) {
            DWORD delErr = GetLastError();
            if (delErr == ERROR_ACCESS_DENIED || delErr == ERROR_FILE_READ_ONLY) {
                // Likely read-only attribute or a >MAX_PATH path; clear + retry.
                SetFileAttributesW(c.path.wstring().c_str(), FILE_ATTRIBUTE_NORMAL);
                ok = DeleteFileW(extendedLengthPath(c.path).wstring().c_str()) == TRUE;
            }
        }
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
    ss << L"\n==== Run report (JSON) ====\n";
    ss << L"{\n";
    ss << L"  \"runId\": \"" << jsonEscape(g_state.runId) << L"\",\n";
    ss << L"  \"exePath\": \"" << jsonEscape(g_state.exePath.wstring()) << L"\",\n";
    ss << L"  \"identity\": \"" << jsonEscape(currentTokenAccount()) << L"\",\n";
    ss << L"  \"isAdmin\": " << (isAdmin() ? L"true" : L"false") << L",\n";
    ss << L"  \"isTrustedInstaller\": " << (isTrustedInstaller() ? L"true" : L"false") << L",\n";
    ss << L"  \"execute\": " << (g_options.execute ? L"true" : L"false") << L",\n";
    ss << L"  \"version\": \"" << GPD_VERSION << L"\",\n";
    ss << L"  \"logPath\": \"" << jsonEscape(g_state.logPath.wstring()) << L"\",\n";
    ss << L"  \"aborted\": " << (g_state.aborted ? L"true" : L"false") << L",\n";
    ss << L"  \"candidateCount\": " << g_state.candidates.size() << L",\n";
    if (g_state.postRunCheckDone) {
        ss << L"  \"candidatePathsRemainingAfterRun\": " << g_state.pathsRemainingAfterRun << L",\n";
        ss << L"  \"candidatePathsPendingRebootDelete\": " << g_state.pathsPendingReboot << L",\n";
    }
    if (g_state.historyScanDone) {
        ss << L"  \"previousLogs\": {\"files\": " << g_state.historyLogFiles
           << L", \"runs\": " << g_state.historyRuns
           << L", \"candidatesDiscoveredTotal\": " << g_state.historyCandidates << L"},\n";
    }

    ss << L"  \"components\": {";
    {
        bool firstComponent = true;
        for (const auto& c : buildComponents()) {
            if (!firstComponent) ss << L", ";
            firstComponent = false;
            auto enabledIt = g_componentEnabled.find(c.key);
            const bool on = enabledIt != g_componentEnabled.end() && enabledIt->second;
            ss << L"\"" << jsonEscape(c.key) << L"\": " << (on ? L"true" : L"false");
        }
    }
    ss << L"},\n";

    ss << L"  \"options\": {"
    << L"\"killLockers\": " << (g_options.killLockers ? L"true" : L"false")
    << L", \"disableServices\": " << (g_options.disableServices ? L"true" : L"false")
    << L", \"deleteServices\": " << (g_options.deleteServices ? L"true" : L"false")
    << L", \"disableScheduledTasks\": " << (g_options.disableScheduledTasks ? L"true" : L"false")
    << L", \"deleteScheduledTasks\": " << (g_options.deleteScheduledTasks ? L"true" : L"false")
    << L", \"scheduleLockedForReboot\": " << (g_options.scheduleLockedForReboot ? L"true" : L"false")
    << L", \"takeOwnership\": " << (g_options.takeOwnership ? L"true" : L"false")
    << L", \"preserveNvContainers\": " << (g_options.preserveNvContainers ? L"true" : L"false")
    << L", \"allowAdminFallback\": " << (g_options.allowAdminFallback ? L"true" : L"false")
    << L", \"tiChild\": " << (g_options.tiChild ? L"true" : L"false")
    << L"},\n";

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

    HANDLE h = CreateFileW(g_state.logPath.wstring().c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
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
}

// -----------------------------
// Main cleanup flow
// -----------------------------

// Re-checks every discovered candidate path after processing and appends a
// "Post-run existence check" section to the log: how many NVIDIA bloat paths
// remain after the run (0 = fully cleaned) and why any of them remain
// (dry-run / pending reboot deletion / deletion failure).
void verifyCandidateRemoval() {
    // Latest DeletePath/ScheduleDelete outcome per candidate path.
    std::map<std::wstring, std::pair<std::wstring, std::wstring>> pathOutcome; // status, detail
    for (const auto& a : g_state.actions) {
        if (a.type == L"DeletePath") pathOutcome[a.path] = {a.status, a.detail};
        else if (a.type == L"ScheduleDelete") pathOutcome[a.path] = {L"SCHEDULED_REBOOT", a.detail};
    }

    size_t remaining = 0, pendingReboot = 0, dryRun = 0;
    std::wstringstream ss;
    ss << L"\n==== Post-run existence check ====\n";
    for (const auto& c : g_state.candidates) {
        std::error_code ec;
        if (!fs::exists(c.path, ec)) continue; // gone: exactly what we want
        ++remaining;
        auto it = pathOutcome.find(c.path.wstring());
        const std::wstring status = it != pathOutcome.end() ? it->second.first : std::wstring();
        const std::wstring detail = it != pathOutcome.end() ? it->second.second : std::wstring();
        std::wstring reason;
        if (!g_options.execute || status == L"DRYRUN") { reason = L"dry-run: not attempted"; ++dryRun; }
        else if (status == L"SCHEDULED_REBOOT") { reason = L"scheduled for deletion at next reboot"; ++pendingReboot; }
        else if (status == L"SKIP") { reason = L"skipped: " + detail; }
        else if (status == L"WARN") { reason = L"deletion failed: " + detail; }
        else if (status.empty()) { reason = L"no action recorded"; }
        else { reason = L"still present"; }
        ss << L"STILL EXISTS [" << reason << L"] " << c.path.wstring() << L"\n";
    }

    g_state.postRunCheckDone = true;
    g_state.pathsRemainingAfterRun = static_cast<long>(remaining);
    g_state.pathsPendingReboot = static_cast<long>(pendingReboot);

    if (g_state.candidates.empty()) {
        ss << L"No NVIDIA bloat candidates were discovered in this run.\n";
    } else if (remaining == 0) {
        ss << L"Result: all " << g_state.candidates.size() << L" discovered paths are gone (0 remaining).\n";
    } else {
        ss << L"Summary: " << remaining << L" of " << g_state.candidates.size()
           << L" discovered paths still exist"
           << (pendingReboot ? L" (" + std::to_wstring(pendingReboot) + L" pending reboot deletion)" : L"")
           << (dryRun ? L" (" + std::to_wstring(dryRun) + L" not attempted in dry-run)" : L"")
           << L".\n";
    }
    appendUtf8File(g_state.logPath, utf8(ss.str()));

    if (g_state.candidates.empty()) {
        logLine(L"INFO", L"Post-run check: no NVIDIA bloat candidates discovered; nothing to verify (0 paths remain).");
    } else {
        logLine(remaining == 0 ? L"INFO" : L"WARN",
                L"Post-run check: " + std::to_wstring(g_state.candidates.size() - remaining) + L"/"
                + std::to_wstring(g_state.candidates.size()) + L" candidate paths removed"
                + (remaining ? L"; " + std::to_wstring(remaining) + L" still exist (details above)" : L""));
    }
}

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
    {
        std::wstring compSummary;
        for (const auto& c : buildComponents()) {
            if (!compSummary.empty()) compSummary += L", ";
            auto enabledIt = g_componentEnabled.find(c.key);
            const bool on = enabledIt != g_componentEnabled.end() && enabledIt->second;
            compSummary += c.key + L"=" + (on ? L"on" : L"off");
        }
        logLine(L"INFO", L"Components: " + compSummary);
    }

    if (g_options.execute && !g_options.tiChild && !isTrustedInstaller() && !g_options.allowAdminFallback) {
        throw std::runtime_error("Destructive execution requires TrustedInstaller. Relaunch did not occur or did not enter TI context.");
    }
    if (g_options.tiChild && !isTrustedInstaller()) {
        logLine(L"WARN", L"TI child running as " + currentTokenAccount() + L" instead of TrustedInstaller. SYSTEM-level privileges should suffice for file operations.");
    }

    handleServices();
    killLockerProcesses();
    handleScheduledTasks();
    tallyPreviousLogs();
    inspectNvContainerModules(false);
    discoverCandidates();
    writeCandidatesCsv();

    for (const auto& c : g_state.candidates) {
        if (g_abortRequested.load()) { g_state.aborted = true; break; }
        deleteCandidate(c);
    }

    inspectNvContainerModules(true);
    verifyCandidateRemoval();
    if (g_state.aborted) {
        logLine(L"WARN", L"Run aborted by user request; partial results recorded.");
    }
    writeReport();
    logLine(L"INFO", L"Done. Log: " + g_state.logPath.wstring());
    if (!g_options.execute) logLine(L"WARN", L"Dry run only. Re-run with --execute to make changes.");
    if (g_options.scheduleLockedForReboot) logLine(L"WARN", L"Some locked-file deletions may require reboot. Review report/log.");
}

// Re-launches this executable elevated via a UAC "runas" prompt, carrying the
// full effective option/component state plus the shared --log-file target.
// Used when a bare double-click launch ends up in EXECUTE mode without
// administrator rights: the wizard has already been shown and confirmed
// interactively ('y' plus typing EXECUTE), so the elevated instance runs with
// --no-menu but keeps its window open via --pause.
// Returns false if the user declined UAC or the launch failed; otherwise sets
// childExitCode from the elevated instance.
bool relaunchElevatedForWizard(const fs::path& exePath, const fs::path& logFile, int& childExitCode) {
    std::wstring params;
    for (const auto& a : effectiveChildSwitches()) {
        if (!params.empty()) params += L" ";
        params += a;
    }
    params += L" --no-menu --pause";
    fs::path file = logFile.empty() ? exePath.parent_path() / (L"debloat-" + g_state.runId + L".log") : logFile;
    params += L" --log-file \"" + file.wstring() + L"\"";
    SHELLEXECUTEINFOW sei{};
    sei.cbSize = sizeof(sei);
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_DDEWAIT;
    sei.lpVerb = L"runas";
    sei.lpFile = exePath.c_str();
    sei.lpParameters = params.c_str();
    sei.nShow = SW_SHOWNORMAL;
    if (!ShellExecuteExW(&sei) || !sei.hProcess) return false;
    WaitForSingleObject(sei.hProcess, INFINITE);
    DWORD rc = 0;
    GetExitCodeProcess(sei.hProcess, &rc);
    CloseHandle(sei.hProcess);
    childExitCode = static_cast<int>(rc);
    return true;
}

// Graceful Ctrl+C / console-close handling: stop after the current item and
// still produce report/status artifacts.  Keep the handler minimal -- it runs
// on a different thread -- so it only sets the atomic flag.
static BOOL WINAPI consoleCtrlHandler(DWORD ctrlType) {
    switch (ctrlType) {
        case CTRL_C_EVENT:
        case CTRL_BREAK_EVENT:
        case CTRL_CLOSE_EVENT:
            g_abortRequested.store(true);
            return TRUE;
        default:
            return FALSE;
    }
}

int wmain(int argc, wchar_t** argv) {
    SetConsoleCtrlHandler(consoleCtrlHandler, TRUE);
    int exitCode = EXIT_OK;
    try {
        g_options = parseArgs(argc, argv);

        if (g_options.showHelp) { printUsage(); return EXIT_OK; }
        if (g_options.showVersion) {
            std::wcout << L"GreenPostInstallDebloatNative version " << GPD_VERSION << L"\n";
            return EXIT_OK;
        }

        initializeComponentSelection();

        if (g_options.listComponents) {
            for (const auto& c : buildComponents()) {
                auto enabledIt = g_componentEnabled.find(c.key);
                const bool on = enabledIt != g_componentEnabled.end() && enabledIt->second;
                std::wcout << (on ? L"[x] " : L"[ ] ") << c.key
                           << (c.optional ? L" (optional)" : L"") << L"\n";
            }
            return EXIT_OK;
        }

        applyComponentArgs(argc, argv);

        // Wizard default keeps the NVIDIA driver-update/profile-updater stack:
        // deselect UpdateAndProfileUpdater unless explicitly re-enabled.
        if (g_options.wizardDefaults) {
            auto updIt = g_componentEnabled.find(L"UpdateAndProfileUpdater");
            if (updIt != g_componentEnabled.end()) updIt->second = false;
        }

        initializeRunState();

        if (g_options.menu && !g_options.tiChild) {
            interactiveMenu();
        }
        if (g_userQuitRequested) {
            return EXIT_OK;
        }

        logLine(L"INFO", L"Run log (all stages of this run append to this single file): " + g_state.logPath.wstring());
        for (const auto& unknown : g_options.unknownArgs) {
            logLine(L"WARN", L"Ignoring unknown option: " + unknown);
        }

        // Bare double-click launch in EXECUTE mode without elevation: the wizard
        // has been confirmed interactively at this point, so hand off to an
        // elevated instance via UAC. Must happen before the single-instance
        // mutex is taken, otherwise the elevated child would see it as held.
        if (g_options.wizardDefaults && g_options.execute && !g_options.tiChild
            && !isAdmin() && !isTrustedInstaller()) {
            logLine(L"INFO", L"Administrator rights required; relaunching elevated via UAC with the confirmed selection.");
            int childExit = 0;
            if (relaunchElevatedForWizard(getExePath(), g_state.logPath, childExit)) {
                logLine(L"INFO", L"Elevated instance finished with exit code " + std::to_wstring(childExit) + L".");
                return childExit;
            }
            logLine(L"WARN", L"Elevation was declined or unavailable; continuing without administrator rights.");
            std::wcout << L"\nElevation was declined or unavailable; continuing without administrator rights.\n";
        }

        // Refuse overlapping destructive runs; they would race over files,
        // services and pending-reboot registrations.
        HANDLE singleInstanceMutex = nullptr;
        if (g_options.execute && !g_options.tiChild) {
            singleInstanceMutex = CreateMutexW(nullptr, TRUE, L"Global\\GreenPostInstallDebloatNative_Execute_Mutex");
            if (!singleInstanceMutex) {
                logLine(L"WARN", L"Single-instance mutex unavailable: " + formatWinError(GetLastError()));
            } else if (GetLastError() == ERROR_ALREADY_EXISTS) {
                logLine(L"FATAL", L"Another execute-mode instance is already running; refusing concurrent destructive run.");
                writeStatusJson(g_options.statusFile, EXIT_ALREADY_RUNNING, L"failed", L"another execute-mode instance is running");
                return EXIT_ALREADY_RUNNING;
            }
        }

        TiRelaunchResult tiResult;
        if (g_options.execute && !g_options.tiChild && !g_options.allowAdminFallback
            && !isTrustedInstaller() && g_options.attemptTiRelaunch) {
            tiResult = attemptTrustedInstallerRelaunch();
            if (tiResult.childStatusSeen && tiResult.childSucceeded) {
                logLine(L"INFO", L"Parent process finished after TI child completion. Everything (including the worker's report) is in the shared run log.");
                return EXIT_OK;
            }
            if (tiResult.childStatusSeen) {
                logLine(L"FATAL", L"Elevated TI child reported failure: " + tiResult.detail);
                writeStatusJson(g_options.statusFile, EXIT_TI_CHILD_FAILED, L"failed", L"TI child failed: " + tiResult.detail);
                return EXIT_TI_CHILD_FAILED;
            }
            if (!g_options.allowAdminFallback) {
                logLine(L"FATAL", L"TrustedInstaller relaunch failed"
                                   + (tiResult.detail.empty() ? L"." : L" (" + tiResult.detail + L")")
                                   + L" and --allow-admin-fallback was not specified.");
                writeStatusJson(g_options.statusFile, EXIT_TI_RELAUNCH_FAILED, L"failed", L"TrustedInstaller relaunch failed: " + tiResult.detail);
                return EXIT_TI_RELAUNCH_FAILED;
            }
        }

        runCleanup();
        if (g_state.aborted) {
            writeStatusJson(g_options.statusFile, EXIT_ABORTED, L"aborted", L"run aborted by user request");
            exitCode = EXIT_ABORTED;
        } else {
            writeStatusJson(g_options.statusFile, EXIT_OK, L"ok", L"completed");
        }
    } catch (const std::exception& ex) {
        exitCode = EXIT_FATAL_EXCEPTION;
        std::wstring msg = widen(ex.what());
        logLine(L"FATAL", msg);
        try { writeReport(); } catch (...) {}
        writeStatusJson(g_options.statusFile, exitCode, L"fatal", msg);
    } catch (...) {
        exitCode = EXIT_FATAL_UNKNOWN;
        logLine(L"FATAL", L"Unknown fatal exception.");
        try { writeReport(); } catch (...) {}
        writeStatusJson(g_options.statusFile, exitCode, L"fatal", L"unknown fatal exception");
    }

    if (!g_options.noPause && !g_options.tiChild && !g_userQuitRequested
        && (g_options.menu || g_options.pauseOnExit)) {
        std::wcout << L"\nPress Enter to exit...";
        std::wcout.flush();
        std::wstring dummy;
        std::getline(std::wcin, dummy);
    }
    return exitCode;
}
