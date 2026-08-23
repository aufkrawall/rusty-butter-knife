//! The bloat-component catalog. Port of `buildComponents()` — keys, display
//! names, defaults, globs and exact leaf names must stay byte-identical.

use crate::types::Component;

fn c(
    key: &str,
    display_name: &str,
    default_enabled: bool,
    optional: bool,
    leaf_globs: &[&str],
    exact_leaf_names: &[&str],
) -> Component {
    Component {
        key: key.to_string(),
        display_name: display_name.to_string(),
        default_enabled,
        optional,
        leaf_globs: leaf_globs.iter().map(|s| s.to_string()).collect(),
        exact_leaf_names: exact_leaf_names.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn build_components() -> &'static [Component] {
    static COMPONENTS: std::sync::OnceLock<Vec<Component>> = std::sync::OnceLock::new();
    COMPONENTS.get_or_init(|| {
        vec![
            c(
                "Telemetry",
                "Telemetry, DisplayDriverRAS, container telemetry plugins",
                true,
                false,
                &[
                    "*Telemetry*",
                    "_DisplayDriverRAS.dll",
                    "_NvMsgBusBroadcast.dll",
                    "_nvtopps.dll",
                    "_NvGSTPlugin.dll",
                    "NvTelemetry*.dll",
                    "NvTelemetry*.exe",
                ],
                &["DisplayDriverRAS", "GameSessionTelemetry", "NvTelemetry"],
            ),
            c(
                "UpdateAndProfileUpdater",
                "Driver update and profile-updater components",
                true,
                false,
                &[
                    "nvprofileupdaterplugin.dll",
                    "*ProfileUpdater*",
                    "*DriverUpdate*",
                    "*NvOTA*",
                    "*NvBackend*",
                    "*NvModuleTracker*",
                    "*NvGFTrayPlugin*",
                    "*NvNodeLauncher*",
                ],
                &[
                    "Display.Update",
                    "Update.Core",
                    "NvProfileUpdaterPlugin",
                    "NvBackend",
                    "NvDriverUpdateCheck",
                    "NvModuleTracker",
                    "NvOTA",
                ],
            ),
            c(
                "Installer2Cache",
                "Installer2 unpacked installer cache",
                true,
                false,
                &[],
                &["Installer2"],
            ),
            c(
                "GeForceExperienceAndNvidiaApp",
                "NVIDIA App / GeForce Experience userland extras",
                true,
                false,
                &[
                    "*GeForce Experience*",
                    "*GFExperience*",
                    "*NVIDIA Web Helper*",
                    "NVIDIA App*.exe",
                    "*NvBackend*",
                ],
                &[
                    "NVIDIA App",
                    "NVIDIA GeForce Experience",
                    "GeForce Experience",
                    "GFExperience",
                ],
            ),
            // Keep this tight: broad '*Share*.dll' matched Nsight InterfaceShared*.dll.
            // NvFBC/NvIFR are capture SDK/runtime APIs and are opt-in via CaptureSDK.
            c(
                "ShadowPlayShare",
                "ShadowPlay / NVIDIA Share userland extras",
                true,
                false,
                &["*ShadowPlay*", "NVIDIA Share*.exe", "nvsphelper*.exe"],
                &["ShadowPlay", "NVIDIA Share"],
            ),
            c(
                "CaptureSDK",
                "NvFBC / NvIFR capture SDK runtime components",
                false,
                true,
                &["NvIFR*.dll", "NvFBC*.dll"],
                &[],
            ),
            c(
                "AnselCamera",
                "Ansel / NvCamera",
                true,
                false,
                &["NvCamera*.dll", "*Ansel*"],
                &["NvCamera", "Ansel"],
            ),
            c(
                "FrameView",
                "FrameView SDK / PresentMon extras",
                true,
                false,
                &[
                    "*FrameView*",
                    "*PresentMon*",
                    "NvFv*.dll",
                    "NvFrameView*.dll",
                ],
                &["FrameViewSDK", "FrameView", "PresentMon"],
            ),
            // Do not match NvWGF2UMX*.dll: nvwgf2umx.dll is a core display user-mode driver.
            c(
                "Shield",
                "SHIELD / streaming / wireless controller support",
                true,
                false,
                &["*NvStream*", "*Shield*", "*SHIELD*", "*WirelessController*"],
                &["NvStream", "SHIELD", "Shield", "WirelessController"],
            ),
            // ProgramData\NvVAD is matched by exact leaf. Avoid broad '*NvVAD*' because it matched DriverStore package sidecars.
            c(
                "VirtualAudio",
                "NVIDIA virtual audio device",
                false,
                true,
                &["NvVAD*.dll", "NvVAD*.exe", "NvVAD*.sys", "nvvad*.sys"],
                &["NvVAD"],
            ),
            c(
                "USBTypeC",
                "USB-C / virtual host controller support",
                true,
                false,
                &[
                    "*USB-C*",
                    "*NvUSB*",
                    "*NvvHCI*",
                    "nvvhci*.inf",
                    "nvvhci*.sys",
                    "*nvppc*",
                ],
                &["NvvHCI", "USB-C", "NvUSB"],
            ),
            // Do not use broad '*VR*': it matched CUDA nvrtc*, NvRules*, and VRAM documentation.
            c(
                "Legacy3DVisionVR",
                "Legacy 3D Vision / Stereo extras",
                true,
                false,
                &["*3D Vision*", "*Stereo*", "nvst*.dll"],
                &["3D Vision", "Stereo"],
            ),
            // NVWMI is a management interface and is opt-in via NvWMI.
            c(
                "NvWMI",
                "NVIDIA WMI management interface",
                false,
                true,
                &["nvwmi*.dll", "nvwmi*.exe", "nvwmi*.mof"],
                &["NVWMI"],
            ),
            c(
                "NGX",
                "NGX / DLSS runtime cache and plugins",
                false,
                true,
                &["nvngx*.dll", "*NGX*"],
                &["NGX", "NvNGX"],
            ),
            c(
                "HDAudio",
                "NVIDIA HD Audio",
                false,
                true,
                &[
                    "nvhda*.inf",
                    "nvhda*.sys",
                    "*HDAudio*",
                    "*HD Audio*",
                    "*NvHDA*",
                ],
                &["HDAudio", "HD Audio", "NvHDA"],
            ),
            c(
                "PhysX",
                "NVIDIA PhysX",
                false,
                true,
                &["*PhysX*"],
                &["PhysX"],
            ),
            c(
                "NotebookOptimus",
                "Notebook/Optimus/MSI helper components",
                false,
                true,
                &["*Optimus*", "*NvOptimus*", "nvdmi*.inf", "*NvMsi*"],
                &["Optimus", "NvOptimus", "NvMsi"],
            ),
        ]
    })
}
