//! Local macOS display modes. Selection is read-only; only `apply` changes a mode.
use serde::Serialize;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionRole {
    Fullscreen,
    SplitHalf,
    PipMain,
    PipInset,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mode {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: f64,
}

impl Mode {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.pixel_width > 0
            && self.pixel_height > 0
            && self.refresh_hz.is_finite()
            && self.refresh_hz >= 0.0
    }

    fn density_distance(&self, current: &Mode) -> f64 {
        (self.pixel_width as f64 / self.width as f64
            - current.pixel_width as f64 / current.width as f64)
            .abs()
            + (self.pixel_height as f64 / self.height as f64
                - current.pixel_height as f64 / current.height as f64)
                .abs()
    }
}

/// Select an advertised physical pixel size, preserving desktop density and
/// refresh rate where possible. This never synthesizes a timing.
pub fn choose(role: ResolutionRole, available: &[Mode], current: &Mode) -> Result<Mode, String> {
    if !current.valid() {
        return Err("Current display mode has invalid dimensions or refresh rate".into());
    }
    let target =
        match role {
            ResolutionRole::Fullscreen => (5120, 1440),
            ResolutionRole::PipMain => {
                if available.iter().any(|mode| {
                    mode.valid() && (mode.pixel_width, mode.pixel_height) == (5120, 1440)
                }) {
                    (5120, 1440)
                } else {
                    // The observed PiP-main EDID also advertises this exact 32:9 size.
                    (3840, 1080)
                }
            }
            ResolutionRole::SplitHalf => (2560, 1440),
            ResolutionRole::PipInset => {
                if available.iter().any(|mode| {
                    mode.valid() && (mode.pixel_width, mode.pixel_height) == (1920, 1080)
                }) {
                    (1920, 1080)
                } else {
                    // Observed 0x7461 inset profile advertises a 1280x720 ceiling.
                    (1280, 720)
                }
            }
        };
    available
        .iter()
        .filter(|m| m.valid() && (m.pixel_width, m.pixel_height) == target)
        .min_by(|a, b| compare_preference(a, b, current))
        .cloned()
        .ok_or_else(|| {
            format!(
                "No advertised mode with physical pixels {}x{}",
                target.0, target.1
            )
        })
}

fn compare_preference(a: &Mode, b: &Mode, current: &Mode) -> Ordering {
    a.density_distance(current)
        .total_cmp(&b.density_distance(current))
        .then_with(|| {
            (a.refresh_hz - current.refresh_hz)
                .abs()
                .total_cmp(&(b.refresh_hz - current.refresh_hz).abs())
        })
        .then_with(|| (b.id == current.id).cmp(&(a.id == current.id)))
        .then_with(|| b.refresh_hz.total_cmp(&a.refresh_hz))
        .then_with(|| a.id.cmp(&b.id))
}

