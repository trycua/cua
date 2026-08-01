//! Exact macOS setup for Chrome's per-instance remote-debugging toggle.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::browser::{
    BrowserProduct, BrowserRefusal, BrowserRefusalCode, BrowserSetupDescriptor,
    EXISTING_PROFILE_SETUP_READY_TIMEOUT,
};

use crate::ax::bindings::{
    copy_number_attr, copy_string_attr, element_screen_center, focused_element_of_pid,
    kAXErrorSuccess, perform_action, set_bool_attr_true, set_string_attr, AXUIElementRef,
};
use crate::ax::tree::{walk_tree, AXNode, TreeWalkResult};

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn field_equals(node: &AXNode, expected: &str) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn has_action(node: &AXNode, action: &str) -> bool {
    node.actions.iter().any(|value| value == action)
}

fn release_actionable_nodes(nodes: &[AXNode]) {
    for node in nodes.iter().filter(|node| node.element_index.is_some()) {
        unsafe { CFRelease(node.element_ptr as CFTypeRef) };
    }
}

fn unique_actionable(
    nodes: &[AXNode],
    role: &str,
    label: &str,
    action: &str,
) -> Result<Option<usize>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| node.role == role && field_equals(node, label) && has_action(node, action))
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("multiple exact {role} controls matched {label:?}"),
        )),
    }
}

fn setup_page_proven(nodes: &[AXNode], descriptor: &BrowserSetupDescriptor) -> bool {
    let exact_url = nodes.iter().any(|node| {
        node.role == "AXTextField"
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
    });
    let exact_page = nodes.iter().any(|node| {
        node.role == "AXWebArea"
            && descriptor
                .page_titles
                .iter()
                .any(|title| field_equals(node, title))
    });
    let exact_heading = nodes
        .iter()
        .any(|node| node.role == "AXHeading" && field_equals(node, descriptor.page_heading));
    exact_url && exact_page && exact_heading
}

fn native_setup_page_proven(nodes: &[AXNode], descriptor: &BrowserSetupDescriptor) -> bool {
    let exact_urls = nodes
        .iter()
        .filter(|node| {
            node.role == "AXTextField"
                && field_equals(node, "Address and search bar")
                && node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
        })
        .count();
    let exact_selected_tabs = nodes
        .iter()
        .filter(|node| {
            node.role == "AXRadioButton"
                && node.selected == Some(true)
                && descriptor
                    .page_titles
                    .iter()
                    .any(|title| field_equals(node, title))
        })
        .count();
    let omnibox_popup_open = nodes
        .iter()
        .any(|node| node.role == "AXWebArea" && field_equals(node, "Omnibox Popup"));
    exact_urls == 1 && exact_selected_tabs == 1 && !omnibox_popup_open
}

fn native_setup_page_committed(
    pid: i32,
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> bool {
    if !native_setup_page_proven(nodes, descriptor) {
        return false;
    }
    let exact_omnibox = nodes.iter().find(|node| {
        node.role == "AXTextField"
            && node.element_index.is_some()
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
    });
    let Some(exact_omnibox) = exact_omnibox else {
        return false;
    };
    let focused = unsafe { focused_element_of_pid(pid) };
    let omnibox_focused = focused.is_some_and(|element| {
        let matches = is_same_element(element as usize, exact_omnibox.element_ptr);
        unsafe { CFRelease(element as CFTypeRef) };
        matches
    });
    !omnibox_focused
}

fn exact_setup_checkbox(
    tree: &TreeWalkResult,
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    if tree.truncated {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s setup accessibility proof was truncated",
                descriptor.product_name
            ),
        ));
    }
    let nodes = &tree.nodes;
    if !setup_page_proven(nodes, descriptor) {
        return Ok(None);
    }
    unique_actionable(nodes, "AXCheckBox", descriptor.checkbox_label, "AXPress")
}

fn unique_omnibox(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXTextField", "Address and search bar", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the approved {} window has no exact address-and-search field",
                descriptor.product_name
            ),
        )
    })
}

fn is_exact_setup_suggestion(value: &str, setup_url: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case(setup_url)
        || value.strip_prefix(setup_url).is_some_and(|suffix| {
            suffix.eq_ignore_ascii_case(", press Tab then Enter to Remove Suggestion.")
        })
}

fn node_is_exact_setup_suggestion(node: &AXNode, setup_url: &str) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| is_exact_setup_suggestion(value, setup_url))
}

fn exact_omnibox_suggestion(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    let omniboxes = nodes
        .iter()
        .filter(|node| {
            node.role == "AXTextField"
                && field_equals(node, "Address and search bar")
                && node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
        })
        .count();
    match omniboxes {
        0 => return Ok(None),
        1 => {}
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{} exposed multiple exact address fields containing the setup URL",
                    descriptor.product_name
                ),
            ))
        }
    }
    let popups = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXWebArea" && field_equals(node, "Omnibox Popup"))
        .collect::<Vec<_>>();
    let (popup_index, popup) = match popups.as_slice() {
        [] => return Ok(None),
        [(index, popup)] => (*index, *popup),
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{} exposed multiple exact omnibox suggestion popups",
                    descriptor.product_name
                ),
            ))
        }
    };
    let end = nodes
        .iter()
        .enumerate()
        .skip(popup_index + 1)
        .find(|(_, node)| node.depth <= popup.depth)
        .map_or(nodes.len(), |(index, _)| index);
    let matches = nodes[popup_index + 1..end]
        .iter()
        .filter(|node| {
            node.role == "AXMenuItem"
                && node_is_exact_setup_suggestion(node, descriptor.setup_url)
                && has_action(node, "AXPress")
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{} exposed multiple exact setup URL suggestions",
                descriptor.product_name
            ),
        )),
    }
}

fn new_tab_button(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXButton", "New Tab", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the approved {} window has no exact New Tab button",
                descriptor.product_name
            ),
        )
    })
}

fn is_same_element(left: usize, right: usize) -> bool {
    unsafe { CFEqual(left as CFTypeRef, right as CFTypeRef) != 0 }
}

fn select_new_tab_close_button(
    before: &[AXNode],
    after: &[AXNode],
    same_element: impl Fn(usize, usize) -> bool,
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    let prior_tabs = before
        .iter()
        .filter(|node| node.role == "AXRadioButton" && node.element_index.is_some())
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    let new_tabs = after
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.role == "AXRadioButton"
                && node.element_index.is_some()
                && !prior_tabs
                    .iter()
                    .any(|prior| same_element(*prior, node.element_ptr))
        })
        .collect::<Vec<_>>();
    let (tab_index, tab) = match new_tabs.as_slice() {
        [(index, tab)] => (*index, *tab),
        [] => return Ok(None),
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{} exposed multiple new tab candidates",
                    descriptor.product_name
                ),
            ))
        }
    };
    let end = after
        .iter()
        .enumerate()
        .skip(tab_index + 1)
        .find(|(_, node)| node.depth <= tab.depth)
        .map_or(after.len(), |(index, _)| index);
    let close_buttons = after[tab_index + 1..end]
        .iter()
        .filter(|node| {
            node.role == "AXButton"
                && descriptor
                    .tab_close_labels
                    .iter()
                    .any(|label| field_equals(node, label))
                && has_action(node, "AXPress")
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match close_buttons.as_slice() {
        [element] => Ok(Some(*element)),
        [] => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the newly created {} tab has no exact Close button",
                descriptor.product_name
            ),
        )),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the newly created {} tab has multiple Close buttons",
                descriptor.product_name
            ),
        )),
    }
}

