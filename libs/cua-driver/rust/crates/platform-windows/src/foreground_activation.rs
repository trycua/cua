/// Return whether the foreground window still belongs to the clicked target's
/// activation family.
///
/// A click on browser chrome can synchronously open an owned top-level surface
/// (for example Edge's quiet-permission panel). That surface has a different
/// `GA_ROOT`, but the same `GA_ROOTOWNER` as the browser frame. Treating only
/// exact root equality as success turns an externally delivered click into a
/// tool error. Root-owner equality keeps the allowance narrow: an unrelated
/// top-level window still fails the activation check.
pub(crate) fn matches_target_family(
    target_root: usize,
    target_root_owner: usize,
    foreground_root: usize,
    foreground_root_owner: usize,
) -> bool {
    target_root != 0
        && foreground_root != 0
        && (target_root == foreground_root
            || (target_root_owner != 0 && target_root_owner == foreground_root_owner))
}

#[cfg(test)]
mod tests {
    use super::matches_target_family;

    #[test]
    fn accepts_exact_target_root() {
        assert!(matches_target_family(0x10, 0x10, 0x10, 0x10));
    }

    #[test]
    fn accepts_browser_owned_transient_surface() {
        assert!(matches_target_family(0x10, 0x10, 0x20, 0x10));
    }

    #[test]
    fn rejects_unrelated_foreground_window() {
        assert!(!matches_target_family(0x10, 0x10, 0x20, 0x20));
    }

    #[test]
    fn rejects_missing_window_identity() {
        assert!(!matches_target_family(0, 0, 0x20, 0x20));
        assert!(!matches_target_family(0x10, 0x10, 0, 0));
    }
}
