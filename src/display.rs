//! Enumerates external AV proxies; selection always uses the exact IORegistry ID.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Display {
    pub registry_id: u64,
    pub name: String,
    pub manufacturer: Option<String>,
    pub product_id: Option<u32>,
    pub serial: Option<u32>,
    pub location: String,
    pub edid_hex: Option<String>,
    #[serde(default)]
    pub active_width: Option<u32>,
    #[serde(default)]
    pub active_height: Option<u32>,
    #[serde(default)]
    pub cg_display_id: Option<u32>,
    /// Local IORegistry connection type, independent of the monitor's main input.
    #[serde(default)]
    pub connection: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
struct CgIdentity {
    id: u32,
    vendor: u32,
    product: u32,
    serial: u32,
    builtin: bool,
    width: u32,
    height: u32,
}

/// Registry metadata may disappear while the built-in screen reconnects. Only a
/// one-to-one external topology permits using CG identity without registry pairing.
#[cfg(any(target_os = "macos", test))]
fn fill_single_external_identity(displays: &mut [Display], online: &[CgIdentity]) {
    let mut proxies = displays
        .iter_mut()
        .filter(|display| display.location == "External");
    let Some(display) = proxies.next() else {
        return;
    };
    if proxies.next().is_some() {
        return;
    }
    let mut externals = online.iter().filter(|display| !display.builtin);
    let Some(cg) = externals.next() else {
        return;
    };
    if externals.next().is_some()
        || cg.vendor != 0x4c2d
        || !matches!(cg.product, 0x749b | 0x745e | 0x7462 | 0x7461)
        || cg.serial == 0
        || cg.id == 0
        || cg.width == 0
        || cg.height == 0
    {
        return;
    }
    if display
        .manufacturer
        .as_deref()
        .is_some_and(|value| value != "SAM")
        || display.product_id.is_some_and(|value| value != cg.product)
        || display.serial.is_some_and(|value| value != cg.serial)
        || display.cg_display_id.is_some_and(|value| value != cg.id)
        || (display.name != "External display"
            && !display.name.to_ascii_uppercase().contains("G93SC"))
    {
        return;
    }
    display.manufacturer = Some("SAM".into());
    display.product_id = Some(cg.product);
    display.serial = Some(cg.serial);
    if display.name == "External display" {
        display.name = "Odyssey G93SC".into();
    }
    display.cg_display_id = Some(cg.id);
    display.active_width = Some(cg.width);
    display.active_height = Some(cg.height);
    // CG identity does not establish a cable/port. Keep any absent hint absent.
}