fn new_tab_close_button(
    before: &[AXNode],
    after: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    let element = select_new_tab_close_button(before, after, is_same_element, descriptor)?;
    if let Some(element) = element {
        unsafe { CFRetain(element as CFTypeRef) };
    }
    Ok(element)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckboxState {
    Off,
    On,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PixelCheckbox {
    screen_x: f64,
    screen_y: f64,
    window_local_x: f64,
    window_local_y: f64,
    window_frame: [f64; 4],
    state: CheckboxState,
}

impl PixelCheckbox {
    fn same_control_as(self, other: Self, tolerance: f64) -> bool {
        (self.window_local_x - other.window_local_x).abs() <= tolerance
            && (self.window_local_y - other.window_local_y).abs() <= tolerance
            && frames_agree(self.window_frame, other.window_frame, tolerance)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SetupGeometry {
    search: (u32, u32, u32, u32),
    scale_x: f64,
    scale_y: f64,
}

fn frames_agree(left: [f64; 4], right: [f64; 4], tolerance: f64) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| (left - right).abs() <= tolerance)
}

fn setup_geometry(
    window_frame: [f64; 4],
    omnibox_frame: [f64; 4],
    screenshot_size: (u32, u32),
) -> Result<SetupGeometry, &'static str> {
    if !window_frame.into_iter().all(f64::is_finite)
        || !omnibox_frame.into_iter().all(f64::is_finite)
        || window_frame[2] <= 0.0
        || window_frame[3] <= 0.0
        || screenshot_size.0 == 0
        || screenshot_size.1 == 0
    {
        return Err("invalid native or screenshot geometry");
    }
    let tolerance = 1.0;
    if omnibox_frame[0] < window_frame[0] - tolerance
        || omnibox_frame[1] < window_frame[1] - tolerance
        || omnibox_frame[0] + omnibox_frame[2] > window_frame[0] + window_frame[2] + tolerance
        || omnibox_frame[1] + omnibox_frame[3] > window_frame[1] + window_frame[3] + tolerance
    {
        return Err("address-field frame is outside the captured window");
    }
    let scale_x = f64::from(screenshot_size.0) / window_frame[2];
    let scale_y = f64::from(screenshot_size.1) / window_frame[3];
    if !scale_x.is_finite()
        || !scale_y.is_finite()
        || scale_x <= 0.0
        || scale_y <= 0.0
        || (scale_x - scale_y).abs() > 0.15
    {
        return Err("screenshot and native window scales disagree");
    }
    let local_omnibox_bottom = omnibox_frame[1] + omnibox_frame[3] - window_frame[1];
    if local_omnibox_bottom < -tolerance || local_omnibox_bottom > window_frame[3] + tolerance {
        return Err("address-field frame has invalid window-local geometry");
    }
    let left = (window_frame[2] * 0.15 * scale_x).round().max(0.0) as u32;
    let top = ((local_omnibox_bottom + 30.0) * scale_y)
        .round()
        .min(f64::from(screenshot_size.1)) as u32;
    let right = (window_frame[2] * 0.40 * scale_x)
        .round()
        .clamp(0.0, f64::from(screenshot_size.0)) as u32;
    let bottom = ((local_omnibox_bottom + 150.0) * scale_y)
        .round()
        .clamp(0.0, f64::from(screenshot_size.1)) as u32;
    if left >= right || top >= bottom {
        return Err("bounded setup search region is empty");
    }
    Ok(SetupGeometry {
        search: (left, top, right, bottom),
        scale_x,
        scale_y,
    })
}

fn pixel_to_screen(
    window_frame: [f64; 4],
    pixel: (u32, u32),
    geometry: SetupGeometry,
) -> (f64, f64, f64, f64) {
    let local_x = f64::from(pixel.0) / geometry.scale_x;
    let local_y = f64::from(pixel.1) / geometry.scale_y;
    (
        window_frame[0] + local_x,
        window_frame[1] + local_y,
        local_x,
        local_y,
    )
}

fn unique_pixel_checkbox(
    matches: &[(u32, u32, CheckboxState)],
    window_frame: [f64; 4],
    geometry: SetupGeometry,
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<PixelCheckbox>, BrowserRefusal> {
    let [(center_x, center_y, state)] = matches else {
        return match matches.len() {
            0 => Ok(None),
            _ => Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{}'s exact setup page exposed multiple checkbox-shaped controls in the bounded trusted region",
                    descriptor.product_name
                ),
            )),
        };
    };
    let (screen_x, screen_y, window_local_x, window_local_y) =
        pixel_to_screen(window_frame, (*center_x, *center_y), geometry);
    Ok(Some(PixelCheckbox {
        screen_x,
        screen_y,
        window_local_x,
        window_local_y,
        window_frame,
        state: *state,
    }))
}

fn is_checkbox_edge_pixel(pixel: image::Rgba<u8>) -> bool {
    let [red, green, blue, _] = pixel.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let neutral_outline = max.saturating_sub(min) <= 20 && (70..=225).contains(&max);
    let chrome_blue = blue > 140 && blue > red.saturating_add(35) && blue > green;
    neutral_outline || chrome_blue
}

fn is_chrome_blue(pixel: image::Rgba<u8>) -> bool {
    let [red, green, blue, _] = pixel.0;
    blue > 140 && blue > red.saturating_add(35) && blue > green
}

fn has_checkbox_border_coverage(
    component: &[(u32, u32)],
    bounds: (u32, u32, u32, u32),
    scale: f64,
) -> bool {
    let (min_x, min_y, max_x, max_y) = bounds;
    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    let corner_inset = (2.0 * scale).round().max(2.0) as u32;
    let doubled_inset = corner_inset.saturating_mul(2);
    if width <= doubled_inset || height <= doubled_inset {
        return false;
    }
    let horizontal_span = width - doubled_inset;
    let vertical_span = height - doubled_inset;
    let horizontal_edges = [min_y, max_y].into_iter().all(|sample_y| {
        let edge_pixels = component
            .iter()
            .filter(|(sample_x, component_y)| {
                *component_y == sample_y
                    && *sample_x >= min_x + corner_inset
                    && *sample_x <= max_x - corner_inset
            })
            .count() as u32;
        edge_pixels * 3 >= horizontal_span * 2
    });
    let vertical_edges = [min_x, max_x].into_iter().all(|sample_x| {
        let edge_pixels = component
            .iter()
            .filter(|(component_x, sample_y)| {
                *component_x == sample_x
                    && *sample_y >= min_y + corner_inset
                    && *sample_y <= max_y - corner_inset
            })
            .count() as u32;
        edge_pixels * 3 >= vertical_span * 2
    });
    horizontal_edges && vertical_edges
}

fn detect_checkbox_pixels(
    image: &image::RgbaImage,
    search: (u32, u32, u32, u32),
    scale: f64,
) -> Vec<(u32, u32, CheckboxState)> {
    use std::collections::VecDeque;

    let (left, top, right, bottom) = search;
    if left >= right || top >= bottom || right > image.width() || bottom > image.height() {
        return Vec::new();
    }
    let width = right - left;
    let height = bottom - top;
    let mut visited = vec![false; (width * height) as usize];
    let min_side = (9.0 * scale).round().max(7.0) as u32;
    let max_side = (30.0 * scale).round().max(min_side as f64) as u32;
    let mut candidates = Vec::new();

    for y in top..bottom {
        for x in left..right {
            let local = ((y - top) * width + (x - left)) as usize;
            if visited[local] || !is_checkbox_edge_pixel(*image.get_pixel(x, y)) {
                continue;
            }
            visited[local] = true;
            let mut queue = VecDeque::from([(x, y)]);
            let mut component = Vec::new();
            let (mut min_x, mut max_x, mut min_y, mut max_y) = (x, x, y, y);
            while let Some((current_x, current_y)) = queue.pop_front() {
                component.push((current_x, current_y));
                min_x = min_x.min(current_x);
                max_x = max_x.max(current_x);
                min_y = min_y.min(current_y);
                max_y = max_y.max(current_y);
                for delta_y in -1i32..=1 {
                    for delta_x in -1i32..=1 {
                        if delta_x == 0 && delta_y == 0 {
                            continue;
                        }
                        let next_x = current_x as i32 + delta_x;
                        let next_y = current_y as i32 + delta_y;
                        if next_x < left as i32
                            || next_x >= right as i32
                            || next_y < top as i32
                            || next_y >= bottom as i32
                        {
                            continue;
                        }
                        let next_x = next_x as u32;
                        let next_y = next_y as u32;
                        let next_local = ((next_y - top) * width + (next_x - left)) as usize;
                        if !visited[next_local]
                            && is_checkbox_edge_pixel(*image.get_pixel(next_x, next_y))
                        {
                            visited[next_local] = true;
                            queue.push_back((next_x, next_y));
                        }
                    }
                }
            }

            let component_width = max_x - min_x + 1;
            let component_height = max_y - min_y + 1;
            let side_delta = component_width.abs_diff(component_height);
            if component_width < min_side
                || component_width > max_side
                || component_height < min_side
                || component_height > max_side
                || side_delta > (3.0 * scale).round().max(2.0) as u32
            {
                continue;
            }
            let perimeter = 2 * (component_width + component_height);
            if component.len() < (perimeter / 3) as usize {
                continue;
            }
            if !has_checkbox_border_coverage(
                component.as_slice(),
                (min_x, min_y, max_x, max_y),
                scale,
            ) {
                continue;
            }
            let center_x = (min_x + max_x) / 2;
            let center_y = (min_y + max_y) / 2;
            let blue_pixels = (min_y..=max_y)
                .flat_map(|sample_y| {
                    (min_x..=max_x).map(move |sample_x| *image.get_pixel(sample_x, sample_y))
                })
                .filter(|pixel| is_chrome_blue(*pixel))
                .count();
            let area = (component_width * component_height) as usize;
            let state = if blue_pixels * 4 >= area {
                CheckboxState::On
            } else {
                let center = *image.get_pixel(center_x, center_y);
                let [red, green, blue, _] = center.0;
                let light_center = red >= 235 && green >= 235 && blue >= 235;
                let dark_center = red.max(green).max(blue) <= 100
                    && red.max(green).max(blue) - red.min(green).min(blue) <= 18;
                if !light_center && !dark_center {
                    continue;
                }
                CheckboxState::Off
            };
            candidates.push((center_x, center_y, state));
        }
    }
    candidates
}

