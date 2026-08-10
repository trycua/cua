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

const EDGE: BrowserSetupDescriptor = BrowserSetupDescriptor {
    product: BrowserProduct::MicrosoftEdge,
    product_name: "Microsoft Edge",
    setup_url: "edge://inspect/#remote-debugging",
    page_titles: EDGE_TITLES,
    page_heading: "Remote debugging",
    checkbox_label: "Allow remote debugging for this browser instance",
    tab_close_labels: EDGE_TAB_CLOSE_LABELS,
};

pub fn existing_profile_setup_descriptor(
    product: BrowserProduct,
) -> Option<&'static BrowserSetupDescriptor> {
    match product {
        BrowserProduct::GoogleChrome => Some(&CHROME),
        BrowserProduct::Chromium => Some(&CHROMIUM),
        BrowserProduct::MicrosoftEdge => Some(&EDGE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_is_limited_to_products_with_exact_descriptors() {
        assert_eq!(
            existing_profile_setup_descriptor(BrowserProduct::GoogleChrome)
                .unwrap()
                .setup_url,
            "chrome://inspect/#remote-debugging"
        );
        assert_eq!(
            existing_profile_setup_descriptor(BrowserProduct::MicrosoftEdge)
                .unwrap()
                .setup_url,
            "edge://inspect/#remote-debugging"
        );
        assert_eq!(
            existing_profile_setup_descriptor(BrowserProduct::Chromium)
                .unwrap()
                .product,
            BrowserProduct::Chromium
        );
        for product in [
            BrowserProduct::Brave,
            BrowserProduct::Firefox,
            BrowserProduct::Safari,
            BrowserProduct::Electron,
            BrowserProduct::Other,
        ] {
            assert!(existing_profile_setup_descriptor(product).is_none());
        }
    }
}
