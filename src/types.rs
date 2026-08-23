//! Data types mirroring the C++ structs (`Options`, `Component`, `Candidate`,
//! `ActionRecord`, `RunState`).

use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Options {
    pub execute: bool,
    pub menu: bool,
    /// bare launch: wizard presets applied
    pub wizard_defaults: bool,
    /// --pause: force a final "Press Enter"
    pub pause_on_exit: bool,
    pub ti_child: bool,
    pub allow_admin_fallback: bool,
    pub attempt_ti_relaunch: bool,
    pub kill_lockers: bool,
    pub preserve_nv_containers: bool,
    pub disable_services: bool,
    pub delete_services: bool,
    pub disable_scheduled_tasks: bool,
    pub delete_scheduled_tasks: bool,
    pub schedule_locked_for_reboot: bool,
    pub take_ownership: bool,
    pub include_ngx: bool,
    pub include_hd_audio: bool,
    pub include_physx: bool,
    pub include_notebook_optimus: bool,
    pub include_virtual_audio: bool,
    pub include_nvwmi: bool,
    pub include_capture_sdk: bool,
    pub no_pause: bool,
    pub no_color: bool,
    pub ti_wait_seconds: i64,
    pub status_file: String,
    pub log_dir_override: String,
    pub log_file_override: String,
    pub show_help: bool,
    pub show_version: bool,
    pub list_components: bool,
    pub unknown_args: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            execute: false,
            menu: false,
            wizard_defaults: false,
            pause_on_exit: false,
            ti_child: false,
            allow_admin_fallback: false,
            attempt_ti_relaunch: true,
            kill_lockers: false,
            preserve_nv_containers: true,
            disable_services: false,
            delete_services: false,
            disable_scheduled_tasks: true,
            delete_scheduled_tasks: false,
            schedule_locked_for_reboot: false,
            take_ownership: false,
            include_ngx: false,
            include_hd_audio: false,
            include_physx: false,
            include_notebook_optimus: false,
            include_virtual_audio: false,
            include_nvwmi: false,
            include_capture_sdk: false,
            no_pause: false,
            no_color: false,
            ti_wait_seconds: 600,
            status_file: String::new(),
            log_dir_override: String::new(),
            log_file_override: String::new(),
            show_help: false,
            show_version: false,
            list_components: false,
            unknown_args: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Component {
    pub key: String,
    pub display_name: String,
    pub default_enabled: bool,
    pub optional: bool,
    pub leaf_globs: Vec<String>,
    pub exact_leaf_names: Vec<String>,
}

#[derive(Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub component_key: String,
    /// Kept for parity with the legacy Candidate layout / future reporting.
    #[allow(dead_code)]
    pub component_name: String,
    pub is_directory: bool,
}

#[derive(Clone)]
pub struct ActionRecord {
    pub kind: String,
    pub status: String,
    pub component: String,
    pub path: String,
    pub detail: String,
}

#[derive(Default)]
pub struct RunState {
    pub exe_path: PathBuf,
    pub exe_dir: PathBuf,
    pub run_id: String,
    pub log_path: PathBuf,
    pub actions: Vec<ActionRecord>,
    pub candidates: Vec<Candidate>,
    pub aborted: bool,
    // Filled by verify_candidate_removal() after processing.
    pub post_run_check_done: bool,
    pub paths_remaining_after_run: i64,
    pub paths_pending_reboot: i64,
    // Filled by tally_previous_logs() at start of cleanup.
    pub history_scan_done: bool,
    pub history_log_files: i64,
    pub history_runs: i64,
    pub history_candidates: i64,
    // Dedup sets for the two-phase NVIDIA-container module inspection.
    pub reported_containers: HashSet<String>,
    pub reported_modules: HashSet<String>,
}