#[cfg(any(target_os = "macos", test))]
fn normalize_connection(description: &str) -> Option<&'static str> {
    match description.trim().to_ascii_lowercase().as_str() {
        "hdmi" => Some("HDMI"),
        "displayport" | "display port" | "dp" => Some("DisplayPort"),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn connection_hint(descriptions: &[String]) -> Option<String> {
    let mut hint = None;
    for description in descriptions {
        let Some(value) = normalize_connection(description) else {
            continue;
        };
        if hint.is_some_and(|previous| previous != value) {
            return None;
        }
        hint = Some(value);
    }
    hint.map(str::to_owned)
}

pub fn enumerate() -> Result<Vec<Display>, String> {
    #[cfg(target_os = "macos")]
    {
        mac::enumerate()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Native discovery requires macOS on Apple Silicon".into())
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod mac {
    use super::Display;
    use std::ffi::{c_char, c_void, CString};
    pub type Cf = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayVendorNumber(display: u32) -> u32;
        fn CGDisplayModelNumber(display: u32) -> u32;
        fn CGDisplaySerialNumber(display: u32) -> u32;
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayPixelsHigh(display: u32) -> usize;
        fn CGDisplayIsBuiltin(display: u32) -> u32;
    }
    fn pnp_vendor(manufacturer: &str) -> Option<u32> {
        let bytes = manufacturer.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(u8::is_ascii_uppercase) {
            return None;
        }
        Some(
            (u32::from(bytes[0] - b'A' + 1) << 10)
                | (u32::from(bytes[1] - b'A' + 1) << 5)
                | u32::from(bytes[2] - b'A' + 1),
        )
    }
    fn identity_key(display: &Display) -> Option<(u32, u32, u32)> {
        Some((
            pnp_vendor(display.manufacturer.as_deref()?)?,
            display.product_id?,
            display.serial?,
        ))
    }
    fn fill_active_dimensions(displays: &mut [Display]) {
        let mut count = 0;
        if unsafe { CGGetOnlineDisplayList(0, std::ptr::null_mut(), &mut count) } != 0
            || count == 0
            || count > 128
        {
            return;
        }
        let mut ids = vec![0u32; count as usize];
        let capacity = count;
        if unsafe { CGGetOnlineDisplayList(capacity, ids.as_mut_ptr(), &mut count) } != 0
            || count > capacity
        {
            return;
        }
        ids.truncate(count as usize);
        let online: Vec<_> = ids
            .iter()
            .map(|&id| unsafe {
                super::CgIdentity {
                    id,
                    vendor: CGDisplayVendorNumber(id),
                    product: CGDisplayModelNumber(id),
                    serial: CGDisplaySerialNumber(id),
                    builtin: CGDisplayIsBuiltin(id) != 0,
                    width: u32::try_from(CGDisplayPixelsWide(id)).unwrap_or(0),
                    height: u32::try_from(CGDisplayPixelsHigh(id)).unwrap_or(0),
                }
            })
            .collect();
        let keys: Vec<_> = displays.iter().map(identity_key).collect();
        for (index, display) in displays.iter_mut().enumerate() {
            let Some(key) = keys[index] else {
                continue;
            };
            if keys
                .iter()
                .filter(|candidate| **candidate == Some(key))
                .count()
                != 1
            {
                continue;
            }
            let matches: Vec<_> = online
                .iter()
                .filter(|cg| (cg.vendor, cg.product, cg.serial) == key)
                .collect();
            if matches.len() != 1 {
                continue;
            }
            let cg = matches[0];
            if cg.width == 0 || cg.height == 0 {
                continue;
            }
            display.cg_display_id = Some(cg.id);
            display.active_width = Some(cg.width);
            display.active_height = Some(cg.height);
        }
        super::fill_single_external_identity(displays, &online);
    }
    pub struct Object(pub u32);
    impl Drop for Object {
        fn drop(&mut self) {
            unsafe {
                IOObjectRelease(self.0);
            }
        }
    }
    pub struct OwnedCf(pub Cf);
    impl Drop for OwnedCf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CFRelease(self.0);
                }
            }
        }
    }
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> Cf;
        fn IOServiceGetMatchingServices(port: u32, matching: Cf, iterator: *mut u32) -> i32;
        fn IOIteratorNext(iterator: u32) -> u32;
        fn IOIteratorIsValid(iterator: u32) -> u32;
        fn IOObjectConformsTo(object: u32, name: *const c_char) -> u32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
        fn IORegistryEntryGetParentEntry(entry: u32, plane: *const c_char, parent: *mut u32)
            -> i32;
        fn IORegistryEntryCreateIterator(
            entry: u32,
            plane: *const c_char,
            options: u32,
            iterator: *mut u32,
        ) -> i32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: Cf, allocator: Cf, options: u32) -> Cf;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(value: Cf);
        fn CFGetTypeID(value: Cf) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFStringCreateWithCString(allocator: Cf, string: *const c_char, encoding: u32) -> Cf;
        fn CFStringGetCString(value: Cf, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFDictionaryGetValue(value: Cf, key: Cf) -> Cf;
        fn CFNumberGetTypeID() -> usize;
        fn CFNumberGetValue(value: Cf, number_type: i32, result: *mut c_void) -> u8;
        fn CFDataGetTypeID() -> usize;
        fn CFDataGetLength(value: Cf) -> isize;
        fn CFDataGetBytePtr(value: Cf) -> *const u8;
    }
    fn key(value: &str) -> OwnedCf {
        let value = CString::new(value).expect("static property key");
        OwnedCf(unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x08000100) })
    }
    fn property(entry: u32, name: &str) -> Option<OwnedCf> {
        let value =
            unsafe { IORegistryEntryCreateCFProperty(entry, key(name).0, std::ptr::null(), 0) };
        (!value.is_null()).then(|| OwnedCf(value))
    }
    fn field(dict: Cf, name: &str) -> Cf {
        if dict.is_null() || unsafe { CFGetTypeID(dict) != CFDictionaryGetTypeID() } {
            return std::ptr::null();
        }
        unsafe { CFDictionaryGetValue(dict, key(name).0) }
    }
    fn string(value: Cf) -> Option<String> {
        if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
            return None;
        }
        let mut bytes = vec![0u8; 4096];
        if unsafe {
            CFStringGetCString(
                value,
                bytes.as_mut_ptr().cast(),
                bytes.len() as isize,
                0x08000100,
            )
        } == 0
        {
            return None;
        }
        let end = bytes.iter().position(|&b| b == 0)?;
        String::from_utf8(bytes[..end].to_vec()).ok()
    }
    fn number(value: Cf) -> Option<u32> {
        if value.is_null() || unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
            return None;
        }
        let mut n: i64 = 0;
        if unsafe { CFNumberGetValue(value, 4, (&mut n as *mut i64).cast()) } == 0 {
            return None;
        }
        u32::try_from(n).ok()
    }
    fn data(value: Cf) -> Option<String> {
        if value.is_null() || unsafe { CFGetTypeID(value) != CFDataGetTypeID() } {
            return None;
        }
        let length = unsafe { CFDataGetLength(value) };
        if !(128..=32768).contains(&length) {
            return None;
        }
        Some(crate::transport::hex(unsafe {
            std::slice::from_raw_parts(CFDataGetBytePtr(value), length as usize)
        }))
    }
    fn edids(entry: u32) -> Vec<String> {
        let mut values = Vec::new();
        for name in ["EDID", "IODisplayEDID"] {
            if let Some(value) = property(entry, name).and_then(|p| data(p.0)) {
                values.push(value);
            }
        }
        for name in ["Metadata", "DisplayAttributes"] {
            if let Some(dict) = property(entry, name) {
                for key in ["EDID", "IODisplayEDID"] {
                    if let Some(value) = data(field(dict.0, key)) {
                        values.push(value);
                    }
                }
            }
        }
        values.sort();
        values.dedup();
        values
    }
    fn connection_descriptions(entry: u32) -> Vec<String> {
        property(entry, "Metadata")
            .and_then(|metadata| string(field(metadata.0, "DFP Type Description")))
            .into_iter()
            .collect()
    }
    // Metadata can live beside the CLCD node. Accept it only inside a subtree
    // containing one AV proxy and one distinct EDID, independent of node order.
    // Connection hints come from explicit DFP descriptions, never class names or VCP 60.
    fn subtree_metadata(entry: u32) -> (Option<String>, Option<String>) {
        let mut iterator = 0;
        if unsafe { IORegistryEntryCreateIterator(entry, c"IOService".as_ptr(), 1, &mut iterator) }
            != 0
        {
            return (None, None);
        }
        let iterator = Object(iterator);
        let mut proxies =
            u32::from(unsafe { IOObjectConformsTo(entry, c"DCPAVServiceProxy".as_ptr()) } != 0);
        let mut values = edids(entry);
        let mut descriptions = connection_descriptions(entry);
        for _ in 0..4096 {
            let node = unsafe { IOIteratorNext(iterator.0) };
            if node == 0 {
                if unsafe { IOIteratorIsValid(iterator.0) } == 0 {
                    return (None, None);
                }
                return if proxies == 1 && values.len() == 1 {
                    (values.pop(), super::connection_hint(&descriptions))
                } else {
                    (None, None)
                };
            }
            let node = Object(node);
            if unsafe { IOObjectConformsTo(node.0, c"DCPAVServiceProxy".as_ptr()) } != 0 {
                proxies += 1;
            }
            values.extend(edids(node.0));
            descriptions.extend(connection_descriptions(node.0));
            values.sort();
            values.dedup();
            if proxies > 1 || values.len() > 1 {
                return (None, None);
            }
        }
        (None, None)
    }
    pub fn proxies() -> Result<Vec<Object>, String> {
        let mut iterator = 0;
        let status = unsafe {
            IOServiceGetMatchingServices(
                0,
                IOServiceMatching(c"DCPAVServiceProxy".as_ptr()),
                &mut iterator,
            )
        };
        if status != 0 {
            return Err(format!(
                "IOServiceGetMatchingServices failed: {status:#010x}"
            ));
        }
        let iterator = Object(iterator);
        let mut result = Vec::new();
        loop {
            let entry = unsafe { IOIteratorNext(iterator.0) };
            if entry == 0 {
                break;
            }
            let entry = Object(entry);
            if property(entry.0, "Location")
                .and_then(|p| string(p.0))
                .as_deref()
                == Some("External")
            {
                result.push(entry);
            }
        }
        if unsafe { IOIteratorIsValid(iterator.0) } == 0 {
            return Err(
                "IORegistry changed during discovery; retry before selecting a display".into(),
            );
        }
        Ok(result)
    }
    pub fn registry_id(entry: u32) -> Result<u64, String> {
        let mut id = 0;
        let status = unsafe { IORegistryEntryGetRegistryEntryID(entry, &mut id) };
        if status != 0 {
            Err(format!("Cannot read registry ID: {status:#010x}"))
        } else {
            Ok(id)
        }
    }
    fn identity(entry: u32, display: &mut Display) -> bool {
        let Some(attrs) = property(entry, "DisplayAttributes") else {
            return false;
        };
        let product = field(attrs.0, "ProductAttributes");
        if product.is_null() {
            return false;
        }
        display.manufacturer = string(field(product, "ManufacturerID"));
        display.product_id = number(field(product, "ProductID"));
        display.serial = number(field(product, "SerialNumber"));
        if let Some(name) = string(field(product, "ProductName")) {
            display.name = name;
        }
        display.edid_hex = data(field(attrs.0, "EDID"))
            .or_else(|| property(entry, "IODisplayEDID").and_then(|p| data(p.0)))
            .or_else(|| property(entry, "EDID").and_then(|p| data(p.0)))
            .or_else(|| property(entry, "Metadata").and_then(|p| data(field(p.0, "EDID"))));
        true
    }
    // Associate a sibling CLCD only when this subtree contains exactly one AV proxy
    // and exactly one identity node. Never pair displays by traversal order.
    fn subtree_identity(entry: u32, display: &mut Display) -> bool {
        let mut iterator = 0;
        if unsafe { IORegistryEntryCreateIterator(entry, c"IOService".as_ptr(), 1, &mut iterator) }
            != 0
        {
            return false;
        }
        let iterator = Object(iterator);
        let mut proxies = 0;
        let mut identities: Vec<Object> = Vec::new();
        for _ in 0..4096 {
            let node = unsafe { IOIteratorNext(iterator.0) };
            if node == 0 {
                if unsafe { IOIteratorIsValid(iterator.0) } == 0 {
                    return false;
                }
                return proxies == 1 && identities.len() == 1 && identity(identities[0].0, display);
            }
            let node = Object(node);
            if unsafe { IOObjectConformsTo(node.0, c"DCPAVServiceProxy".as_ptr()) } != 0 {
                proxies += 1;
            }
            if let Some(attrs) = property(node.0, "DisplayAttributes") {
                if !field(attrs.0, "ProductAttributes").is_null() {
                    identities.push(node);
                }
            }
            if proxies > 1 || identities.len() > 1 {
                return false;
            }
        }
        false
    }
    pub fn enumerate() -> Result<Vec<Display>, String> {
        let mut displays = Vec::new();
        for proxy in proxies()? {
            let mut display = Display {
                registry_id: registry_id(proxy.0)?,
                name: "External display".into(),
                manufacturer: None,
                product_id: None,
                serial: None,
                location: "External".into(),
                edid_hex: None,
                active_width: None,
                active_height: None,
                cg_display_id: None,
                connection: None,
            };
            let mut parent: Option<Object> = None;
            let mut identity_found = false;
            for _ in 0..32 {
                let entry = parent.as_ref().map_or(proxy.0, |p| p.0);
                if !identity_found {
                    identity_found =
                        identity(entry, &mut display) || subtree_identity(entry, &mut display);
                }
                if display.edid_hex.is_none() || display.connection.is_none() {
                    let (edid, connection) = subtree_metadata(entry);
                    if display.edid_hex.is_none() {
                        display.edid_hex = edid;
                    }
                    if display.connection.is_none() {
                        display.connection = connection;
                    }
                }
                if identity_found && display.edid_hex.is_some() && display.connection.is_some() {
                    break;
                }
                let mut next = 0;
                if unsafe { IORegistryEntryGetParentEntry(entry, c"IOService".as_ptr(), &mut next) }
                    != 0
                {
                    break;
                }
                parent = Some(Object(next));
            }
            displays.push(display);
        }
        displays.sort_by_key(|d| d.registry_id);
        fill_active_dimensions(&mut displays);
        Ok(displays)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        connection_hint, fill_single_external_identity, normalize_connection, CgIdentity, Display,
    };
    fn anonymous_proxy(id: u64) -> Display {
        Display {
            registry_id: id,
            name: "External display".into(),
            manufacturer: None,
            product_id: None,
            serial: None,
            location: "External".into(),
            edid_hex: None,
            active_width: None,
            active_height: None,
            cg_display_id: None,
            connection: None,
        }
    }
    fn cg_external() -> CgIdentity {
        CgIdentity {
            id: 2,
            vendor: 0x4c2d,
            product: 0x749b,
            serial: 42,
            builtin: false,
            width: 2560,
            height: 1440,
        }
    }
    #[test]
    fn cg_identity_fallback_requires_unique_external_topology_and_preserves_unknown_port() {
        let builtin = CgIdentity {
            id: 1,
            vendor: 0x610,
            product: 0,
            serial: 0,
            builtin: true,
            width: 1512,
            height: 982,
        };
        let mut proxies = vec![anonymous_proxy(9)];
        fill_single_external_identity(&mut proxies, &[builtin, cg_external()]);
        assert_eq!(proxies[0].manufacturer.as_deref(), Some("SAM"));
        assert_eq!(proxies[0].serial, Some(42));
        assert_eq!(proxies[0].cg_display_id, Some(2));
        assert_eq!(proxies[0].active_width, Some(2560));
        assert!(proxies[0].connection.is_none());
        let mut proxies = vec![anonymous_proxy(9), anonymous_proxy(10)];
        fill_single_external_identity(&mut proxies, &[cg_external()]);
        assert!(proxies.iter().all(|display| display.manufacturer.is_none()));
        let mut proxies = vec![anonymous_proxy(9)];
        let other = CgIdentity {
            id: 3,
            ..cg_external()
        };
        fill_single_external_identity(&mut proxies, &[cg_external(), other]);
        assert!(proxies[0].manufacturer.is_none());
    }
    #[test]
    fn cg_identity_fallback_rejects_unverified_or_conflicting_identity() {
        for cg in [
            CgIdentity {
                vendor: 0x610,
                ..cg_external()
            },
            CgIdentity {
                product: 0x1234,
                ..cg_external()
            },
            CgIdentity {
                serial: 0,
                ..cg_external()
            },
        ] {
            let mut proxies = vec![anonymous_proxy(9)];
            fill_single_external_identity(&mut proxies, &[cg]);
            assert!(proxies[0].manufacturer.is_none());
        }
        let mut proxies = vec![anonymous_proxy(9)];
        proxies[0].serial = Some(123);
        fill_single_external_identity(&mut proxies, &[cg_external()]);
        assert_eq!(proxies[0].serial, Some(123));
        assert!(proxies[0].manufacturer.is_none());
    }

    #[test]
    fn observed_pip_inset_product_preserves_the_same_monitor_identity() {
        let mut proxies = vec![anonymous_proxy(4295626181)];
        let inset = CgIdentity {
            product: 0x7461,
            width: 1280,
            height: 720,
            ..cg_external()
        };
        fill_single_external_identity(&mut proxies, &[inset]);
        assert_eq!(proxies[0].product_id, Some(0x7461));
        assert_eq!(proxies[0].serial, Some(42));
        assert_eq!(proxies[0].cg_display_id, Some(2));
        assert!(crate::app::supported_display(&proxies[0]));
    }

    #[test]
    fn normalizes_explicit_connection_descriptions() {
        assert_eq!(normalize_connection("HDMI"), Some("HDMI"));
        assert_eq!(normalize_connection(" hdmi "), Some("HDMI"));
        assert_eq!(normalize_connection("DisplayPort"), Some("DisplayPort"));
        assert_eq!(normalize_connection("Display Port"), Some("DisplayPort"));
        assert_eq!(normalize_connection("DP"), Some("DisplayPort"));
        for description in ["", "cHDMIb", "DCPAVServiceProxy", "USB-C", "HDMI adapter"] {
            assert_eq!(normalize_connection(description), None);
        }
    }

    #[test]
    fn conflicting_connection_hints_stay_unknown() {
        let hints = |values: &[&str]| {
            connection_hint(&values.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(hints(&["HDMI", "hdmi"]), Some("HDMI".into()));
        assert_eq!(hints(&["DisplayPort", "DP"]), Some("DisplayPort".into()));
        assert_eq!(hints(&["HDMI", "DisplayPort"]), None);
        assert_eq!(hints(&["DisplayPort", "HDMI", "DisplayPort"]), None);
        assert_eq!(hints(&["unknown"]), None);
        assert_eq!(hints(&[]), None);
    }
}
