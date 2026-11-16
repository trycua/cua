// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AX guard: a small fingerprint per element at read time, compared with the
//! live element right before acting.

use super::reason::{Reason, Verdict};
use super::Rect;

/// What the agent saw. Captured from the walk that minted the token, so it
/// costs nothing extra at read time.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ElementFingerprint {
    pub role: String,
    pub subrole: Option<String>,
    /// Title, or description when the title is empty.
    pub label: Option<String>,
    pub identifier: Option<String>,
    pub frame: Option<Rect>,
    /// Index of the nearest actionable ancestor in the same snapshot.
    pub parent_index: Option<usize>,
    /// The parent's label, so two "Delete" buttons in different rows differ.
    pub parent_label: Option<String>,
}

/// The same attributes read from the live element at act time.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ElementObservation {
    /// The retained reference still answers (not `kAXErrorInvalidUIElement`).
    pub valid: bool,
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub label: Option<String>,
    pub identifier: Option<String>,
    pub frame: Option<Rect>,
    /// `None` when the parent was not read (cheap path).
    pub parent_label: Option<Option<String>>,
}

/// How the action reaches the element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delivery {
    /// AXPress / UIA Invoke / AT-SPI action on the reference: position does
    /// not matter, identity does.
    Semantic,
    /// A pointer event at a point: position matters too.
    Pixel,
}

/// The outcome of a hit-test at the click point (pixel delivery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitOutcome {
    /// The target or one of its descendants (within 5 ancestors).
    Target,
    Other,
    Nothing,
}

/// Frame tolerance: 2 pt, or 10% of the element's size, whichever is larger.
pub fn tolerance(extent: f64) -> f64 {
    (extent.abs() * 0.10).max(2.0)
}

fn norm(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|v| !v.is_empty())
}