fn exact_pixel_setup_checkbox(
    pid: i32,
    tree: &TreeWalkResult,
    window_id: u32,
    descriptor: &BrowserSetupDescriptor,
    navigation_committed: bool,
) -> Result<Option<PixelCheckbox>, BrowserRefusal> {
    if !navigation_committed {
        return Ok(None);
    }
    if tree.truncated {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s setup accessibility proof was truncated",
                descriptor.product_name
            ),
        ));
    }
    let nodes = &tree.nodes;
    if !native_setup_page_committed(pid, nodes, descriptor) {
        return Ok(None);
    }
    let window_frames = nodes
        .iter()
        .filter(|node| node.role == "AXWindow")
        .filter_map(|node| node.frame)
        .collect::<Vec<_>>();
    let [window_frame] = window_frames.as_slice() else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s exact setup tab did not expose one native window frame",
                descriptor.product_name
            ),
        ));
    };
    let omnibox_frames = nodes
        .iter()
        .filter(|node| {
            node.role == "AXTextField"
                && node.element_index.is_some()
                && field_equals(node, "Address and search bar")
                && node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
        })
        .filter_map(|node| node.frame)
        .collect::<Vec<_>>();
    let [omnibox_frame] = omnibox_frames.as_slice() else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s exact setup tab did not expose one address-field frame",
                descriptor.product_name
            ),
        ));
    };
    let capture_bounds = crate::windows::window_bounds_by_id(window_id).ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s exact setup window disappeared before capture",
                descriptor.product_name
            ),
        )
    })?;
    let capture_frame = [
        capture_bounds.x,
        capture_bounds.y,
        capture_bounds.width,
        capture_bounds.height,
    ];
    if !frames_agree(*window_frame, capture_frame, 1.0) {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s native and captured setup-window bounds disagree",
                descriptor.product_name
            ),
        ));
    }
    let png = crate::capture::screenshot_window_bytes(window_id).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!(
                "could not capture {}'s exact setup window: {error}",
                descriptor.product_name
            ),
        )
    })?;
    let screenshot = image::load_from_memory(&png)
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "could not decode {}'s exact setup window: {error}",
                    descriptor.product_name
                ),
            )
        })?
        .to_rgba8();
    let geometry = setup_geometry(
        capture_frame,
        *omnibox_frame,
        (screenshot.width(), screenshot.height()),
    )
    .map_err(|cause| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{}'s setup screenshot geometry was refused: {cause}",
                descriptor.product_name,
            ),
        )
    })?;
    let matches = detect_checkbox_pixels(
        &screenshot,
        geometry.search,
        (geometry.scale_x + geometry.scale_y) / 2.0,
    );
    unique_pixel_checkbox(matches.as_slice(), capture_frame, geometry, descriptor)
}

fn press_pixel_checkbox(
    pid: i32,
    window_id: u32,
    checkbox: PixelCheckbox,
    descriptor: &BrowserSetupDescriptor,
    navigation_committed: bool,
) -> anyhow::Result<bool> {
    crate::input::skylight::with_foreground_assist(pid, window_id, || {
        std::thread::sleep(Duration::from_millis(60));
        if crate::apps::frontmost_pid() != Some(pid) {
            anyhow::bail!("the approved browser lost foreground before the setup click");
        }
        let current = walk_tree(pid, Some(window_id), None);
        let validation = (|| {
            if current.truncated
                || !navigation_committed
                || !native_setup_page_committed(pid, &current.nodes, descriptor)
            {
                anyhow::bail!("the exact committed setup-page proof changed before the click");
            }
            let frames = current
                .nodes
                .iter()
                .filter(|node| node.role == "AXWindow")
                .filter_map(|node| node.frame)
                .collect::<Vec<_>>();
            let [frame] = frames.as_slice() else {
                anyhow::bail!("the exact setup window frame became ambiguous before the click");
            };
            if frame
                .iter()
                .zip(checkbox.window_frame)
                .any(|(current, captured)| (*current - captured).abs() > 1.0)
            {
                anyhow::bail!("the exact setup window moved before the click");
            }
            if checkbox.screen_x < frame[0]
                || checkbox.screen_y < frame[1]
                || checkbox.screen_x >= frame[0] + frame[2]
                || checkbox.screen_y >= frame[1] + frame[3]
            {
                anyhow::bail!("the setup click point left the exact window");
            }
            let refreshed = exact_pixel_setup_checkbox(pid, &current, window_id, descriptor, true)
                .map_err(|error| anyhow::anyhow!(error.message))?
                .ok_or_else(|| {
                    anyhow::anyhow!("the exact setup checkbox disappeared before the click")
                })?;
            if !checkbox.same_control_as(refreshed, 3.0) || checkbox.state != refreshed.state {
                anyhow::bail!("the exact setup checkbox changed before the click");
            }
            Ok(refreshed)
        })();
        release_actionable_nodes(&current.nodes);
        let refreshed = validation?;
        crate::input::mouse::click_at_xy_with_window_local(
            pid,
            refreshed.screen_x,
            refreshed.screen_y,
            refreshed.window_local_x,
            refreshed.window_local_y,
            window_id,
            1,
            &[],
        )?;
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    })
}

fn checkbox_state(value: Option<f64>) -> Result<CheckboxState, BrowserRefusal> {
    match value {
        Some(value) if value.abs() < f64::EPSILON => Ok(CheckboxState::Off),
        Some(value) if (value - 1.0).abs() < f64::EPSILON => Ok(CheckboxState::On),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the exact remote-debugging checkbox had an unknown checked state",
        )),
    }
}

pub struct SetupUiHandle {
    descriptor: &'static BrowserSetupDescriptor,
    close_button: Option<usize>,
    enable_attempted: bool,
    trusted_checkbox_fallback_attempted: bool,
    pixel_checkbox_fallback_attempted: bool,
    setup_navigation_committed: bool,
    remote_debugging_mutation_possible: bool,
    pixel_checkbox: Option<PixelCheckbox>,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub used_bounded_pixel_fallback: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
}

fn remote_debugging_cleanup_required(enabled: bool, mutation_possible: bool) -> bool {
    enabled || mutation_possible
}

