// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Exact, product-specific anchors for approved existing-profile setup.

use std::time::Duration;

use super::types::BrowserProduct;

/// Cold browser profiles can spend several seconds initializing their internal
/// DevTools page. Keep one bounded timeout across platform adapters so a slow
/// first render does not become platform-specific behavior.
pub const EXISTING_PROFILE_SETUP_READY_TIMEOUT: Duration = Duration::from_secs(12);

/// Accessibility-visible anchors for a Chromium product's internal remote
/// debugging page. Adapters still require an exact native window and exact
/// control matches; these values only describe the product surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserSetupDescriptor {
    pub product: BrowserProduct,
    pub product_name: &'static str,
    pub setup_url: &'static str,
    pub page_titles: &'static [&'static str],
    pub page_heading: &'static str,
    pub checkbox_label: &'static str,
    pub tab_close_labels: &'static [&'static str],
}

const CHROME_TITLES: &[&str] = &["Inspect with Chrome Developer Tools"];
const CHROMIUM_TITLES: &[&str] = &[
    "Inspect with Chromium Developer Tools",
    "Inspect with Chrome Developer Tools",
];
const EDGE_TITLES: &[&str] = &[
    "Inspect with Edge Developer Tools",
    "Inspect with Microsoft Edge DevTools",
    "Inspect with Edge DevTools",
];
const CHROME_TAB_CLOSE_LABELS: &[&str] = &["Close"];
const EDGE_TAB_CLOSE_LABELS: &[&str] = &["Close tab"];

const CHROME: BrowserSetupDescriptor = BrowserSetupDescriptor {
    product: BrowserProduct::GoogleChrome,
    product_name: "Google Chrome",
    setup_url: "chrome://inspect/#remote-debugging",
    page_titles: CHROME_TITLES,
    page_heading: "Remote debugging",
    checkbox_label: "Allow remote debugging for this browser instance",
    tab_close_labels: CHROME_TAB_CLOSE_LABELS,
};

const CHROMIUM: BrowserSetupDescriptor = BrowserSetupDescriptor {
    product: BrowserProduct::Chromium,
    product_name: "Chromium",
    setup_url: "chrome://inspect/#remote-debugging",
    page_titles: CHROMIUM_TITLES,
    page_heading: "Remote debugging",
    checkbox_label: "Allow remote debugging for this browser instance",
    tab_close_labels: CHROME_TAB_CLOSE_LABELS,
};

// Brave ships Chromium's inspect page unchanged: same title, heading, checkbox
// and tab close label (read from Brave 1.99 on Linux). Only the scheme differs.
const BRAVE: BrowserSetupDescriptor = BrowserSetupDescriptor {
    product: BrowserProduct::Brave,
    product_name: "Brave",
    setup_url: "brave://inspect/#remote-debugging",
    page_titles: CHROME_TITLES,
    page_heading: "Remote debugging",
    checkbox_label: "Allow remote debugging for this browser instance",
    tab_close_labels: CHROME_TAB_CLOSE_LABELS,
};

const EDGE: BrowserSetupDescriptor = BrowserSetupDescriptor {
    product: BrowserProduct::MicrosoftEdge,
    product_name: "Microsoft Edge",
    setup_url: "edge://inspect/#remote-debugging",
    page_titles: EDGE_TITLES,
    page_heading: "Remote debugging",
    checkbox_label: "Allow remote debugging for this browser instance",
    tab_close_labels: EDGE_TAB_CLOSE_LABELS,
};

/// The desktop surface an adapter runs existing-profile setup on. A product's
/// descriptor resolves only on the surfaces its setup page was verified on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPlatform {
    MacOS,
    Windows,
    LinuxX11,
    LinuxWayland,
}

pub fn existing_profile_setup_descriptor(
    product: BrowserProduct,
    platform: SetupPlatform,
) -> Option<&'static BrowserSetupDescriptor> {
    match product {
        BrowserProduct::GoogleChrome => Some(&CHROME),
        BrowserProduct::Chromium => Some(&CHROMIUM),
        BrowserProduct::MicrosoftEdge => Some(&EDGE),
        // Brave setup is verified on Linux X11 only.
        BrowserProduct::Brave if platform == SetupPlatform::LinuxX11 => Some(&BRAVE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORMS: [SetupPlatform; 4] = [
        SetupPlatform::MacOS,
        SetupPlatform::Windows,
        SetupPlatform::LinuxX11,
        SetupPlatform::LinuxWayland,
    ];

    #[test]
    fn setup_is_limited_to_products_with_exact_descriptors() {
        for platform in PLATFORMS {
            assert_eq!(
                existing_profile_setup_descriptor(BrowserProduct::GoogleChrome, platform)
                    .unwrap()
                    .setup_url,
                "chrome://inspect/#remote-debugging"
            );
            assert_eq!(
                existing_profile_setup_descriptor(BrowserProduct::MicrosoftEdge, platform)
                    .unwrap()
                    .setup_url,
                "edge://inspect/#remote-debugging"
            );
            assert_eq!(
                existing_profile_setup_descriptor(BrowserProduct::Chromium, platform)
                    .unwrap()
                    .product,
                BrowserProduct::Chromium
            );
            for product in [
                BrowserProduct::Firefox,
                BrowserProduct::Safari,
                BrowserProduct::Electron,
                BrowserProduct::Other,
            ] {
                assert!(existing_profile_setup_descriptor(product, platform).is_none());
            }
        }
    }

    #[test]
    fn brave_setup_resolves_only_on_linux_x11() {
        for platform in PLATFORMS {
            let descriptor = existing_profile_setup_descriptor(BrowserProduct::Brave, platform);
            if platform == SetupPlatform::LinuxX11 {
                assert_eq!(
                    descriptor.unwrap().setup_url,
                    "brave://inspect/#remote-debugging"
                );
            } else {
                assert!(descriptor.is_none(), "{platform:?}");
            }
        }
    }
}
