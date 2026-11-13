// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use core_graphics::display::CGDisplayBounds;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{NSArray, NSPoint, NSSize, NSString};

const MAX_DISPLAYS: usize = 8;
const MIN_EDGE: u32 = 640;
const MAX_EDGE: u32 = 7680;
const PIXELS_PER_INCH: f64 = 110.0;
const REFRESH_HZ: f64 = 60.0;
const VENDOR_ID: u32 = 0x0505;
const PRODUCT_ID: u32 = 0x5678;
const GLOBAL_QUEUE_PRIORITY_HIGH: isize = 2;

extern "C" {
    fn CGGetOnlineDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

struct Handle(Retained<AnyObject>);

unsafe impl Send for Handle {}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    pub display_id: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

static DISPLAYS: Mutex<Option<HashMap<u32, Handle>>> = Mutex::new(None);

pub fn validate_size(width: u32, height: u32) -> Result<(), String> {
    for (label, edge) in [("width", width), ("height", height)] {
        if !(MIN_EDGE..=MAX_EDGE).contains(&edge) {
            return Err(format!(
                "{label} must be between {MIN_EDGE} and {MAX_EDGE} pixels, got {edge}"
            ));
        }
    }
    Ok(())
}

pub fn bounds_of(display_id: u32) -> DisplayInfo {
    let bounds = unsafe { CGDisplayBounds(display_id) };
    DisplayInfo {
        display_id,
        x: bounds.origin.x,
        y: bounds.origin.y,
        width: bounds.size.width,
        height: bounds.size.height,
    }
}

pub fn list() -> Vec<DisplayInfo> {
    let guard = DISPLAYS.lock().unwrap_or_else(|e| e.into_inner());
    let mut ids: Vec<u32> = guard
        .as_ref()
        .map(|map| map.keys().copied().collect())
        .unwrap_or_default();
    ids.sort_unstable();
    let mut online = [0u32; 32];
    let mut count = 0u32;
    let status = unsafe { CGGetOnlineDisplayList(32, online.as_mut_ptr(), &mut count) };
    if status != 0 {
        return Vec::new();
    }
    let online = &online[..count as usize];
    ids.into_iter()
        .filter(|id| online.contains(id))
        .map(bounds_of)
        .collect()
}

pub fn create(width: u32, height: u32, name: &str) -> Result<DisplayInfo, String> {
    validate_size(width, height)?;
    let mut guard = DISPLAYS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_DISPLAYS {
        return Err(format!("at most {MAX_DISPLAYS} virtual displays"));
    }
    let serial = 0x6000 + map.len() as u32 + 1;
    let handle = unsafe { build(width, height, name, serial)? };
    let display_id: u32 = unsafe { msg_send![&*handle.0, displayID] };
    if display_id == 0 {
        return Err("CGVirtualDisplay returned display id 0".into());
    }
    map.insert(display_id, handle);
    Ok(bounds_of(display_id))
}

pub fn remove(display_id: u32) -> Result<(), String> {
    let mut guard = DISPLAYS.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut().and_then(|map| map.remove(&display_id)) {
        Some(_) => Ok(()),
        None => Err(format!(
            "display {display_id} was not created by this driver"
        )),
    }
}

unsafe fn class(name: &str) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| format!("{name} is not available on this macOS build"))
}

unsafe fn build(width: u32, height: u32, name: &str, serial: u32) -> Result<Handle, String> {
    let descriptor_class = class("CGVirtualDisplayDescriptor")?;
    let settings_class = class("CGVirtualDisplaySettings")?;
    let mode_class = class("CGVirtualDisplayMode")?;
    let display_class = class("CGVirtualDisplay")?;

    let descriptor: Retained<AnyObject> = msg_send_id![descriptor_class, new];
    let ns_name = NSString::from_str(name);
    let queue = dispatch_get_global_queue(GLOBAL_QUEUE_PRIORITY_HIGH, 0) as *mut AnyObject;
    let size_mm = NSSize::new(
        25.4 * f64::from(width) / PIXELS_PER_INCH,
        25.4 * f64::from(height) / PIXELS_PER_INCH,
    );
    let _: () = msg_send![&*descriptor, setName: &*ns_name];
    let _: () = msg_send![&*descriptor, setMaxPixelsWide: width];
    let _: () = msg_send![&*descriptor, setMaxPixelsHigh: height];
    let _: () = msg_send![&*descriptor, setSizeInMillimeters: size_mm];
    let _: () = msg_send![&*descriptor, setWhitePoint: NSPoint::new(0.3125, 0.3291)];
    let _: () = msg_send![&*descriptor, setBluePrimary: NSPoint::new(0.1494, 0.0557)];
    let _: () = msg_send![&*descriptor, setGreenPrimary: NSPoint::new(0.2559, 0.6983)];
    let _: () = msg_send![&*descriptor, setRedPrimary: NSPoint::new(0.6797, 0.3203)];
    let _: () = msg_send![&*descriptor, setVendorID: VENDOR_ID];
    let _: () = msg_send![&*descriptor, setProductID: PRODUCT_ID];
    let _: () = msg_send![&*descriptor, setSerialNum: serial];
    let _: () = msg_send![&*descriptor, setQueue: queue];

    let display: Option<Retained<AnyObject>> =
        msg_send_id![msg_send_id![display_class, alloc], initWithDescriptor: &*descriptor];
    let display = display.ok_or("CGVirtualDisplay initWithDescriptor failed")?;

    let settings: Retained<AnyObject> = msg_send_id![settings_class, new];
    let hidpi: u32 = 0;
    let _: () = msg_send![&*settings, setHiDPI: hidpi];
    let mode: Retained<AnyObject> = msg_send_id![
        msg_send_id![mode_class, alloc],
        initWithWidth: width,
        height: height,
        refreshRate: REFRESH_HZ
    ];
    let modes = NSArray::from_vec(vec![mode]);
    let _: () = msg_send![&*settings, setModes: &*modes];
    let applied: bool = msg_send![&*display, applySettings: &*settings];
    if !applied {
        return Err("CGVirtualDisplay applySettings failed".into());
    }
    Ok(Handle(display))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_limits_reject_out_of_range_edges() {
        assert!(validate_size(1920, 1080).is_ok());
        assert!(validate_size(639, 1080).is_err());
        assert!(validate_size(1920, 7681).is_err());
    }

    #[test]
    fn removing_an_unknown_display_is_an_error() {
        assert!(remove(4_000_000_000).is_err());
    }
}