pub fn verify_element(
    fp: &ElementFingerprint,
    now: &ElementObservation,
    delivery: Delivery,
    hit: Option<HitOutcome>,
) -> Verdict {
    if !now.valid {
        return Verdict::refuse(Reason::Gone, "the element reference is no longer valid");
    }
    if let Some(role) = now.role.as_deref() {
        if role != fp.role {
            return Verdict::refuse(
                Reason::RoleChanged,
                format!("role was {} and is now {role}", fp.role),
            );
        }
    }
    if fp.subrole.is_some() && now.subrole.is_some() && fp.subrole != now.subrole {
        return Verdict::refuse(Reason::RoleChanged, "subrole changed");
    }
    if let (Some(a), Some(b)) = (&fp.identifier, &now.identifier) {
        if a != b {
            return Verdict::refuse(
                Reason::Relabelled,
                format!("identifier was {a:?} and is now {b:?}"),
            );
        }
    }
    let (was, is) = (norm(&fp.label), norm(&now.label));
    if was != is {
        return Verdict::refuse(
            Reason::Relabelled,
            format!(
                "label was {:?} and is now {:?}",
                was.unwrap_or_default(),
                is.unwrap_or_default()
            ),
        );
    }
    if let Some(parent) = &now.parent_label {
        if norm(parent) != norm(&fp.parent_label) {
            return Verdict::refuse(
                Reason::Relabelled,
                "the element now sits in another container",
            );
        }
    }
    if delivery == Delivery::Semantic {
        return Verdict::Ok;
    }

    let (Some(old), Some(new)) = (fp.frame, now.frame) else {
        return Verdict::refuse(Reason::Offscreen, "the element has no frame now");
    };
    if new.w <= 0.0 || new.h <= 0.0 {
        return Verdict::refuse(Reason::Offscreen, "the element has an empty frame now");
    }
    if (new.w - old.w).abs() > tolerance(old.w) || (new.h - old.h).abs() > tolerance(old.h) {
        return Verdict::refuse(
            Reason::Resized,
            format!(
                "size was {:.0}x{:.0} and is now {:.0}x{:.0}",
                old.w, old.h, new.w, new.h
            ),
        );
    }
    let dx = new.x - old.x;
    let dy = new.y - old.y;
    if dx.abs() > tolerance(old.w) || dy.abs() > tolerance(old.h) {
        // Identity is proven above; only the position changed.
        return Verdict::Relocated { dx, dy };
    }
    match hit {
        Some(HitOutcome::Other) => {
            Verdict::refuse(Reason::HitMismatch, "another element is at the click point")
        }
        Some(HitOutcome::Nothing) => {
            Verdict::refuse(Reason::HitMismatch, "nothing is at the click point")
        }
        _ => Verdict::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> ElementFingerprint {
        ElementFingerprint {
            role: "AXButton".into(),
            label: Some("Save".into()),
            frame: Some(Rect::new(100.0, 100.0, 80.0, 24.0)),
            parent_label: Some("Toolbar".into()),
            ..Default::default()
        }
    }

    fn same() -> ElementObservation {
        ElementObservation {
            valid: true,
            role: Some("AXButton".into()),
            label: Some("Save".into()),
            frame: Some(Rect::new(100.0, 100.0, 80.0, 24.0)),
            ..Default::default()
        }
    }

    #[test]
    fn unchanged_element_passes_both_deliveries() {
        assert!(verify_element(&fp(), &same(), Delivery::Semantic, None).is_ok());
        assert!(verify_element(&fp(), &same(), Delivery::Pixel, Some(HitOutcome::Target)).is_ok());
    }

    #[test]
    fn each_reason_is_reported() {
        let mut o = same();
        o.valid = false;
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Semantic, None).reason(),
            Some(Reason::Gone)
        );
        let mut o = same();
        o.role = Some("AXCheckBox".into());
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Semantic, None).reason(),
            Some(Reason::RoleChanged)
        );
        let mut o = same();
        o.label = Some("Delete".into());
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Semantic, None).reason(),
            Some(Reason::Relabelled)
        );
        let mut o = same();
        o.frame = Some(Rect::new(100.0, 100.0, 160.0, 24.0));
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Pixel, None).reason(),
            Some(Reason::Resized)
        );
        let mut o = same();
        o.frame = None;
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Pixel, None).reason(),
            Some(Reason::Offscreen)
        );
        assert_eq!(
            verify_element(&fp(), &same(), Delivery::Pixel, Some(HitOutcome::Other)).reason(),
            Some(Reason::HitMismatch)
        );
    }

    #[test]
    fn whitespace_only_label_differences_are_ignored() {
        let mut o = same();
        o.label = Some("  Save ".into());
        assert!(verify_element(&fp(), &o, Delivery::Semantic, None).is_ok());
    }

    #[test]
    fn tolerance_edges() {
        // 10% of 80 = 8 pt horizontally, 2.4 pt vertically.
        let mut o = same();
        o.frame = Some(Rect::new(107.9, 102.3, 80.0, 24.0));
        assert!(verify_element(&fp(), &o, Delivery::Pixel, Some(HitOutcome::Target)).is_ok());
        o.frame = Some(Rect::new(108.1, 100.0, 80.0, 24.0));
        assert!(matches!(
            verify_element(&fp(), &o, Delivery::Pixel, None),
            Verdict::Relocated { .. }
        ));
        // Tiny elements still get 2 pt.
        assert_eq!(tolerance(5.0), 2.0);
    }

    #[test]
    fn semantic_delivery_ignores_moves() {
        let mut o = same();
        o.frame = Some(Rect::new(300.0, 100.0, 80.0, 24.0));
        assert!(verify_element(&fp(), &o, Delivery::Semantic, None).is_ok());
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Pixel, None),
            Verdict::Relocated { dx: 200.0, dy: 0.0 }
        );
    }

    #[test]
    fn identical_labels_are_told_apart_by_parent() {
        let mut o = same();
        o.parent_label = Some(Some("Row 4".into()));
        assert_eq!(
            verify_element(&fp(), &o, Delivery::Semantic, None).reason(),
            Some(Reason::Relabelled)
        );
        o.parent_label = Some(Some("Toolbar".into()));
        assert!(verify_element(&fp(), &o, Delivery::Semantic, None).is_ok());
    }

    #[test]
    fn identifier_change_is_relabelled_even_with_same_label() {
        let mut f = fp();
        f.identifier = Some("row-3".into());
        let mut o = same();
        o.identifier = Some("row-9".into());
        assert_eq!(
            verify_element(&f, &o, Delivery::Semantic, None).reason(),
            Some(Reason::Relabelled)
        );
    }
}