pub fn enumerate(cg_id: u32) -> Result<Vec<Mode>, String> {
    #[cfg(target_os = "macos")]
    {
        mac::enumerate(cg_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cg_id;
        Err("Native display modes require macOS".into())
    }
}

pub fn current(cg_id: u32) -> Result<Mode, String> {
    #[cfg(target_os = "macos")]
    {
        mac::current(cg_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cg_id;
        Err("Native display modes require macOS".into())
    }
}

/// Apply only to this Mac. Mode IDs are resolved again immediately before use.
/// An error after the set call may mean the display changed but verification failed.
pub fn apply(cg_id: u32, mode_id: u32) -> Result<Mode, String> {
    #[cfg(target_os = "macos")]
    {
        mac::apply(cg_id, mode_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cg_id, mode_id);
        Err("Native display modes require macOS".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Mode;
    use std::ffi::c_void;
    type Cf = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGDisplayCopyAllDisplayModes(display: u32, options: Cf) -> Cf;
        fn CGDisplayCopyDisplayMode(display: u32) -> Cf;
        fn CGDisplayModeGetWidth(mode: Cf) -> usize;
        fn CGDisplayModeGetHeight(mode: Cf) -> usize;
        fn CGDisplayModeGetPixelWidth(mode: Cf) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: Cf) -> usize;
        fn CGDisplayModeGetRefreshRate(mode: Cf) -> f64;
        fn CGDisplayModeGetIODisplayModeID(mode: Cf) -> i32;
        fn CGBeginDisplayConfiguration(config: *mut *mut c_void) -> i32;
        fn CGConfigureDisplayWithDisplayMode(
            config: *mut c_void,
            display: u32,
            mode: Cf,
            options: Cf,
        ) -> i32;
        fn CGCompleteDisplayConfiguration(config: *mut c_void, scope: u32) -> i32;
        fn CGCancelDisplayConfiguration(config: *mut c_void) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: Cf) -> isize;
        fn CFArrayGetValueAtIndex(array: Cf, index: isize) -> Cf;
        fn CFRelease(value: Cf);
    }

    struct Owned(Cf);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }

    // The caller keeps the owning array or copied mode alive while reading.
    fn read(raw: Cf) -> Result<Mode, String> {
        if raw.is_null() {
            return Err("CoreGraphics returned a null display mode".into());
        }
        let dimension =
            |n| u32::try_from(n).map_err(|_| "Display mode dimension exceeds u32".to_string());
        let mode = unsafe {
            Mode {
                id: CGDisplayModeGetIODisplayModeID(raw) as u32,
                width: dimension(CGDisplayModeGetWidth(raw))?,
                height: dimension(CGDisplayModeGetHeight(raw))?,
                pixel_width: dimension(CGDisplayModeGetPixelWidth(raw))?,
                pixel_height: dimension(CGDisplayModeGetPixelHeight(raw))?,
                refresh_hz: CGDisplayModeGetRefreshRate(raw),
            }
        };
        if !mode.valid() {
            return Err("CoreGraphics returned an invalid display mode".into());
        }
        Ok(mode)
    }

    fn modes(cg_id: u32) -> Result<Owned, String> {
        let array = Owned(unsafe { CGDisplayCopyAllDisplayModes(cg_id, std::ptr::null()) });
        if array.0.is_null() {
            return Err(format!(
                "Cannot enumerate display modes for display {cg_id}"
            ));
        }
        Ok(array)
    }

    pub fn enumerate(cg_id: u32) -> Result<Vec<Mode>, String> {
        let array = modes(cg_id)?;
        let count = unsafe { CFArrayGetCount(array.0) };
        if !(0..=65536).contains(&count) {
            return Err("Invalid CoreGraphics mode count".into());
        }
        let mut result = Vec::with_capacity(count as usize);
        for index in 0..count {
            result.push(read(unsafe { CFArrayGetValueAtIndex(array.0, index) })?);
        }
        result.sort_by_key(|m| m.id);
        Ok(result)
    }

    pub fn current(cg_id: u32) -> Result<Mode, String> {
        let raw = Owned(unsafe { CGDisplayCopyDisplayMode(cg_id) });
        read(raw.0)
    }

    pub fn apply(cg_id: u32, mode_id: u32) -> Result<Mode, String> {
        let array = modes(cg_id)?;
        let count = unsafe { CFArrayGetCount(array.0) };
        if !(0..=65536).contains(&count) {
            return Err("Invalid CoreGraphics mode count".into());
        }
        let mut found = None;
        for index in 0..count {
            let raw = unsafe { CFArrayGetValueAtIndex(array.0, index) };
            let mode = read(raw)?;
            if mode.id == mode_id {
                if let Some((_, previous)) = &found {
                    if previous != &mode {
                        return Err(format!("Mode ID {mode_id} is ambiguous; refresh the plan"));
                    }
                } else {
                    found = Some((raw, mode));
                }
            }
        }
        let (raw, requested) = found.ok_or_else(|| {
            format!("Mode ID {mode_id} is no longer advertised; refresh the plan")
        })?;
        // `raw` is borrowed from `array`, which remains alive through the set call.
        // The convenience setter is app-scoped and reverts as a CLI exits.
        // Commit for this login session; do not overwrite permanent user defaults.
        let mut config = std::ptr::null_mut();
        let status = unsafe { CGBeginDisplayConfiguration(&mut config) };
        if status != 0 || config.is_null() {
            return Err(format!("Could not begin a display configuration: {status}"));
        }
        let status =
            unsafe { CGConfigureDisplayWithDisplayMode(config, cg_id, raw, std::ptr::null()) };
        if status != 0 {
            unsafe {
                CGCancelDisplayConfiguration(config);
            }
            return Err(format!(
                "Could not configure the selected display mode: {status}"
            ));
        }
        // Complete consumes the configuration even on error, per the SDK contract.
        let status = unsafe { CGCompleteDisplayConfiguration(config, 1) };
        if status != 0 {
            return Err(format!(
                "Could not commit the display mode for this session: {status}"
            ));
        }
        let actual = current(cg_id)
            .map_err(|e| format!("Mode set succeeded, but verification failed: {e}"))?;
        if actual != requested {
            return Err(format!("Mode set returned success, but requested mode {mode_id} was not verified (observed {})", actual.id));
        }
        Ok(actual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mode(id: u32, pixels: (u32, u32), scale: u32, hz: f64) -> Mode {
        Mode {
            id,
            width: pixels.0 / scale,
            height: pixels.1 / scale,
            pixel_width: pixels.0,
            pixel_height: pixels.1,
            refresh_hz: hz,
        }
    }
    #[test]
    fn preserves_density_then_refresh() {
        let current = mode(10, (5120, 1440), 2, 120.0);
        let choices = vec![
            mode(1, (2560, 1440), 1, 120.0),
            mode(2, (2560, 1440), 2, 60.0),
            mode(3, (2560, 1440), 2, 120.0),
        ];
        assert_eq!(
            choose(ResolutionRole::SplitHalf, &choices, &current)
                .unwrap()
                .id,
            3
        );
    }
    #[test]
    fn deterministic_ties_preserve_current_id() {
        let current = mode(9, (5120, 1440), 1, 60.0);
        let choices = vec![
            mode(9, (5120, 1440), 1, 60.0),
            mode(1, (5120, 1440), 1, 60.0),
        ];
        assert_eq!(
            choose(ResolutionRole::Fullscreen, &choices, &current)
                .unwrap()
                .id,
            9
        );
        assert_eq!(
            choose(
                ResolutionRole::Fullscreen,
                &choices,
                &mode(8, (5120, 1440), 1, 60.0)
            )
            .unwrap()
            .id,
            1
        );
    }
    #[test]
    fn refuses_unsupported_physical_size() {
        let current = mode(1, (3840, 1080), 1, 60.0);
        assert!(choose(
            ResolutionRole::Fullscreen,
            std::slice::from_ref(&current),
            &current
        )
        .is_err());
    }
    #[test]
    fn pip_main_accepts_advertised_fallback_and_preserves_density_and_refresh() {
        let current = mode(1, (3840, 1080), 2, 120.0);
        let choices = vec![
            mode(2, (3840, 1080), 1, 120.0),
            mode(3, (3840, 1080), 2, 60.0),
            mode(4, (3840, 1080), 2, 120.0),
        ];
        assert_eq!(
            choose(ResolutionRole::PipMain, &choices, &current)
                .unwrap()
                .id,
            4
        );
        assert!(choose(ResolutionRole::Fullscreen, &choices, &current).is_err());
    }
    #[test]
    fn pip_main_prefers_advertised_5120_over_faster_fallback() {
        let current = mode(1, (3840, 1080), 1, 120.0);
        let choices = vec![current.clone(), mode(2, (5120, 1440), 1, 60.0)];
        assert_eq!(
            choose(ResolutionRole::PipMain, &choices, &current)
                .unwrap()
                .id,
            2
        );
    }
    #[test]
    fn pip_main_refuses_unadvertised_sizes_and_wrong_aspect() {
        let current = mode(1, (3840, 1080), 1, 120.0);
        for choices in [
            vec![],
            vec![mode(2, (3840, 2160), 1, 120.0)],
            vec![mode(3, (6400, 1800), 1, 120.0)],
            vec![mode(4, (2560, 1440), 1, 120.0)],
        ] {
            assert!(choose(ResolutionRole::PipMain, &choices, &current).is_err());
        }
    }
    #[test]
    fn inset_requires_the_advertised_user_calibrated_size() {
        let current = mode(1, (5120, 1440), 1, 60.0);
        let choices = vec![
            mode(2, (1440, 900), 1, 60.0),
            mode(3, (2560, 1440), 1, 60.0),
            mode(4, (3840, 2160), 1, 60.0),
            mode(5, (1920, 1080), 1, 60.0),
        ];
        assert_eq!(
            choose(ResolutionRole::PipInset, &choices, &current)
                .unwrap()
                .id,
            5
        );
        // EDID preference is not an input: only the calibrated advertised size qualifies.
        assert!(choose(ResolutionRole::PipInset, &choices[..3], &current).is_err());
        assert!(choose(ResolutionRole::PipInset, &[], &current).is_err());
    }
    #[test]
    fn inset_preserves_density_and_refresh_within_calibrated_size() {
        let current = mode(1, (5120, 1440), 2, 120.0);
        let choices = vec![
            mode(2, (1920, 1080), 1, 120.0),
            mode(3, (1920, 1080), 2, 60.0),
            mode(4, (1920, 1080), 2, 120.0),
        ];
        assert_eq!(
            choose(ResolutionRole::PipInset, &choices, &current)
                .unwrap()
                .id,
            4
        );
    }
    #[test]
    fn observed_inset_ceiling_is_used_only_when_1080p_is_not_advertised() {
        let current = mode(1, (1280, 720), 1, 60.0);
        assert_eq!(
            choose(
                ResolutionRole::PipInset,
                std::slice::from_ref(&current),
                &current
            )
            .unwrap()
            .id,
            1
        );
        let preferred = mode(2, (1920, 1080), 1, 60.0);
        assert_eq!(
            choose(
                ResolutionRole::PipInset,
                &[current.clone(), preferred],
                &current
            )
            .unwrap()
            .id,
            2
        );
    }
    #[test]
    fn ignores_invalid_modes() {
        let current = mode(1, (5120, 1440), 1, 60.0);
        let mut invalid = mode(2, (2560, 1440), 1, f64::NAN);
        assert!(choose(ResolutionRole::SplitHalf, &[invalid.clone()], &current).is_err());
        invalid.refresh_hz = 60.0;
        invalid.width = 0;
        assert!(choose(ResolutionRole::SplitHalf, &[invalid], &current).is_err());
    }
}
