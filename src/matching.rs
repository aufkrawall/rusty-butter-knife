//! NVIDIA matching predicates — THE SAFETY CORE.
//! Port of the C++ predicate layer (`isNvidiaContextString`,
//! `isPreservedContainerName`, `isBloatProcessName`, DriverStore guards,
//! `matchComponentForPath`, ...). Fail-closed direction is mandatory here:
//! when in doubt, a path must NOT match for deletion.

use std::collections::BTreeMap;
use std::path::Path;

use crate::components::build_components;
use crate::types::Component;
use crate::util::{contains_no_case, to_lower, wildcard_match_no_case};

/// Lowercased wide-string view of a path (shared with discovery/deletion).
pub fn path_wide_lower(p: &Path) -> String {
    to_lower(&p.to_string_lossy())
}

pub type EnabledMap = BTreeMap<String, bool>;

/// Port of `isNvidiaContextString`.
pub fn is_nvidia_context_string(s: &str) -> bool {
    let l = to_lower(s);
    if l.contains("nvidia")
        || l.contains("geforce")
        || l.contains("frameview")
        || l.contains("shadowplay")
        || l.contains("displaydriverras")
    {
        return true;
    }
    // Bare 'nv' substring matching also hits words like 'Inventory',
    // 'Invoice' or 'Convergence'. Require an alphanumeric token that starts
    // with nv instead (nvcontainer, nvstream, nvcamera, ...).
    let chars: Vec<char> = l.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        let start = i;
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        if i > start && chars[start..].starts_with(&['n', 'v']) {
            let len = i - start;
            // "NV" itself or nvXXXX tokens.
            if len == 2 || len >= 4 {
                return true;
            }
        }
    }
    false
}

/// Port of `isPreservedContainerName`.
pub fn is_preserved_container_name(name_or_path: &str) -> bool {
    let l = to_lower(name_or_path);
    l.contains("nvdisplay.container") || l.contains("nvcontainer.exe")
}

/// Port of `moduleIsTelemetryOrUpdater` (matches on the file leaf).
pub fn module_is_telemetry_or_updater(path: &str) -> bool {
    const GLOBS: &[&str] = &[
        "*Telemetry*",
        "_DisplayDriverRAS.dll",
        "_NvMsgBusBroadcast.dll",
        "_nvtopps.dll",
        "_NvGSTPlugin.dll",
        "nvprofileupdaterplugin.dll",
        "*ProfileUpdater*",
        "*DriverUpdate*",
    ];
    let leaf = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    GLOBS.iter().any(|g| wildcard_match_no_case(&leaf, g))
}

/// NVIDIA ships driver packages as e.g. nv_dispi.inf_amd64_<hash> (x64) or
/// ...inf_arm64_<hash> (ARM64). Recognize both so package-root protection and
/// sidecar detection work on either architecture.
pub fn leaf_has_inf_arch_marker(leaf_lower: &str) -> bool {
    leaf_lower.contains(".inf_amd64") || leaf_lower.contains(".inf_arm64")
}

