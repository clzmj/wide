//! Explicit local recovery attempts, separate from DDC and normal layout changes.
use crate::{
    display::Display,
    resolution::{self, ResolutionRole},
};
use serde_json::{json, Value};

pub fn retime(
    display: &Display,
    role: Option<ResolutionRole>,
    confirm: bool,
) -> Result<Value, String> {
    let _bus = if confirm {
        Some(crate::transport::lock_bus(display.registry_id)?)
    } else {
        None
    };
    let id = display
        .cg_display_id
        .filter(|id| *id != 0)
        .ok_or("macOS has not exposed this connection's display modes.")?;
    let before = resolution::current(id)?;
    let modes = resolution::enumerate(id)?;
    let requested = match role {
        Some(role) => resolution::choose(role, &modes, &before)?,
        None => before.clone(),
    };
    let would_change = requested != before;
    // Deliberately apply once even if this mode is already current. No alternate
    // mode, retry, DDC command, or automatic rollback is part of this operation.
    let after = if confirm {
        Some(resolution::apply(id, requested.id)?)
    } else {
        None
    };
    let changed = after.as_ref().is_some_and(|mode| mode != &before);
    Ok(json!({
        "operation": "retime",
        "scope": "this_mac_only",
        "cg_display_id": id,
        "registry_id": display.registry_id,
        "role": role,
        "dry_run": !confirm,
        "mode_apply_attempts": u8::from(confirm),
        "before": before,
        "requested": requested,
        "after": after,
        "would_change": would_change,
        "changed": changed,
        "reapplied": confirm && !would_change,
        "verified": confirm,
        "physical_reconnect_verified": false,
        "warning": "This attempts one session-scoped macOS mode transaction. A soft reconnect or restored DDC communication is not guaranteed."
    }))
}

