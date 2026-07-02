# GreenPostInstallDebloatNative

**WARNING: THIS SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.**

You use this tool entirely at your own risk. It deletes files, modifies Windows
services, disables scheduled tasks, and schedules reboot-time deletion of locked
files. Misuse or bugs **can** damage your NVIDIA driver installation, break GPU
acceleration, cause display issues, or otherwise mess up your system.

- The author has **not** tested this on a wide range of systems.
- There is **no** undo. If something breaks, you may need to reinstall the
  NVIDIA driver or, in the worst case, restore from backup.
- **Always** run `--dry-run` first and review the log before running with
  `--execute`.

---

## What it does

Scans common NVIDIA installation paths (`Program Files`, `ProgramData`,
`DriverStore/FileRepository`, `%APPDATA%`, etc.) for known bloat components and
optionally removes them.  The list includes:

- Telemetry DLLs and plugins
- GeForce Experience / NVIDIA App userland files
- ShadowPlay / Share components
- Ansel / NvCamera
- FrameView SDK / PresentMon
- SHIELD / streaming support
- NVIDIA virtual audio device (NvVAD)
- USB-C / virtual host controller support (NvvHCI)
- Legacy 3D Vision / Stereo extras
- NGX / DLSS runtime cache
- HD Audio driver extras (nvhda)
- PhysX SDK
- Notebook / Optimus helpers
- Installer2 unpacked installer cache

By default, NGX, HDAudio, PhysX, NotebookOptimus, VirtualAudio, NvWMI and
CaptureSDK are **not** enabled (use their `--include-*` flags to activate).

---

## Build

```powershell
python build.py
```

Downloads a statically-linked LLVM/MinGW toolchain into `mingw64/` and compiles
the binary.  You only need Python — no Visual Studio or manual toolchain setup
required.

---

## Usage

### 1. Interactive menu (default)

```
GreenPostInstallDebloatNative.exe
```

### 2. Dry-run (safe, does nothing)

```
GreenPostInstallDebloatNative.exe --dry-run
```

### 3. Full cleanup

Run as Administrator:

```
GreenPostInstallDebloatNative.exe --execute --kill-lockers --disable-services --delete-scheduled-tasks --schedule-reboot-delete
```

This will attempt a TrustedInstaller-level relaunch via the Task Scheduler COM
API so that it can delete files a normal Administrator cannot touch.  If the
system does not grant TrustedInstaller (Windows limitation), the tool falls back
to SYSTEM privileges, which are still sufficient for the vast majority of files.

### Flags

| Flag | Description |
|------|-------------|
| `--execute` | Actually delete/disable. Default is dry-run. |
| `--dry-run` | Explicit dry-run. |
| `--menu` | Interactive component selection. |
| `--kill-lockers` | Stop bloat processes before deleting. |
| `--preserve-nvcontainers=false` | Also kill NVDisplay.Container/nvcontainer. |
| `--disable-services` | Disable matched NVIDIA bloat services. |
| `--delete-services` | Delete matched NVIDIA bloat services. |
| `--disable-scheduled-tasks` | Disable matched NVIDIA scheduled tasks (default on). |
| `--delete-scheduled-tasks` | Delete matched NVIDIA scheduled tasks. |
| `--schedule-reboot-delete` | Use MoveFileEx for locked files. |
| `--take-ownership` | Run takeown/icacls before deletion. |
| `--include-ngx` | Include NGX/DLSS runtime cache cleanup. |
| `--include-hdaudio` | Include NVIDIA HD Audio cleanup. |
| `--include-physx` | Include PhysX SDK cleanup. |
| `--include-notebook-optimus` | Include notebook/Optimus helper cleanup. |
| `--include-virtual-audio` | Include NvVAD virtual audio device cleanup. |
| `--include-nvwmi` | Include NVIDIA WMI management interface cleanup. |
| `--include-capture-sdk` | Include NvFBC/NvIFR capture SDK runtime cleanup. |
| `--no-ti-relaunch` | Do not attempt TrustedInstaller scheduled-task relaunch. |
| `--allow-admin-fallback` | Permit destructive execution as Administrator. |
| `--ti-wait-seconds N` | Parent wait timeout for TI child (default 600). |
| `--no-pause` | Do not pause on exit. |
| `--no-color` | Monochrome console output. |
| `--component=Key:on/off` | Toggle a specific component (e.g. `--component=NGX:on`). |

---

## Logs & reports

Generated beside the executable:

- `debloat-YYYYMMDD-HHMMSS.log` — full run log
- `last-report.json` — structured action summary
- `last-candidates.csv` — discovered candidates

---

## License

MIT License

Copyright (c) 2026 aufkrawall

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