impl SetupUiHandle {
    fn rollback_remote_debugging(&mut self, pid: i32, window_id: u32) -> bool {
        if !remote_debugging_cleanup_required(
            self.enabled_remote_debugging,
            self.remote_debugging_mutation_possible,
        ) {
            return true;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut pressed_rollback = false;
        let mut trusted_fallback_attempted = false;
        loop {
            let tree = walk_tree(pid, Some(window_id), None);
            let checkbox = exact_setup_checkbox(&tree, self.descriptor);
            let result = match checkbox {
                Ok(Some(element)) => {
                    let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                    match checkbox_state(value) {
                        Ok(CheckboxState::Off) => Some(true),
                        Ok(CheckboxState::On) => {
                            if !pressed_rollback {
                                let pressed =
                                    unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                                if pressed == kAXErrorSuccess {
                                    pressed_rollback = true;
                                    None
                                } else {
                                    Some(false)
                                }
                            } else if self.descriptor.product == BrowserProduct::MicrosoftEdge
                                && !trusted_fallback_attempted
                            {
                                let center =
                                    unsafe { element_screen_center(element as AXUIElementRef) };
                                trusted_fallback_attempted = true;
                                release_actionable_nodes(&tree.nodes);
                                let Some((x, y)) = center else {
                                    return false;
                                };
                                self.foregrounded_window = true;
                                self.injected_global_input = true;
                                let restored = crate::input::skylight::with_foreground_assist(
                                    pid,
                                    window_id,
                                    || {
                                        std::thread::sleep(Duration::from_millis(60));
                                        if crate::apps::frontmost_pid() != Some(pid) {
                                            anyhow::bail!(
                                                "the approved browser lost foreground before the rollback click"
                                            );
                                        }
                                        crate::input::mouse::click_at_xy_desktop_preserving_cursor(
                                            x, y,
                                        )?;
                                        std::thread::sleep(Duration::from_millis(60));
                                        Ok(())
                                    },
                                )
                                .unwrap_or(false);
                                if !restored {
                                    return false;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                                continue;
                            } else {
                                None
                            }
                        }
                        Err(_) => Some(false),
                    }
                }
                Ok(None) => {
                    let pixel_checkbox = exact_pixel_setup_checkbox(
                        pid,
                        &tree,
                        window_id,
                        self.descriptor,
                        self.setup_navigation_committed,
                    );
                    if matches!(pixel_checkbox, Ok(Some(_))) {
                        self.used_bounded_pixel_fallback = true;
                    }
                    match pixel_checkbox {
                        Ok(Some(
                            checkbox @ PixelCheckbox {
                                state: CheckboxState::Off,
                                ..
                            },
                        )) => Some(
                            self.pixel_checkbox
                                .is_none_or(|original| original.same_control_as(checkbox, 3.0)),
                        ),
                        Ok(Some(checkbox)) if !trusted_fallback_attempted => {
                            if self
                                .pixel_checkbox
                                .is_some_and(|original| !original.same_control_as(checkbox, 3.0))
                            {
                                Some(false)
                            } else {
                                trusted_fallback_attempted = true;
                                release_actionable_nodes(&tree.nodes);
                                self.injected_global_input = true;
                                match press_pixel_checkbox(
                                    pid,
                                    window_id,
                                    checkbox,
                                    self.descriptor,
                                    self.setup_navigation_committed,
                                ) {
                                    Ok(fronted) => self.foregrounded_window |= fronted,
                                    Err(_) => return false,
                                }
                                std::thread::sleep(Duration::from_millis(100));
                                continue;
                            }
                        }
                        Ok(Some(_)) | Ok(None) => None,
                        Err(_) => Some(false),
                    }
                }
                Err(_) => Some(false),
            };
            release_actionable_nodes(&tree.nodes);
            if let Some(done) = result {
                if done {
                    self.enabled_remote_debugging = false;
                    self.remote_debugging_mutation_possible = false;
                }
                return done;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn abort(mut self, pid: i32, window_id: u32, error: BrowserRefusal) -> BrowserRefusal {
        let enabled_remote_debugging =
            self.enabled_remote_debugging || self.remote_debugging_mutation_possible;
        let restored_remote_debugging = self.rollback_remote_debugging(pid, window_id);
        let used_bounded_pixel_fallback = self.used_bounded_pixel_fallback;
        let opened_setup_page = self.opened_setup_page;
        let focused_setup_address_field = self.focused_setup_address_field;
        let foregrounded_window = self.foregrounded_window;
        let injected_global_input = self.injected_global_input;
        let closed_setup_page = self.close().unwrap_or(false);
        let mut error = error;
        let cause = error.detail.take();
        error.with_detail(serde_json::json!({
            "setup_side_effects": {
                "opened_setup_page": opened_setup_page,
                "closed_setup_page": closed_setup_page,
                "focused_setup_address_field": focused_setup_address_field,
                "enabled_remote_debugging": enabled_remote_debugging,
                "used_bounded_pixel_fallback": used_bounded_pixel_fallback,
                "foregrounded_window": foregrounded_window,
                "injected_global_input": injected_global_input,
                "restored_remote_debugging": restored_remote_debugging,
            },
            "cause": cause,
        }))
    }

    pub fn close_for_success(
        mut self,
        pid: i32,
        window_id: u32,
    ) -> Result<Option<bool>, BrowserRefusal> {
        let Some(element) = self.close_button.take() else {
            return Ok(None);
        };
        let result = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
        unsafe { CFRelease(element as CFTypeRef) };
        if result == kAXErrorSuccess {
            return Ok(Some(true));
        }
        let product_name = self.descriptor.product_name;
        Err(self.abort(
            pid,
            window_id,
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{}'s exact temporary-tab Close action became stale before AXPress",
                    product_name
                ),
            ),
        ))
    }

    /// Close the temporary setup tab. `None` means the approved window was
    /// already displaying the exact setup page and no tab was opened.
    pub fn close(mut self) -> Option<bool> {
        let element = self.close_button.take()?;
        let result = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
        unsafe { CFRelease(element as CFTypeRef) };
        Some(result == kAXErrorSuccess)
    }
}

impl Drop for SetupUiHandle {
    fn drop(&mut self) {
        if let Some(element) = self.close_button.take() {
            let _ = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
            unsafe { CFRelease(element as CFTypeRef) };
        }
    }
}

type PendingSetupKey = (i32, u32);

fn pending_setups() -> &'static Mutex<HashMap<PendingSetupKey, SetupUiHandle>> {
    static PENDING: OnceLock<Mutex<HashMap<PendingSetupKey, SetupUiHandle>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn retain_pending(
    pid: i32,
    window_id: u32,
    handle: SetupUiHandle,
) -> Result<(), BrowserRefusal> {
    let mut pending = pending_setups().lock().unwrap();
    if pending.contains_key(&(pid, window_id)) {
        drop(pending);
        return Err(handle.abort(
            pid,
            window_id,
            refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "another approved browser setup is already pending for this exact window",
            ),
        ));
    }
    pending.insert((pid, window_id), handle);
    Ok(())
}

pub fn commit_pending(pid: i32, window_id: u32) -> Result<bool, BrowserRefusal> {
    let handle = pending_setups()
        .lock()
        .unwrap()
        .remove(&(pid, window_id))
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "the exact pending browser setup cleanup handle is missing",
            )
        })?;
    Ok(handle.close_for_success(pid, window_id)?.unwrap_or(false))
}

pub fn abort_pending(pid: i32, window_id: u32, error: BrowserRefusal) -> BrowserRefusal {
    match pending_setups().lock().unwrap().remove(&(pid, window_id)) {
        Some(handle) => handle.abort(pid, window_id, error),
        None => error.with_detail(serde_json::json!({
            "setup_cleanup": "the exact pending browser setup cleanup handle was missing"
        })),
    }
}

