//! Task Scheduler COM bindings — part of the FFI boundary module family.
//! Same policy as `ffi.rs`: the only place where `unsafe` is permitted,
//! every call wrapped in a safe, documented function.

#![allow(unsafe_code)]

use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::TaskScheduler::{
    CLSID_CTaskScheduler, IExecAction, IRegisteredTask, ITaskFolder, ITaskService,
    TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE, TASK_ENUM_HIDDEN, TASK_INSTANCES_IGNORE_NEW,
    TASK_LOGON_S4U, TASK_RUNLEVEL_HIGHEST, TASK_STATE_QUEUED, TASK_STATE_RUNNING,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4};

/// Raw HRESULT carrier for safe-wrapper results.
#[derive(Debug, Clone, Copy)]
pub struct HrError(pub i32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TiState {
    Running,
    Queued,
    Other(i32),
}

fn var_empty() -> VARIANT {
    // VT_EMPTY: all-zero VARIANT.
    unsafe { std::mem::zeroed() }
}

fn var_i32(v: i32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: writing a well-formed VT_I4 variant; no owned resources.
    // (ManuallyDrop is repr(transparent), so the pointer cast is layout-true.)
    unsafe {
        let inner = (&mut var.Anonymous.Anonymous
            as *mut core::mem::ManuallyDrop<windows::Win32::System::Variant::VARIANT_0_0>)
            .cast::<windows::Win32::System::Variant::VARIANT_0_0>();
        (*inner).vt = VT_I4;
        (*inner).Anonymous.lVal = v;
    }
    var
}

fn var_bstr(s: &str) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: constructing a VT_BSTR variant that owns the BSTR via
    // ManuallyDrop; the COM callee copies what it needs and our transient
    // copy leaks only on early process exit (same lifetime as _variant_t).
    unsafe {
        let inner = (&mut var.Anonymous.Anonymous
            as *mut core::mem::ManuallyDrop<windows::Win32::System::Variant::VARIANT_0_0>)
            .cast::<windows::Win32::System::Variant::VARIANT_0_0>();
        (*inner).vt = VT_BSTR;
        (*inner).Anonymous.bstrVal = core::mem::ManuallyDrop::new(BSTR::from(s));
    }
    var
}

pub struct RegisteredTiTask {
    task: IRegisteredTask,
}

impl RegisteredTiTask {
    pub fn poll_state(&self) -> Option<TiState> {
        // SAFETY: in-proc COM call on an owned interface pointer.
        let state = unsafe { self.task.State() }.ok()?;
        if state == TASK_STATE_RUNNING {
            Some(TiState::Running)
        } else if state == TASK_STATE_QUEUED {
            Some(TiState::Queued)
        } else {
            Some(TiState::Other(state.0))
        }
    }

    /// LastTaskResult when readable (port of get_LastTaskResult).
    pub fn last_result(&self) -> Option<i64> {
        // SAFETY: in-proc COM call on an owned interface pointer.
        unsafe { self.task.LastTaskResult() }.ok().map(i64::from)
    }

    pub fn stop(&self) {
        // SAFETY: in-proc COM call; failure is non-fatal (mirrors C++).
        let _ = unsafe { self.task.Stop(0) };
    }
}

pub struct TiSession {
    service: Option<ITaskService>,
    root: Option<ITaskFolder>,
}

