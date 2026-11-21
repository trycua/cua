//! How a computer's presence reads across the SDK and the apps.
//!
//! The relay is the one source of a computer's presence: it lists a
//! computer online while that computer's connection to it is up (and its
//! heartbeat answers), and offline the moment it goes. The SDK
//! (`cua-spaces`) follows it: a computer the relay lists offline is reached
//! only through a connection that answers now, never through one cached
//! from before. When it cannot be reached, the reason starts with
//! [`LISTED_OFFLINE`], and the apps' shared core (`cua-spaces-app-core`)
//! reads that reason as "offline" even when the app's own, older read of
//! the relay still says online.

/// Starts the reason a computer the relay lists offline could not be
/// reached (the SDK's connect error, and the row error the apps show).
pub const LISTED_OFFLINE: &str = "the relay lists this computer as offline";

/// Whether `error` (a connect error or a Space row's error) says the relay
/// lists the computer offline.
pub fn says_listed_offline(error: &str) -> bool {
    error.contains(LISTED_OFFLINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_is_found_inside_a_longer_error() {
        let e = format!("relay:abc: {LISTED_OFFLINE} (not retried for 20 s)");
        assert!(says_listed_offline(&e));
        assert!(!says_listed_offline("timed out"));
    }
}
