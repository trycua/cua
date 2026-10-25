//! Known standalone product bundle identifiers.
//!
//! Recognition is diagnostic only: it does not establish code-signing identity,
//! permission attribution, or authorization. Release and local product state
//! and grants must still be matched against the exact claimed identity.

pub const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
pub const LOCAL_BUNDLE_ID: &str = "com.trycua.driver.local";

pub fn is_driver_bundle_id(bundle_id: &str) -> bool {
    matches!(bundle_id, RELEASE_BUNDLE_ID | LOCAL_BUNDLE_ID)
}
