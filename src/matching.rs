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

/// Port of `isBloatProcessName`.
pub fn is_bloat_process_name(exe: &str) -> bool {
    const NAMES: &[&str] = &[
        "nvtelemetrycontainer.exe",
        "nvidia share.exe",
        "nvidia web helper.exe",
        "nvidia app.exe",
        "nvbackend.exe",
        "nvnodejslauncher.exe",
        "nvsphelper64.exe",
        "nvstreamservice.exe",
        "nvstreamnetworkservice.exe",
        "nvstreamuseragent.exe",
        "nvidia overlay.exe",
        "nvidia broadcast.exe",
        "frameview.exe",
        "presentmon.exe",
    ];
    let l = to_lower(exe);
    if NAMES.iter().any(|n| *n == l) {
        return true;
    }
    contains_no_case(exe, "telemetry")
        || contains_no_case(exe, "shadowplay")
        || contains_no_case(exe, "frameview")
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