impl TiSession {
    /// CoInitializeEx + CoCreateInstance(TaskScheduler) + Connect + GetFolder.
    pub fn connect() -> Result<TiSession, HrError> {
        // SAFETY: standard COM init; balanced by CoUninitialize in Drop.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(HrError(hr.0));
        }
        let svc_res: windows::core::Result<ITaskService> =
            // SAFETY: in-proc COM object creation with our class identifier.
            unsafe { CoCreateInstance(&CLSID_CTaskScheduler, None, CLSCTX_INPROC_SERVER) };
        let service = match svc_res {
            Ok(s) => s,
            Err(e) => {
                unsafe { CoUninitialize() };
                return Err(HrError(e.code().0));
            }
        };
        // Parameter order per IDL: servername, user, domain, password —
        // all empty here, matching the C++ `_variant_t()` calls.
        let connected =
            unsafe { service.Connect(&var_empty(), &var_empty(), &var_empty(), &var_empty()) };
        if connected.is_err() {
            unsafe { CoUninitialize() };
            return Err(HrError(connected.err().unwrap().code().0));
        }
        let root = unsafe { service.GetFolder(&BSTR::from("\\")) }.ok();
        Ok(TiSession {
            service: Some(service),
            root,
        })
    }

    pub fn root_available(&self) -> bool {
        self.root.is_some()
    }

    /// Delete scheduled tasks whose name starts with `prefix`; returns names.
    /// Port of the task half of `sweepStaleTiArtifacts`.
    pub fn sweep_stale_tasks(&self, prefix: &str) -> Vec<String> {
        let mut removed = Vec::new();
        let Some(root) = &self.root else {
            return removed;
        };
        // SAFETY: collection/interface calls on owned pointers; item refs are
        // released by Interface drop after each iteration.
        let tasks = unsafe { root.GetTasks(TASK_ENUM_HIDDEN.0) };
        let Ok(tasks) = tasks else {
            return removed;
        };
        let mut count = 0i32;
        if (unsafe { tasks.Count() }).map(|c| count = c).is_err() {
            return removed;
        }
        for i in (1..=count).rev() {
            // Backwards iteration: removal shifts indices.
            let item = unsafe { tasks.get_Item(&var_i32(i)) };
            let Ok(task) = item else { continue };
            let name_res = unsafe { task.Name() };
            if let Ok(name_bstr) = name_res {
                let name = name_bstr.to_string();
                if name.starts_with(prefix) {
                    // Stop BEFORE deleting: DeleteTask invalidates the
                    // IRegisteredTask of the deleted task (see C++ comment).
                    let _ = unsafe { task.Stop(0) };
                    drop(task);
                    if unsafe { root.DeleteTask(&name_bstr, 0) }.is_ok() {
                        removed.push(name);
                    }
                }
            }
        }
        removed
    }

    /// Remove a previous instance of `task_name` if present (Stop+Delete).
    pub fn delete_task_if_exists(&self, task_name: &str) {
        let Some(root) = &self.root else { return };
        let wname = BSTR::from(task_name);
        // SAFETY: owned interface calls; GetTask failure means "not present".
        if let Ok(existing) = unsafe { root.GetTask(&wname) } {
            let _ = unsafe { existing.Stop(0) };
        }
        let _ = unsafe { root.DeleteTask(&wname, 0) };
    }

    /// Register + run the one-shot TI child task. Returns the running-task
    /// handle for polling. Port of RegisterTaskDefinition + Run.
    pub fn register_and_run_ti_child(
        &self,
        task_name: &str,
        exe_path: &str,
        arguments_joined: &str,
        working_dir: &str,
    ) -> Result<RegisteredTiTask, HrError> {
        let Some(root) = &self.root else {
            return Err(HrError(-2147467259 /* E_FAIL */));
        };
        let service = self.service.as_ref().expect("service held with session");
        // SAFETY: all following in-proc COM calls use owned interfaces and
        // BSTR/VARIANT parameters that outlive each call.
        let def = unsafe { service.NewTask(0) }.map_err(|e| HrError(e.code().0))?;

        if let Ok(reg_info) = unsafe { def.RegistrationInfo() } {
            let _ = unsafe { reg_info.SetAuthor(&BSTR::from("NT AUTHORITY\\SYSTEM")) };
        }
        if let Ok(principal) = unsafe { def.Principal() } {
            let _ = unsafe { principal.SetUserId(&BSTR::from("NT SERVICE\\TrustedInstaller")) };
            let _ = unsafe { principal.SetLogonType(TASK_LOGON_S4U) };
            let _ = unsafe { principal.SetRunLevel(TASK_RUNLEVEL_HIGHEST) };
        }
        if let Ok(settings) = unsafe { def.Settings() } {
            let _ = unsafe { settings.SetEnabled(true.into()) };
            let _ = unsafe { settings.SetAllowDemandStart(true.into()) };
            let _ = unsafe { settings.SetDisallowStartIfOnBatteries(false.into()) };
            let _ = unsafe { settings.SetStopIfGoingOnBatteries(false.into()) };
            let _ = unsafe { settings.SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW) };
            let _ = unsafe { settings.SetExecutionTimeLimit(&BSTR::from("PT2H")) };
            let _ = unsafe { settings.SetStartWhenAvailable(false.into()) };
        }
        if let Ok(actions) = unsafe { def.Actions() } {
            if let Ok(action) = unsafe { actions.Create(TASK_ACTION_EXEC) } {
                if let Ok(exec) = action.cast::<IExecAction>() {
                    let _ = unsafe { exec.SetPath(&BSTR::from(exe_path)) };
                    let _ = unsafe { exec.SetArguments(&BSTR::from(arguments_joined)) };
                    let _ = unsafe { exec.SetWorkingDirectory(&BSTR::from(working_dir)) };
                }
            }
        }

        let registered = unsafe {
            root.RegisterTaskDefinition(
                &BSTR::from(task_name),
                &def,
                TASK_CREATE_OR_UPDATE.0,
                &var_bstr("NT SERVICE\\TrustedInstaller"),
                &var_empty(),
                TASK_LOGON_S4U,
                &var_bstr(""),
            )
        }
        .map_err(|e| HrError(e.code().0))?;

        // Mirrors C++ releasing IRunningTask immediately after Run succeeds.
        let _run = unsafe { registered.Run(&var_empty()) }.map_err(|e| HrError(e.code().0))?;
        drop(_run);
        Ok(RegisteredTiTask { task: registered })
    }

    /// Stop and delete the task after completion/failure.
    pub fn finish_task(&self, task: &RegisteredTiTask, task_name: &str) {
        task.stop();
        self.delete_task_if_exists(task_name);
    }
}

impl Drop for TiSession {
    fn drop(&mut self) {
        // Interfaces release first (field drop order), then COM apartment.
        self.root = None;
        self.service = None;
        unsafe { CoUninitialize() };
    }
}