fn leaf_lower(p: &Path) -> String {
    to_lower(
        &p.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// Port of `isDriverStoreSidecarIni`.
pub fn is_driver_store_sidecar_ini(p: &Path) -> bool {
    let full = path_wide_lower(p);
    let leaf = leaf_lower(p);
    full.contains("\\windows\\system32\\driverstore\\filerepository\\")
        && leaf_has_inf_arch_marker(&leaf)
        && leaf.len() >= 4
        && leaf.ends_with(".ini")
}

/// Port of `isDriverStorePackageRootName`.
pub fn is_driver_store_package_root_name(p: &Path) -> bool {
    let parent = to_lower(
        &p.parent()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    parent.contains("\\windows\\system32\\driverstore\\filerepository")
        && leaf_has_inf_arch_marker(&leaf_lower(p))
}

/// Port of `isDeveloperToolPath`.
pub fn is_developer_tool_path(p: &Path) -> bool {
    let full = path_wide_lower(p);
    full.contains("\\nvidia gpu computing toolkit\\")
        || full.contains("\\cuda\\")
        || full.contains("\\nsight compute")
        || full.contains("\\nsight systems")
}

/// Port of `isNgxPathWithoutNgxSelection`.
pub fn is_ngx_path_without_ngx_selection(p: &Path, enabled: &EnabledMap) -> bool {
    let full = path_wide_lower(p);
    // Match the NGX directories themselves as well as anything below them.
    let under_ngx = full.contains("\\programdata\\nvidia\\ngx\\")
        || full.contains("\\nvidia corporation\\ngx\\")
        || full.ends_with("\\programdata\\nvidia\\ngx")
        || full.ends_with("\\nvidia corporation\\ngx");
    if !under_ngx {
        return false;
    }
    enabled.get("NGX").copied().map(|v| !v).unwrap_or(true)
}

const CORE_DISPLAY_LEAVES: &[&str] = &[
    "nvwgf2umx.dll",
    "nvwgf2um.dll",
    "nvd3dumx.dll",
    "nvd3dum.dll",
    "nvldumdx.dll",
    "nvapi.dll",
    "nvapi64.dll",
    "nvlddmkm.sys",
    "nvdispig.inf",
    "nv_dispi.inf",
];

/// Port of `isCoreDisplayDriverLeaf`.
pub fn is_core_display_driver_leaf(p: &Path) -> bool {
    CORE_DISPLAY_LEAVES.contains(&leaf_lower(p).as_str())
}

/// Port of `isExcludedCandidatePath`.
pub fn is_excluded_candidate_path(p: &Path, enabled: &EnabledMap) -> bool {
    is_developer_tool_path(p)
        || is_driver_store_package_root_name(p)
        || is_driver_store_sidecar_ini(p)
        || is_ngx_path_without_ngx_selection(p, enabled)
        || is_core_display_driver_leaf(p)
}

/// Port of `shouldPruneTraversal`.
pub fn should_prune_traversal(p: &Path, enabled: &EnabledMap) -> bool {
    is_developer_tool_path(p) || is_ngx_path_without_ngx_selection(p, enabled)
}

/// Port of `matchComponentForPath`. `enabled` is a snapshot of the
/// component-selection map taken by discovery.
pub fn match_component_for_path(p: &Path, enabled: &EnabledMap) -> Option<&'static Component> {
    if is_excluded_candidate_path(p, enabled) {
        return None;
    }

    // Installer2 is only safe as NVIDIA Corporation\Installer2, not any arbitrary Installer2.
    let installer2_guard = |c_key: &str, full: &str| {
        c_key != "Installer2Cache" || contains_no_case(full, "NVIDIA Corporation")
    };

    let leaf = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let full = p.to_string_lossy();

    for c in build_components() {
        if !enabled.get(&c.key).copied().unwrap_or(false) {
            continue;
        }
        for exact in &c.exact_leaf_names {
            if exact.eq_ignore_ascii_case(&leaf) && installer2_guard(&c.key, &full) {
                return Some(c);
            }
        }
        for glob in &c.leaf_globs {
            if wildcard_match_no_case(&leaf, glob) {
                return Some(c);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Action-target classification (audit finding SAFETY-01)
// ---------------------------------------------------------------------------

/// One decision type shared by every mutation entry point: file discovery
/// (match_component_for_path), process killing, service handling and
/// scheduled-task handling all route through `ActionDecision` so that a
/// deselected component prevents ALL associated mutations — files, processes,
/// services AND tasks. Fail closed: unmatched targets are never mutated.
#[derive(Debug)]
pub enum ActionDecision {
    /// No component claims this target; never mutate.
    NotMatched,
    /// A component owns this target but is deselected; never mutate.
    ComponentDisabled(&'static Component),
    /// A component owns this target and is enabled.
    Allowed(&'static Component),
}

impl ActionDecision {
    /// Owning component's catalog key when this decision permits mutation
    /// (test helper; production sites pattern-match exhaustively instead).
    #[cfg(test)]
    pub fn allowed_key(&self) -> Option<&'static str> {
        match self {
            ActionDecision::Allowed(c) => Some(&c.key),
            _ => None,
        }
    }

    /// Resolve against the user's component selection.
    pub fn resolve(key: &str, enabled: &EnabledMap) -> ActionDecision {
        match build_components().iter().find(|c| c.key == key) {
            Some(c) => {
                if enabled.get(c.key.as_str()).copied().unwrap_or(false) {
                    ActionDecision::Allowed(c)
                } else {
                    ActionDecision::ComponentDisabled(c)
                }
            }
            // Unknown catalog key: fail closed.
            None => ActionDecision::NotMatched,
        }
    }
}

/// Map one killable bloat process name (any case) to its owning component
/// key. Mirrors the historically matched names of `isBloatProcessName`.
fn classify_process_key(exe: &str) -> Option<&'static str> {
    const EXACT_NAMES: &[(&str, &str)] = &[
        ("nvtelemetrycontainer.exe", "Telemetry"),
        ("nvidia share.exe", "ShadowPlayShare"),
        ("nvsphelper64.exe", "ShadowPlayShare"),
        ("nvidia web helper.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvidia app.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvbackend.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvnodejslauncher.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvidia overlay.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvidia broadcast.exe", "GeForceExperienceAndNvidiaApp"),
        ("nvstreamservice.exe", "Shield"),
        ("nvstreamnetworkservice.exe", "Shield"),
        ("nvstreamuseragent.exe", "Shield"),
        ("frameview.exe", "FrameView"),
        ("presentmon.exe", "FrameView"),
    ];
    let l = to_lower(exe);
    for (name, key) in EXACT_NAMES {
        if *name == l {
            return Some(key);
        }
    }
    // Substring fallbacks preserving the historical containment rules.
    if contains_no_case(exe, "telemetry") {
        return Some("Telemetry");
    }
    if contains_no_case(exe, "shadowplay") {
        return Some("ShadowPlayShare");
    }
    if contains_no_case(exe, "frameview") {
        return Some("FrameView");
    }
    if contains_no_case(exe, "nvstream") {
        return Some("Shield");
    }
    None
}

/// Component ownership for one killable process. Container-preservation is
/// applied here so no other layer can accidentally bypass it.
pub fn process_action_decision(
    exe: &str,
    enabled: &EnabledMap,
    preserve_containers: bool,
) -> ActionDecision {
    if preserve_containers && is_preserved_container_name(exe) {
        return ActionDecision::NotMatched;
    }
    match classify_process_key(exe) {
        Some(key) => ActionDecision::resolve(key, enabled),
        None => ActionDecision::NotMatched,
    }
}

/// Term list mapping service/task display or key names onto owning components
/// ((term, component key)). Order matters: first hit wins.
const NAME_TERMS: &[(&str, &str)] = &[
    ("telemetry", "Telemetry"),
    ("displaydriverras", "Telemetry"),
    ("profileupdater", "UpdateAndProfileUpdater"),
    ("profile updater", "UpdateAndProfileUpdater"),
    ("update", "UpdateAndProfileUpdater"),
    ("frameview", "FrameView"),
    ("presentmon", "FrameView"),
    ("shadowplay", "ShadowPlayShare"),
    ("share", "ShadowPlayShare"),
    ("nvstream", "Shield"),
    ("ansel", "AnselCamera"),
    ("nvvad", "VirtualAudio"),
    ("nvwmi", "NvWMI"),
    ("nvidia app", "GeForceExperienceAndNvidiaApp"),
    ("geforce experience", "GeForceExperienceAndNvidiaApp"),
    ("broadcast", "GeForceExperienceAndNvidiaApp"),
    ("geforce", "GeForceExperienceAndNvidiaApp"),
];

fn classify_named_target(combined: &str) -> Option<&'static str> {
    let l = to_lower(combined);
    NAME_TERMS
        .iter()
        .find(|(term, _)| l.contains(term))
        .map(|(_, key)| *key)
}

/// Component ownership for one Windows service.
pub fn service_action_decision(
    service_name: &str,
    display_name: &str,
    enabled: &EnabledMap,
    preserve_containers: bool,
) -> ActionDecision {
    let combined = format!("{service_name} {display_name}");
    if preserve_containers && is_preserved_container_name(&combined) {
        return ActionDecision::NotMatched;
    }
    if !is_nvidia_context_string(&combined) {
        return ActionDecision::NotMatched;
    }
    match classify_named_target(&combined) {
        Some(key) => ActionDecision::resolve(key, enabled),
        None => ActionDecision::NotMatched,
    }
}

/// Component ownership for one scheduled task (matched by absolute task
/// path/name).
pub fn task_action_decision(task_name: &str, enabled: &EnabledMap) -> ActionDecision {
    if !is_nvidia_context_string(task_name) {
        return ActionDecision::NotMatched;
    }
    match classify_named_target(task_name) {
        Some(key) => ActionDecision::resolve(key, enabled),
        None => ActionDecision::NotMatched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn enabled_of(keys: &[&str]) -> EnabledMap {
        keys.iter().map(|k| (k.to_string(), true)).collect()
    }

    #[test]
    fn process_classification_maps_to_components() {
        let on = enabled_of(&["Telemetry", "Shield", "ShadowPlayShare", "FrameView"]);
        assert_eq!(
            process_action_decision("NvTelemetryContainer.exe", &on, true).allowed_key(),
            Some("Telemetry")
        );
        assert_eq!(
            process_action_decision("NvStreamService.exe", &on, true).allowed_key(),
            Some("Shield")
        );
        assert!(
            process_action_decision("PresentMon-Main.exe", &on, true)
                .allowed_key()
                .is_none(),
            "exact names only; PresentMon-<suffix> is not an exact bloat name"
        );
    }

    #[test]
    fn process_kill_blocked_when_component_disabled() {
        let off: EnabledMap = BTreeMap::new(); // nothing enabled
        match process_action_decision("nvtelemetrycontainer.exe", &off, false) {
            ActionDecision::ComponentDisabled(c) => assert_eq!(c.key, "Telemetry"),
            other => panic!("expected disabled, got {other:?}"),
        }
        // Preserved container never becomes a kill target.
        let all = enabled_of(&["Telemetry"]);
        assert!(matches!(
            process_action_decision("NVDisplay.Container.exe", &all, true),
            ActionDecision::NotMatched
        ));
    }

    #[test]
    fn service_classification_gates_by_component() {
        let all = enabled_of(&[
            "Telemetry",
            "UpdateAndProfileUpdater",
            "VirtualAudio",
            "AnselCamera",
            "GeForceExperienceAndNvidiaApp",
        ]);
        assert_eq!(
            service_action_decision(
                "NvTelemetryContainer",
                "NVIDIA Telemetry Container",
                &all,
                true
            )
            .allowed_key(),
            Some("Telemetry")
        );
        assert_eq!(
            service_action_decision("NvBackend", "NVIDIA Backend", &all, true).allowed_key(),
            None,
            "unclassified NVIDIA-context service fails closed"
        );
        // Unrelated services keep the historical token-context guard.
        assert!(matches!(
            service_action_decision("InventorySvc", "Inventory", &all, true),
            ActionDecision::NotMatched
        ));
        // nvvad opt-in rule preserved (no NVIDIA context otherwise).
        assert_eq!(
            service_action_decision("nvvadsvc", "NVIDIA Virtual Audio", &all, true).allowed_key(),
            Some("VirtualAudio")
        );
        // Deselected component => ComponentDisabled, no mutation.
        let none: EnabledMap = BTreeMap::new();
        assert!(matches!(
            service_action_decision("NvTelemetryContainer", "NVIDIA Telemetry", &none, false),
            ActionDecision::ComponentDisabled(_)
        ));
    }

    #[test]
    fn task_classification_gates_by_component() {
        let all = enabled_of(&["Telemetry", "UpdateAndProfileUpdater", "VirtualAudio", "NvWMI"]);
        assert_eq!(
            task_action_decision("\\NvTmrep\\NVIDIA Telemetry Task", &all).allowed_key(),
            Some("Telemetry")
        );
        assert_eq!(
            task_action_decision("\\NvVAD\\Virtual Audio Task", &all).allowed_key(),
            Some("VirtualAudio")
        );
        assert_eq!(
            task_action_decision("\\NvWMI\\WMI Monitor", &all).allowed_key(),
            Some("NvWMI")
        );
        assert!(matches!(
            task_action_decision("\\Inventory\\Scan", &all),
            ActionDecision::NotMatched
        ));
        let none: EnabledMap = BTreeMap::new();
        assert!(matches!(
            task_action_decision("\\NvNode\\NVIDIA Update Task", &none),
            ActionDecision::ComponentDisabled(_)
        ));
        assert!(matches!(
            task_action_decision("\\NvWMI\\WMI Monitor", &none),
            ActionDecision::ComponentDisabled(_)
        ));
    }

    fn win(path_forward: &str) -> PathBuf {
        PathBuf::from(path_forward.replace("/", "\\"))
    }

    fn only(keys: &[&str]) -> EnabledMap {
        keys.iter().map(|k| (k.to_string(), true)).collect()
    }

    #[test]
    fn inf_arch_marker_recognized_on_supported_arches() {
        assert!(leaf_has_inf_arch_marker("nv_dispi.inf_amd64_0373"));
        assert!(leaf_has_inf_arch_marker("nvhda.inf_arm64_9007"));
        assert!(!leaf_has_inf_arch_marker("nv_dispi.inf"));
        // x86 legacy packages are not recognized (documents intent).
        assert!(!leaf_has_inf_arch_marker("nvdimm.inf_x86_1"));
    }

    #[test]
    fn package_root_and_sidecar_detection() {
        assert!(is_driver_store_package_root_name(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0")));
        assert!(is_driver_store_sidecar_ini(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0.ini")));
        assert!(!is_driver_store_sidecar_ini(&win(
            "C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nvhda.inf"
        )));
        // A payload FILE inside a package is not itself a package root.
        assert!(!is_driver_store_package_root_name(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0\\nvvhci.sys")));
        assert!(!is_driver_store_sidecar_ini(&win(
            "C:\\temp\\nv_dispi.inf_amd64_1.ini"
        )));
    }

    #[test]
    fn core_display_leaves_protected() {
        assert!(is_core_display_driver_leaf(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0\\nvlddmkm.sys")));
        assert!(!is_core_display_driver_leaf(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0\\nvsmartmax.sys")));
    }

    #[test]
    fn nvidia_context_string_token_rule() {
        assert!(
            is_nvidia_context_string("NVIDIA Telemetry"),
            "NVIDIA Telemetry"
        );
        assert!(
            is_nvidia_context_string("nvcontainer.exe"),
            "nvcontainer.exe"
        );
        assert!(
            is_nvidia_context_string("NVDisplay.Container"),
            "NVDisplay.Container"
        );
        assert!(is_nvidia_context_string("geforce"), "geforce");
        assert!(is_nvidia_context_string("nv"), "nv");
        assert!(is_nvidia_context_string("nvsphelper64"), "nvsphelper64");
        assert!(is_nvidia_context_string("NvFBC64.dll"), "NvFBC64.dll");
        assert!(!is_nvidia_context_string("inventory"), "inventory");
        assert!(!is_nvidia_context_string("convergence"), "convergence");
        assert!(!is_nvidia_context_string("invoice"), "invoice");
        assert!(!is_nvidia_context_string("nvx"), "nvx");
        assert!(!is_nvidia_context_string("service"), "service");
        assert!(!is_nvidia_context_string("winvnc"), "winvnc");
        // Bare 2-letter nv tokens count; 3-letter tokens deliberately do not.
    }

    #[test]
    fn preserved_container_names() {
        assert!(is_preserved_container_name("NVDisplay.Container.exe"));
        assert!(is_preserved_container_name("path\\to\\nvcontainer.exe"));
        assert!(!is_preserved_container_name("nvcamera.exe"));
    }

    #[test]
    fn module_telemetry_globs() {
        assert!(
            module_is_telemetry_or_updater("NvTelemetry.dll"),
            "NvTelemetry.dll"
        );
        assert!(
            module_is_telemetry_or_updater("x_Telemetry_y.dll"),
            "x_Telemetry_y.dll"
        );
        assert!(
            module_is_telemetry_or_updater("_DisplayDriverRAS.dll"),
            "_DisplayDriverRAS.dll"
        );
        assert!(
            module_is_telemetry_or_updater("_NvMsgBusBroadcast.dll"),
            "_NvMsgBusBroadcast.dll"
        );
        assert!(
            module_is_telemetry_or_updater("_nvtopps.dll"),
            "_nvtopps.dll"
        );
        assert!(
            module_is_telemetry_or_updater("_NvGSTPlugin.dll"),
            "_NvGSTPlugin.dll"
        );
        assert!(
            module_is_telemetry_or_updater("nvprofileupdaterplugin.dll"),
            "nvprofileupdaterplugin.dll"
        );
        assert!(
            module_is_telemetry_or_updater("NvProfileUpdaterPlugin.dll"),
            "NvProfileUpdaterPlugin.dll"
        );
        assert!(!module_is_telemetry_or_updater("nvcuda.dll"));
    }

    #[test]
    fn component_matching_and_exclusions() {
        let m = only(&["Installer2Cache"]);
        assert!(match_component_for_path(
            &win("C:\\Program Files\\NVIDIA Corporation\\Installer2"),
            &m
        )
        .is_some());
        // Installer2 outside NVIDIA Corporation must never match.
        assert!(match_component_for_path(&win("C:\\somewhere\\Installer2"), &m).is_none());
        let m = only(&["USBTypeC"]);
        let got = match_component_for_path(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0\\nvvhci.sys"), &m).map(|c| c.key.clone());
        assert_eq!(got.as_deref(), Some("USBTypeC"));
        // Core display leaves stay unmatched even when a glob would hit.
        let m = only(&["Legacy3DVisionVR", "Shield"]);
        assert!(match_component_for_path(&win("C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_0373d825005116d0\\nvwgf2umx.dll"), &m).is_none());
        // Developer-tool paths are pruned from matching entirely.
        let m = only(&["AnselCamera"]);
        assert!(match_component_for_path(&win("C:\\cuda\\NvCamera.dll"), &m).is_none());
        // NGX paths are excluded while NGX is deselected...
        let m = only(&["NGX"]);
        let disabled: BTreeMap<String, bool> = BTreeMap::new();
        assert!(match_component_for_path(
            &win("C:\\ProgramData\\NVIDIA\\ngx\\nvngx.dll"),
            &disabled
        )
        .is_none());
        // ...and match once NGX is enabled.
        let got = match_component_for_path(&win("C:\\ProgramData\\NVIDIA\\ngx\\nvngx.dll"), &m)
            .map(|c| c.key.clone());
        assert_eq!(got.as_deref(), Some("NGX"));
        let m = only(&["Shield"]);
        assert!(match_component_for_path(
            &win("C:\\Program Files\\NVIDIA Corporation\\NvStreamService.exe"),
            &m
        )
        .is_some());
    }
}