pub fn cycle(display: &Display, confirm: bool) -> Result<Value, String> {
    #[cfg(target_os = "macos")]
    {
        mac::cycle(display, confirm)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (display, confirm);
        Err("Display cycling requires macOS".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::{
        ffi::{c_char, c_void},
        thread,
        time::{Duration, Instant},
    };

    // Exact config-first signatures from the checked-in OpenDisplay
    // ExperimentalLifecycleProvider. Never pass public CG configuration objects.
    type Begin = unsafe extern "C" fn(*mut *mut c_void) -> i32;
    type Configure = unsafe extern "C" fn(*mut c_void, u32, bool) -> i32;
    type Complete = unsafe extern "C" fn(*mut c_void, i32) -> i32;
    type Cancel = unsafe extern "C" fn(*mut c_void) -> i32;
    extern "C" {
        fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> i32;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    struct Library(*mut c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    dlclose(self.0);
                }
            }
        }
    }

    fn active() -> Result<Vec<u32>, String> {
        let mut count = 0;
        let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
        if status != 0 || count > 128 {
            return Err(format!("Cannot count active displays: {status}"));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![0; count as usize];
        let capacity = count;
        let status = unsafe { CGGetActiveDisplayList(capacity, ids.as_mut_ptr(), &mut count) };
        if status != 0 || count > capacity {
            return Err(format!("Active display list changed or failed: {status}"));
        }
        ids.truncate(count as usize);
        Ok(ids)
    }

    struct Functions {
        begin: Begin,
        configure: Configure,
        complete: Complete,
        cancel: Cancel,
    }
    impl Functions {
        fn set_enabled(&self, id: u32, enabled: bool) -> Result<(), String> {
            let mut config = std::ptr::null_mut();
            let status = unsafe { (self.begin)(&mut config) };
            if status != 0 || config.is_null() {
                return Err(format!("SLSBeginDisplayConfiguration failed: {status}"));
            }
            let status = unsafe { (self.configure)(config, id, enabled) };
            if status != 0 {
                let cancel = unsafe { (self.cancel)(config) };
                return Err(format!(
                    "SLSConfigureDisplayEnabled failed: {status}; cancel: {cancel}"
                ));
            }
            // Complete consumes the private configuration; 0 is app-only.
            let status = unsafe { (self.complete)(config, 0) };
            if status != 0 {
                return Err(format!(
                    "SLSCompleteDisplayConfigurationWithOption failed: {status}"
                ));
            }
            Ok(())
        }
    }

    pub fn cycle(display: &Display, confirm: bool) -> Result<Value, String> {
        let id = display
            .cg_display_id
            .filter(|id| *id != 0)
            .ok_or("macOS has not exposed this connection's display ID.")?;
        let before = active()?;
        let library = Library(unsafe {
            dlopen(
                c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight".as_ptr(),
                1,
            )
        });
        let names = [
            c"SLSBeginDisplayConfiguration",
            c"SLSConfigureDisplayEnabled",
            c"SLSCompleteDisplayConfigurationWithOption",
            c"SLSCancelDisplayConfiguration",
        ];
        let symbols: Vec<_> = names
            .iter()
            .map(|name| {
                if library.0.is_null() {
                    std::ptr::null_mut()
                } else {
                    unsafe { dlsym(library.0, name.as_ptr()) }
                }
            })
            .collect();
        let availability: serde_json::Map<String, Value> = names
            .iter()
            .zip(&symbols)
            .map(|(name, value)| (name.to_string_lossy().into_owned(), json!(!value.is_null())))
            .collect();
        let available = symbols.iter().all(|symbol| !symbol.is_null());
        let another_active = before.iter().any(|other| *other != id);
        let mut result = json!({"operation":"logical_display_cycle", "scope":"this_mac_only",
            "cg_display_id":id, "registry_id":display.registry_id, "dry_run":!confirm,
            "active_before":before, "symbols":availability, "symbols_available":available,
            "another_active_display":another_active, "disable":null,"enable":null,
            "inactive_observed":false,"target_active_after":null,"verified":false,
            "warning":"This private app-only transaction attempts a logical disconnect and re-enable. Physical reconnection and restored DDC communication are not guaranteed."});
        if !confirm {
            return Ok(result);
        }
        if !available {
            return Err("Required SkyLight symbols unavailable; no transaction attempted.".into());
        }
        if !before.contains(&id) {
            return Err("Target is not currently active; no cycle attempted.".into());
        }
        if !another_active {
            return Err(
                "Refusing to disable the last active display; no transaction attempted.".into(),
            );
        }
        let _bus = crate::transport::lock_bus(display.registry_id)?;
        let functions = unsafe {
            Functions {
                begin: std::mem::transmute::<*mut c_void, Begin>(symbols[0]),
                configure: std::mem::transmute::<*mut c_void, Configure>(symbols[1]),
                complete: std::mem::transmute::<*mut c_void, Complete>(symbols[2]),
                cancel: std::mem::transmute::<*mut c_void, Cancel>(symbols[3]),
            }
        };
        let disable = functions.set_enabled(id, false);
        let mut inactive_observed = false;
        let mut disable_observation_error = None;
        let started = Instant::now();
        if disable.is_ok() {
            loop {
                match active() {
                    Ok(ids) => {
                        inactive_observed = !ids.contains(&id);
                        result["active_during_disable"] = json!(ids);
                    }
                    Err(error) => disable_observation_error = Some(error),
                }
                if inactive_observed || started.elapsed() >= Duration::from_secs(1) {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        thread::sleep(Duration::from_millis(300));
        // Always attempt one enable transaction, including when disable failed.
        let enable = functions.set_enabled(id, true);
        let started = Instant::now();
        let mut last_error = None;
        let target_active = loop {
            match active() {
                Ok(ids) if ids.contains(&id) => {
                    result["active_after"] = json!(ids);
                    break true;
                }
                Ok(ids) => result["active_after"] = json!(ids),
                Err(error) => last_error = Some(error),
            }
            if started.elapsed() >= Duration::from_secs(2) {
                break false;
            }
            thread::sleep(Duration::from_millis(100));
        };
        result["disable"] = json!({"ok":disable.is_ok(),"error":disable.as_ref().err()});
        result["inactive_observed"] = json!(inactive_observed);
        result["disable_observation_error"] = json!(disable_observation_error);
        result["enable"] = json!({"ok":enable.is_ok(),"error":enable.as_ref().err()});
        result["target_active_after"] = json!(target_active);
        result["verification_error"] = json!(last_error);
        result["verified"] =
            json!(disable.is_ok() && inactive_observed && enable.is_ok() && target_active);
        result["ok"] = result["verified"].clone();
        Ok(result)
    }
}