pub fn enable(
    pid: i32,
    window_id: u32,
    descriptor: &'static BrowserSetupDescriptor,
) -> Result<SetupUiHandle, BrowserRefusal> {
    let initial = walk_tree(pid, Some(window_id), None);
    let initial_checkbox = exact_setup_checkbox(&initial, descriptor);
    let mut handle = match initial_checkbox {
        Ok(Some(_)) => SetupUiHandle {
            descriptor,
            close_button: None,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            pixel_checkbox_fallback_attempted: false,
            setup_navigation_committed: false,
            remote_debugging_mutation_possible: false,
            pixel_checkbox: None,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            used_bounded_pixel_fallback: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
        },
        Ok(None) => {
            let new_tab = new_tab_button(&initial.nodes, descriptor);
            let new_tab = match new_tab {
                Ok(element) => element,
                Err(error) => {
                    release_actionable_nodes(&initial.nodes);
                    return Err(error);
                }
            };
            let pressed = unsafe { perform_action(new_tab as AXUIElementRef, "AXPress") };
            if pressed != kAXErrorSuccess {
                release_actionable_nodes(&initial.nodes);
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!(
                        "{}'s exact New Tab action became stale before AXPress",
                        descriptor.product_name
                    ),
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            let (created, close_button) = loop {
                let created = walk_tree(pid, Some(window_id), None);
                match new_tab_close_button(&initial.nodes, &created.nodes, descriptor) {
                    Ok(Some(element)) => break (created, element),
                    Ok(None) if Instant::now() < deadline => {
                        release_actionable_nodes(&created.nodes);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Ok(None) => {
                        release_actionable_nodes(&initial.nodes);
                        release_actionable_nodes(&created.nodes);
                        return Err(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!(
                                "{} did not expose exactly one newly created tab",
                                descriptor.product_name
                            ),
                        )
                        .with_detail(serde_json::json!({
                            "setup_side_effects": {
                                "opened_setup_page": "unknown",
                                "closed_setup_page": false,
                                "enabled_remote_debugging": false,
                            }
                        })));
                    }
                    Err(error) => {
                        release_actionable_nodes(&initial.nodes);
                        release_actionable_nodes(&created.nodes);
                        return Err(error.with_detail(serde_json::json!({
                            "setup_side_effects": {
                                "opened_setup_page": "unknown",
                                "closed_setup_page": false,
                                "enabled_remote_debugging": false,
                            }
                        })));
                    }
                }
            };
            release_actionable_nodes(&initial.nodes);
            let mut handle = SetupUiHandle {
                descriptor,
                close_button: Some(close_button),
                enable_attempted: false,
                trusted_checkbox_fallback_attempted: false,
                pixel_checkbox_fallback_attempted: false,
                setup_navigation_committed: false,
                remote_debugging_mutation_possible: false,
                pixel_checkbox: None,
                opened_setup_page: true,
                enabled_remote_debugging: false,
                used_bounded_pixel_fallback: false,
                focused_setup_address_field: false,
                foregrounded_window: false,
                injected_global_input: false,
            };
            let omnibox = unique_omnibox(&created.nodes, descriptor);
            let omnibox = match omnibox {
                Ok(element) => element,
                Err(error) => {
                    release_actionable_nodes(&created.nodes);
                    return Err(handle.abort(pid, window_id, error));
                }
            };
            let focused = unsafe { perform_action(omnibox as AXUIElementRef, "AXPress") };
            if focused != kAXErrorSuccess {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field became stale before AXPress",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            handle.focused_setup_address_field = true;
            let wrote_url = unsafe {
                set_string_attr(omnibox as AXUIElementRef, "AXValue", descriptor.setup_url)
            };
            if wrote_url != kAXErrorSuccess {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field rejected the fixed setup URL",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            let exact_value = unsafe { copy_string_attr(omnibox as AXUIElementRef, "AXValue") }
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url));
            let can_confirm = created
                .nodes
                .iter()
                .any(|node| node.element_ptr == omnibox && has_action(node, "AXConfirm"));
            let confirmed = can_confirm
                && unsafe { perform_action(omnibox as AXUIElementRef, "AXConfirm") }
                    == kAXErrorSuccess;
            unsafe { CFRetain(omnibox as CFTypeRef) };
            release_actionable_nodes(&created.nodes);
            if !exact_value {
                unsafe { CFRelease(omnibox as CFTypeRef) };
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field did not retain the fixed setup URL",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            if confirmed {
                unsafe { CFRelease(omnibox as CFTypeRef) };
            } else {
                let deadline = Instant::now() + Duration::from_secs(2);
                let suggestion = loop {
                    let popup = walk_tree(pid, Some(window_id), None);
                    match exact_omnibox_suggestion(&popup.nodes, descriptor) {
                        Ok(Some(element)) => break Ok(Some((popup, element))),
                        Ok(None) if Instant::now() < deadline => {
                            release_actionable_nodes(&popup.nodes);
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Ok(None) => {
                            release_actionable_nodes(&popup.nodes);
                            break Ok(None);
                        }
                        Err(error) => {
                            release_actionable_nodes(&popup.nodes);
                            break Err(error);
                        }
                    }
                };
                match suggestion {
                    Ok(Some((popup, suggestion))) => {
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        let navigation =
                            unsafe { perform_action(suggestion as AXUIElementRef, "AXPress") };
                        release_actionable_nodes(&popup.nodes);
                        if navigation != kAXErrorSuccess {
                            return Err(handle.abort(
                                pid,
                                window_id,
                                refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    format!(
                                        "{}'s exact fixed-URL suggestion became stale before AXPress",
                                        descriptor.product_name
                                    ),
                                ),
                            ));
                        }
                    }
                    Ok(None) => {
                        handle.foregrounded_window = true;
                        handle.injected_global_input = true;
                        let navigated = crate::input::skylight::with_foreground_assist(
                            pid,
                            window_id,
                            || {
                                std::thread::sleep(Duration::from_millis(60));
                                if crate::apps::frontmost_pid() != Some(pid) {
                                    anyhow::bail!(
                                        "the approved browser lost foreground before setup navigation"
                                    );
                                }
                                let exact_value = unsafe {
                                    copy_string_attr(omnibox as AXUIElementRef, "AXValue")
                                }
                                .is_some_and(|value| {
                                    value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                                });
                                if !exact_value {
                                    anyhow::bail!(
                                        "the exact address field no longer contained the fixed setup URL"
                                    );
                                }
                                let focused = unsafe {
                                    perform_action(omnibox as AXUIElementRef, "AXPress")
                                };
                                if focused != kAXErrorSuccess {
                                    anyhow::bail!(
                                        "the exact address field became stale before setup navigation"
                                    );
                                }
                                let _ = unsafe {
                                    set_bool_attr_true(omnibox as AXUIElementRef, "AXFocused")
                                };
                                std::thread::sleep(Duration::from_millis(60));
                                let focused_element = unsafe { focused_element_of_pid(pid) };
                                let exact_focus = focused_element.is_some_and(|element| {
                                    let matches = is_same_element(element as usize, omnibox);
                                    unsafe { CFRelease(element as CFTypeRef) };
                                    matches
                                });
                                if !exact_focus {
                                    anyhow::bail!(
                                        "the exact address field did not own keyboard focus"
                                    );
                                }
                                crate::input::keyboard::press_key_global("a", &["cmd"])?;
                                std::thread::sleep(Duration::from_millis(30));
                                crate::input::keyboard::type_text(pid, descriptor.setup_url)?;
                                std::thread::sleep(Duration::from_millis(100));
                                let typed_exact_value = unsafe {
                                    copy_string_attr(omnibox as AXUIElementRef, "AXValue")
                                }
                                .is_some_and(|value| {
                                    value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                                });
                                if !typed_exact_value {
                                    anyhow::bail!(
                                        "trusted setup typing did not retain the fixed URL"
                                    );
                                }
                                crate::input::keyboard::press_key_global("return", &[])?;
                                std::thread::sleep(Duration::from_millis(100));
                                Ok(())
                            },
                        )
                        .and_then(|fronted| {
                            if fronted {
                                Ok(())
                            } else {
                                Err(anyhow::anyhow!(
                                    "the bounded setup foreground assist was unavailable"
                                ))
                            }
                        });
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        if let Err(error) = navigated {
                            return Err(handle.abort(
                                pid,
                                window_id,
                                refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    format!(
                                        "could not navigate the exact approved {} setup tab: {error}",
                                        descriptor.product_name
                                    ),
                                ),
                            ));
                        }
                    }
                    Err(error) => {
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        return Err(handle.abort(pid, window_id, error));
                    }
                }
            }
            handle.setup_navigation_committed = true;
            handle
        }
        Err(error) => {
            release_actionable_nodes(&initial.nodes);
            return Err(error);
        }
    };
    if !handle.opened_setup_page {
        release_actionable_nodes(&initial.nodes);
    }

    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree = walk_tree(pid, Some(window_id), None);
        let checkbox = exact_setup_checkbox(&tree, descriptor);
        match checkbox {
            Ok(Some(element)) => {
                let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                match checkbox_state(value) {
                    Ok(CheckboxState::On) => {
                        if handle.enable_attempted || handle.remote_debugging_mutation_possible {
                            handle.enabled_remote_debugging = true;
                        }
                        release_actionable_nodes(&tree.nodes);
                        return Ok(handle);
                    }
                    Ok(CheckboxState::Off) => {
                        if !handle.enable_attempted {
                            handle.enable_attempted = true;
                            handle.remote_debugging_mutation_possible = true;
                            let pressed =
                                unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                            release_actionable_nodes(&tree.nodes);
                            if pressed != kAXErrorSuccess {
                                return Err(handle.abort(pid, window_id, refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    "the exact remote-debugging checkbox became stale before AXPress",
                                )));
                            }
                            continue;
                        }

                        if descriptor.product == BrowserProduct::MicrosoftEdge
                            && !handle.trusted_checkbox_fallback_attempted
                        {
                            let center =
                                unsafe { element_screen_center(element as AXUIElementRef) };
                            handle.trusted_checkbox_fallback_attempted = true;
                            release_actionable_nodes(&tree.nodes);
                            let Some((x, y)) = center else {
                                return Err(handle.abort(
                                    pid,
                                    window_id,
                                    refusal(
                                        BrowserRefusalCode::BrowserWrongTargetRefused,
                                        "the exact remote-debugging checkbox had no stable screen center",
                                    ),
                                ));
                            };
                            handle.foregrounded_window = true;
                            handle.injected_global_input = true;
                            let clicked = crate::input::skylight::with_foreground_assist(
                                pid,
                                window_id,
                                || {
                                    std::thread::sleep(Duration::from_millis(60));
                                    if crate::apps::frontmost_pid() != Some(pid) {
                                        anyhow::bail!(
                                            "the approved browser lost foreground before the setup click"
                                        );
                                    }
                                    crate::input::mouse::click_at_xy_desktop_preserving_cursor(x, y)?;
                                    std::thread::sleep(Duration::from_millis(60));
                                    Ok(())
                                },
                            )
                            .and_then(|fronted| {
                                if fronted {
                                    Ok(())
                                } else {
                                    Err(anyhow::anyhow!(
                                        "the bounded Microsoft Edge foreground assist was unavailable"
                                    ))
                                }
                            });
                            if let Err(error) = clicked {
                                return Err(handle.abort(
                                    pid,
                                    window_id,
                                    refusal(
                                        BrowserRefusalCode::BrowserWrongTargetRefused,
                                        format!(
                                            "could not toggle the exact Microsoft Edge remote-debugging checkbox: {error}"
                                        ),
                                    ),
                                ));
                            }
                            continue;
                        }

                        release_actionable_nodes(&tree.nodes);
                    }
                    Err(error) => {
                        release_actionable_nodes(&tree.nodes);
                        return Err(handle.abort(pid, window_id, error));
                    }
                }
            }
            Ok(None) => match exact_pixel_setup_checkbox(
                pid,
                &tree,
                window_id,
                descriptor,
                handle.setup_navigation_committed,
            ) {
                Ok(Some(
                    checkbox @ PixelCheckbox {
                        state: CheckboxState::On,
                        ..
                    },
                )) => {
                    handle.used_bounded_pixel_fallback = true;
                    if handle
                        .pixel_checkbox
                        .is_some_and(|original| !original.same_control_as(checkbox, 3.0))
                    {
                        release_actionable_nodes(&tree.nodes);
                        return Err(handle.abort(
                            pid,
                            window_id,
                            refusal(
                                BrowserRefusalCode::BrowserWrongTargetRefused,
                                format!(
                                    "the exact {} remote-debugging checkbox changed identity after the setup click",
                                    descriptor.product_name
                                ),
                            ),
                        ));
                    }
                    if handle.remote_debugging_mutation_possible {
                        handle.enabled_remote_debugging = true;
                    }
                    release_actionable_nodes(&tree.nodes);
                    return Ok(handle);
                }
                Ok(Some(checkbox)) if !handle.pixel_checkbox_fallback_attempted => {
                    handle.pixel_checkbox_fallback_attempted = true;
                    handle.remote_debugging_mutation_possible = true;
                    handle.pixel_checkbox = Some(checkbox);
                    handle.used_bounded_pixel_fallback = true;
                    release_actionable_nodes(&tree.nodes);
                    handle.injected_global_input = true;
                    match press_pixel_checkbox(
                        pid,
                        window_id,
                        checkbox,
                        descriptor,
                        handle.setup_navigation_committed,
                    ) {
                        Ok(fronted) => handle.foregrounded_window |= fronted,
                        Err(error) => {
                            return Err(handle.abort(
                                pid,
                                window_id,
                                refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    format!(
                                        "could not toggle the exact {} remote-debugging checkbox: {error}",
                                        descriptor.product_name
                                    ),
                                ),
                            ));
                        }
                    }
                    continue;
                }
                Ok(Some(_)) | Ok(None) => release_actionable_nodes(&tree.nodes),
                Err(error) => {
                    release_actionable_nodes(&tree.nodes);
                    return Err(handle.abort(pid, window_id, error));
                }
            },
            Err(error) => {
                release_actionable_nodes(&tree.nodes);
                return Err(handle.abort(pid, window_id, error));
            }
        }
        if Instant::now() >= deadline {
            return Err(handle.abort(
                pid,
                window_id,
                refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!(
                        "the exact {} remote-debugging setup page did not become ready",
                        descriptor.product_name
                    ),
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::{existing_profile_setup_descriptor, BrowserProduct};

    fn chrome() -> &'static BrowserSetupDescriptor {
        existing_profile_setup_descriptor(BrowserProduct::GoogleChrome).unwrap()
    }

    fn node(role: &str, title: Option<&str>, value: Option<&str>, actions: &[&str]) -> AXNode {
        AXNode {
            element_index: (!actions.is_empty()).then_some(0),
            role: role.to_owned(),
            title: title.map(str::to_owned),
            value: value.map(str::to_owned),
            description: None,
            identifier: None,
            help: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_ptr: 7,
            depth: 0,
            parent_element_index: None,
            frame: None,
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: None,
            selected: None,
            in_web_content: false,
        }
    }

    fn tree_node(role: &str, title: Option<&str>, pointer: usize, depth: usize) -> AXNode {
        let mut node = node(role, title, None, &["AXPress"]);
        node.element_ptr = pointer;
        node.depth = depth;
        node
    }

    fn tree(nodes: Vec<AXNode>) -> TreeWalkResult {
        TreeWalkResult {
            tree_markdown: String::new(),
            nodes,
            truncated: false,
            window_scope: Some(crate::ax::WindowScope::Matched),
        }
    }

    #[test]
    fn checkbox_requires_exact_internal_page_proof() {
        let nodes = vec![
            node("AXWebArea", Some(chrome().page_titles[0]), None, &[]),
            node("AXHeading", Some(chrome().page_heading), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(chrome().setup_url),
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
        ];
        assert_eq!(
            exact_setup_checkbox(&tree(nodes.clone()), chrome()).unwrap(),
            Some(7)
        );

        let mut wrong_url = nodes.clone();
        wrong_url[2].value = Some("https://example.test/".to_owned());
        assert_eq!(
            exact_setup_checkbox(&tree(wrong_url), chrome()).unwrap(),
            None
        );
    }

    #[test]
    fn checkbox_matcher_refuses_ambiguity() {
        let nodes = vec![
            node("AXWebArea", Some(chrome().page_titles[0]), None, &[]),
            node("AXHeading", Some(chrome().page_heading), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(chrome().setup_url),
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
        ];
        assert_eq!(
            exact_setup_checkbox(&tree(nodes), chrome())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn pixel_fallback_requires_exact_url_and_selected_internal_tab() {
        let omnibox = node(
            "AXTextField",
            Some("Address and search bar"),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        let mut selected_tab = node(
            "AXRadioButton",
            Some(chrome().page_titles[0]),
            Some("1"),
            &["AXPress"],
        );
        selected_tab.selected = Some(true);
        assert!(native_setup_page_proven(
            &[omnibox.clone(), selected_tab.clone()],
            chrome()
        ));

        selected_tab.selected = Some(false);
        assert!(!native_setup_page_proven(
            &[omnibox.clone(), selected_tab],
            chrome()
        ));
        let mut wrong_url = omnibox;
        wrong_url.value = Some("https://example.test/".to_owned());
        assert!(!native_setup_page_proven(&[wrong_url], chrome()));

        let mut popup = node("AXWebArea", Some("Omnibox Popup"), None, &[]);
        popup.depth = 1;
        assert!(!native_setup_page_proven(
            &[
                node(
                    "AXTextField",
                    Some("Address and search bar"),
                    Some(chrome().setup_url),
                    &["AXPress"],
                ),
                {
                    let mut tab = node(
                        "AXRadioButton",
                        Some(chrome().page_titles[0]),
                        Some("1"),
                        &["AXPress"],
                    );
                    tab.selected = Some(true);
                    tab
                },
                popup,
            ],
            chrome()
        ));
    }

    #[test]
    fn pixel_fallback_requires_committed_navigation_and_complete_ax_proof() {
        let truncated = TreeWalkResult {
            tree_markdown: String::new(),
            nodes: Vec::new(),
            truncated: true,
            window_scope: Some(crate::ax::WindowScope::Matched),
        };
        assert!(
            exact_pixel_setup_checkbox(0, &truncated, 0, chrome(), false)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            exact_pixel_setup_checkbox(0, &truncated, 0, chrome(), true)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(
            exact_setup_checkbox(&truncated, chrome()).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn pixel_checkbox_detector_distinguishes_off_and_on() {
        let mut unchecked =
            image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for coordinate in 100..=112 {
            unchecked.put_pixel(coordinate, 80, image::Rgba([120, 120, 120, 255]));
            unchecked.put_pixel(coordinate, 92, image::Rgba([120, 120, 120, 255]));
            unchecked.put_pixel(100, coordinate - 20, image::Rgba([120, 120, 120, 255]));
            unchecked.put_pixel(112, coordinate - 20, image::Rgba([120, 120, 120, 255]));
        }
        assert_eq!(
            detect_checkbox_pixels(&unchecked, (50, 50, 200, 150), 1.0),
            vec![(106, 86, CheckboxState::Off)]
        );

        let mut checked = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for y in 80..=92 {
            for x in 100..=112 {
                checked.put_pixel(x, y, image::Rgba([26, 115, 232, 255]));
            }
        }
        checked.put_pixel(104, 86, image::Rgba([255, 255, 255, 255]));
        checked.put_pixel(105, 87, image::Rgba([255, 255, 255, 255]));
        checked.put_pixel(106, 86, image::Rgba([255, 255, 255, 255]));
        assert_eq!(
            detect_checkbox_pixels(&checked, (50, 50, 200, 150), 1.0),
            vec![(106, 86, CheckboxState::On)]
        );
    }

    #[test]
    fn pixel_checkbox_detector_refuses_non_square_noise() {
        let mut image = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for x in 80..=180 {
            image.put_pixel(x, 90, image::Rgba([120, 120, 120, 255]));
        }
        assert!(detect_checkbox_pixels(&image, (50, 50, 200, 150), 1.0).is_empty());
    }

    #[test]
    fn pixel_checkbox_detector_rejects_square_text_glyphs() {
        let mut image = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for x in 100..=112 {
            image.put_pixel(x, 80, image::Rgba([120, 120, 120, 255]));
            image.put_pixel(x, 92, image::Rgba([120, 120, 120, 255]));
            image.put_pixel(100, x - 20, image::Rgba([120, 120, 120, 255]));
            image.put_pixel(112, x - 20, image::Rgba([120, 120, 120, 255]));
        }

        // One connected square-ish text glyph has an empty center and enough
        // perimeter to pass the detector's pre-existing size, density, and
        // state filters, but it does not have four substantially occupied
        // straight edges.
        for (x, y) in [
            (144, 80),
            (145, 80),
            (146, 80),
            (142, 81),
            (143, 81),
            (141, 82),
            (140, 83),
            (140, 84),
            (140, 85),
            (140, 86),
            (140, 87),
            (141, 88),
            (142, 89),
            (143, 89),
            (144, 90),
            (145, 90),
            (146, 90),
            (147, 89),
            (148, 89),
            (149, 88),
            (150, 87),
            (150, 86),
            (150, 85),
            (150, 84),
            (150, 83),
            (149, 82),
            (148, 81),
            (147, 81),
        ] {
            image.put_pixel(x, y, image::Rgba([120, 120, 120, 255]));
        }

        assert!(detect_checkbox_pixels(&image, (130, 70, 170, 110), 1.0).is_empty());
        assert_eq!(
            detect_checkbox_pixels(&image, (50, 50, 200, 150), 1.0),
            vec![(106, 86, CheckboxState::Off)]
        );
    }

    #[test]
    fn pixel_checkbox_detector_accepts_rounded_antialiased_outlines() {
        fn rounded_outline(
            image: &mut image::RgbaImage,
            left: u32,
            top: u32,
            side: u32,
            radius: u32,
            border: image::Rgba<u8>,
            antialias: image::Rgba<u8>,
        ) {
            let right = left + side - 1;
            let bottom = top + side - 1;
            for offset in radius..side - radius {
                image.put_pixel(left + offset, top, border);
                image.put_pixel(left + offset, bottom, border);
                image.put_pixel(left, top + offset, border);
                image.put_pixel(right, top + offset, border);
            }
            for offset in 1..radius {
                image.put_pixel(left + radius - offset, top + offset, border);
                image.put_pixel(right - radius + offset, top + offset, border);
                image.put_pixel(left + radius - offset, bottom - offset, border);
                image.put_pixel(right - radius + offset, bottom - offset, border);
            }
            for (x, y) in [
                (left + radius - 1, top),
                (right - radius + 1, top),
                (left + radius - 1, bottom),
                (right - radius + 1, bottom),
            ] {
                image.put_pixel(x, y, antialias);
            }
        }

        let mut light = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        rounded_outline(
            &mut light,
            100,
            80,
            13,
            2,
            image::Rgba([120, 120, 120, 255]),
            image::Rgba([238, 238, 238, 255]),
        );
        assert_eq!(
            detect_checkbox_pixels(&light, (50, 50, 200, 150), 1.0),
            vec![(106, 86, CheckboxState::Off)]
        );

        let mut retina = image::RgbaImage::from_pixel(600, 400, image::Rgba([32, 33, 36, 255]));
        rounded_outline(
            &mut retina,
            100,
            120,
            26,
            4,
            image::Rgba([154, 160, 166, 255]),
            image::Rgba([55, 56, 59, 255]),
        );
        assert_eq!(
            detect_checkbox_pixels(&retina, (50, 80, 250, 220), 2.0),
            vec![(112, 132, CheckboxState::Off)]
        );
    }

    #[test]
    fn pixel_checkbox_detector_preserves_ambiguity() {
        let mut image = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for left in [80, 140] {
            for coordinate in 0..=12 {
                image.put_pixel(left + coordinate, 80, image::Rgba([120, 120, 120, 255]));
                image.put_pixel(left + coordinate, 92, image::Rgba([120, 120, 120, 255]));
                image.put_pixel(left, 80 + coordinate, image::Rgba([120, 120, 120, 255]));
                image.put_pixel(
                    left + 12,
                    80 + coordinate,
                    image::Rgba([120, 120, 120, 255]),
                );
            }
        }
        assert_eq!(
            detect_checkbox_pixels(&image, (50, 50, 200, 150), 1.0).len(),
            2
        );
    }

    #[test]
    fn pixel_checkbox_detector_supports_retina_dark_mode_and_focus_ring() {
        let mut retina = image::RgbaImage::from_pixel(600, 400, image::Rgba([32, 33, 36, 255]));
        for coordinate in 0..=25 {
            retina.put_pixel(100 + coordinate, 120, image::Rgba([154, 160, 166, 255]));
            retina.put_pixel(100 + coordinate, 145, image::Rgba([154, 160, 166, 255]));
            retina.put_pixel(100, 120 + coordinate, image::Rgba([154, 160, 166, 255]));
            retina.put_pixel(125, 120 + coordinate, image::Rgba([154, 160, 166, 255]));
        }
        assert_eq!(
            detect_checkbox_pixels(&retina, (50, 80, 250, 220), 2.0),
            vec![(112, 132, CheckboxState::Off)]
        );

        let mut focused = image::RgbaImage::from_pixel(400, 250, image::Rgba([255, 255, 255, 255]));
        for coordinate in 0..=16 {
            focused.put_pixel(98 + coordinate, 78, image::Rgba([120, 120, 120, 255]));
            focused.put_pixel(98 + coordinate, 94, image::Rgba([120, 120, 120, 255]));
            focused.put_pixel(98, 78 + coordinate, image::Rgba([120, 120, 120, 255]));
            focused.put_pixel(114, 78 + coordinate, image::Rgba([120, 120, 120, 255]));
        }
        assert_eq!(
            detect_checkbox_pixels(&focused, (50, 50, 200, 150), 1.0),
            vec![(106, 86, CheckboxState::Off)]
        );
    }

    #[test]
    fn pixel_checkbox_detector_enforces_bounded_zoom_geometry() {
        fn outlined_box(side: u32) -> image::RgbaImage {
            let mut image =
                image::RgbaImage::from_pixel(100, 100, image::Rgba([255, 255, 255, 255]));
            for coordinate in 0..side {
                image.put_pixel(20 + coordinate, 20, image::Rgba([120, 120, 120, 255]));
                image.put_pixel(
                    20 + coordinate,
                    20 + side - 1,
                    image::Rgba([120, 120, 120, 255]),
                );
                image.put_pixel(20, 20 + coordinate, image::Rgba([120, 120, 120, 255]));
                image.put_pixel(
                    20 + side - 1,
                    20 + coordinate,
                    image::Rgba([120, 120, 120, 255]),
                );
            }
            image
        }

        assert_eq!(
            detect_checkbox_pixels(&outlined_box(20), (0, 0, 100, 100), 1.0).len(),
            1
        );
        assert!(detect_checkbox_pixels(&outlined_box(6), (0, 0, 100, 100), 1.0).is_empty());
        assert!(detect_checkbox_pixels(&outlined_box(40), (0, 0, 100, 100), 1.0).is_empty());
    }

    #[test]
    fn setup_geometry_validates_scale_and_round_trips_retina_coordinates() {
        let frame = [-100.0, 50.0, 400.0, 250.0];
        let omnibox = [-90.0, 60.0, 380.0, 30.0];
        let geometry = setup_geometry(frame, omnibox, (800, 500)).unwrap();
        assert_eq!(geometry.scale_x, 2.0);
        assert_eq!(geometry.scale_y, 2.0);
        assert_eq!(geometry.search, (120, 140, 320, 380));
        assert_eq!(
            pixel_to_screen(frame, (212, 172), geometry),
            (6.0, 136.0, 106.0, 86.0)
        );

        assert!(setup_geometry(frame, omnibox, (800, 400)).is_err());
        assert!(setup_geometry([0.0, 0.0, 0.0, 250.0], omnibox, (800, 500)).is_err());
        assert!(setup_geometry(frame, omnibox, (200, 125)).is_ok());
        assert!(setup_geometry(frame, [-110.0, 60.0, 380.0, 30.0], (800, 500)).is_err());
        assert!(setup_geometry(frame, [-90.0, 290.0, 380.0, 30.0], (800, 500)).is_err());
    }

    #[test]
    fn unique_pixel_checkbox_preserves_ambiguity_and_control_identity() {
        let geometry = SetupGeometry {
            search: (0, 0, 400, 250),
            scale_x: 1.0,
            scale_y: 1.0,
        };
        assert!(
            unique_pixel_checkbox(&[], [0.0, 0.0, 400.0, 250.0], geometry, chrome())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            unique_pixel_checkbox(
                &[(106, 86, CheckboxState::Off), (166, 86, CheckboxState::On)],
                [0.0, 0.0, 400.0, 250.0],
                geometry,
                chrome(),
            )
            .unwrap_err()
            .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        let original = unique_pixel_checkbox(
            &[(106, 86, CheckboxState::Off)],
            [0.0, 0.0, 400.0, 250.0],
            geometry,
            chrome(),
        )
        .unwrap()
        .unwrap();
        let wrong_on = unique_pixel_checkbox(
            &[(130, 86, CheckboxState::On)],
            [0.0, 0.0, 400.0, 250.0],
            geometry,
            chrome(),
        )
        .unwrap()
        .unwrap();
        assert!(!original.same_control_as(wrong_on, 3.0));
    }

    #[test]
    fn pixel_control_correspondence_and_mutation_accounting_fail_closed() {
        let original = PixelCheckbox {
            screen_x: 106.0,
            screen_y: 136.0,
            window_local_x: 106.0,
            window_local_y: 86.0,
            window_frame: [0.0, 50.0, 400.0, 250.0],
            state: CheckboxState::Off,
        };
        let near = PixelCheckbox {
            window_local_x: 108.0,
            window_local_y: 84.0,
            ..original
        };
        let far = PixelCheckbox {
            window_local_x: 130.0,
            ..original
        };
        let resized = PixelCheckbox {
            window_frame: [0.0, 50.0, 430.0, 250.0],
            ..original
        };
        assert!(original.same_control_as(near, 3.0));
        assert!(!original.same_control_as(far, 3.0));
        assert!(!original.same_control_as(resized, 3.0));
        assert!(frames_agree(
            [0.0, 50.0, 400.0, 250.0],
            [0.5, 49.5, 400.5, 250.5],
            1.0
        ));
        assert!(!frames_agree(
            [0.0, 50.0, 400.0, 250.0],
            [0.0, 50.0, 405.0, 250.0],
            1.0
        ));
        assert!(remote_debugging_cleanup_required(false, true));
        assert!(!remote_debugging_cleanup_required(false, false));
    }

    #[test]
    fn new_tab_cleanup_selects_only_the_new_tabs_close_control() {
        let before = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXButton", Some("Close"), 11, 2),
        ];
        let after = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXButton", Some("Close"), 11, 2),
            tree_node("AXRadioButton", Some("New Tab"), 20, 1),
            tree_node("AXButton", Some("Close"), 21, 2),
        ];
        assert_eq!(
            select_new_tab_close_button(&before, &after, |left, right| left == right, chrome(),)
                .unwrap(),
            Some(21)
        );
    }

    #[test]
    fn new_tab_cleanup_refuses_multiple_new_tabs() {
        let before = vec![tree_node("AXRadioButton", Some("Original"), 10, 1)];
        let after = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXRadioButton", Some("New Tab"), 20, 1),
            tree_node("AXButton", Some("Close"), 21, 2),
            tree_node("AXRadioButton", Some("Another"), 30, 1),
            tree_node("AXButton", Some("Close"), 31, 2),
        ];
        assert_eq!(
            select_new_tab_close_button(&before, &after, |left, right| left == right, chrome(),)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn setup_navigation_selects_only_the_exact_omnibox_suggestion() {
        let omnibox = node(
            "AXTextField",
            Some("Address and search bar"),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        let mut popup = node("AXWebArea", Some("Omnibox Popup"), None, &[]);
        popup.depth = 1;
        let mut menu = node("AXMenu", None, None, &[]);
        menu.depth = 2;
        let mut exact = node(
            "AXMenuItem",
            Some(&format!(
                "{}, press Tab then Enter to Remove Suggestion.",
                chrome().setup_url
            )),
            None,
            &["AXPress"],
        );
        exact.element_ptr = 42;
        exact.depth = 3;
        let mut search = node(
            "AXMenuItem",
            Some("chrome://inspect/#remote-debugging search, Google Search"),
            None,
            &["AXPress"],
        );
        search.depth = 3;
        let mut outside = node(
            "AXMenuItem",
            Some(chrome().setup_url),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        outside.element_ptr = 99;
        outside.depth = 1;

        assert_eq!(
            exact_omnibox_suggestion(&[omnibox, popup, menu, exact, search, outside], chrome())
                .unwrap(),
            Some(42)
        );
    }

    #[test]
    fn setup_navigation_rejects_search_suggestion() {
        assert!(!is_exact_setup_suggestion(
            "chrome://inspect/#remote-debugging search, Google Search",
            chrome().setup_url
        ));
    }

    #[test]
    fn setup_navigation_refuses_multiple_exact_suggestions() {
        let omnibox = node(
            "AXTextField",
            Some("Address and search bar"),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        let mut popup = node("AXWebArea", Some("Omnibox Popup"), None, &[]);
        popup.depth = 1;
        let mut first = node("AXMenuItem", Some(chrome().setup_url), None, &["AXPress"]);
        first.depth = 2;
        let mut second = first.clone();
        second.element_ptr = 8;

        assert_eq!(
            exact_omnibox_suggestion(&[omnibox, popup, first, second], chrome())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn unknown_checkbox_values_refuse() {
        assert_eq!(checkbox_state(Some(0.0)).unwrap(), CheckboxState::Off);
        assert_eq!(checkbox_state(Some(1.0)).unwrap(), CheckboxState::On);
        assert_eq!(
            checkbox_state(None).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(
            checkbox_state(Some(0.5)).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }
}
