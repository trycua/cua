use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::snapshot_store::{SnapshotPayload, SnapshotStore};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedIdentity {
    role: String,
    title: Option<String>,
    description: Option<String>,
    identifier: Option<String>,
}

impl ObservedIdentity {
    fn from_node(node: &AXNode) -> Self {
        Self {
            role: node.role.clone(),
            title: node.title.clone(),
            description: node.description.clone(),
            identifier: node.identifier.clone(),
        }
    }

    unsafe fn read(ptr: usize) -> Option<Self> {
        use super::bindings::copy_string_attr;
        let element = ptr as AXUIElementRef;
        Some(Self {
            role: copy_string_attr(element, "AXRole")?,
            title: normalized_label(copy_string_attr(element, "AXTitle")),
            description: normalized_label(copy_string_attr(element, "AXDescription")),
            identifier: copy_string_attr(element, "AXIdentifier"),
        })
    }
}

fn normalized_label(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

pub struct RetainedElement(usize, Option<ObservedIdentity>);

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.0
    }

    /// Recheck the identity that the caller observed, without treating mutable
    /// control values or layout as identity. The retained AX object alone does
    /// not prove that a button still represents the observed action.
    pub fn observed_identity_is_current(&self) -> bool {
        self.1.as_ref().is_some_and(|expected| {
            unsafe { ObservedIdentity::read(self.0) }.as_ref() == Some(expected)
        })
    }

    /// Take a +1 reference on an AX element pointer (0 is kept as null).
    ///
    /// # Safety
    ///
    /// A nonzero `ptr` must be a live `AXUIElementRef`.
    pub unsafe fn retain(ptr: usize) -> Self {
        if ptr != 0 {
            unsafe { CFRetain(ptr as AXUIElementRef as CFTypeRef) };
        }
        Self(ptr, None)
    }
}

impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        let mut retained = unsafe { Self::retain(self.0) };
        retained.1 = self.1.clone();
        retained
    }
}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as AXUIElementRef as CFTypeRef) };
        }
    }
}

pub struct AxSnapshot {
    pub elements: Vec<usize>,
    identities: Vec<ObservedIdentity>,
}

impl AxSnapshot {
    pub fn from_nodes(nodes: &[AXNode]) -> Self {
        Self {
            elements: nodes
                .iter()
                .filter(|node| node.element_index.is_some())
                .map(|node| node.element_ptr)
                .collect(),
            identities: nodes
                .iter()
                .filter(|node| node.element_index.is_some())
                .map(ObservedIdentity::from_node)
                .collect(),
        }
    }
}

impl SnapshotPayload for AxSnapshot {
    type Element = RetainedElement;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<RetainedElement> {
        self.elements.get(index).map(|ptr| {
            let mut element = unsafe { RetainedElement::retain(*ptr) };
            element.1 = self.identities.get(index).cloned();
            element
        })
    }
}

impl Drop for AxSnapshot {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}

pub type Snapshots = SnapshotStore<AxSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::{CFGetRetainCount, TCFType};
    use core_foundation::string::CFString;
    use cua_driver_core::element_token::{token_for, ResolvedElement};

    fn resolve(cache: &Snapshots, snapshot: u32, index: usize) -> Option<RetainedElement> {
        match cache
            .resolve(
                1,
                &serde_json::json!({ "element_token": token_for(snapshot, index) }),
            )
            .ok()?
        {
            ResolvedElement::Element { element, .. } => Some(element),
            _ => None,
        }
    }

    fn payload(ptr: usize) -> AxSnapshot {
        unsafe { CFRetain(ptr as CFTypeRef) };
        AxSnapshot {
            elements: vec![ptr],
            identities: vec![],
        }
    }

    #[test]
    fn fresh_labels_match_tree_normalization() {
        assert_eq!(normalized_label(None), None);
        assert_eq!(normalized_label(Some(String::new())), None);
        assert_eq!(normalized_label(Some("  ".into())), None);
        assert_eq!(
            normalized_label(Some("  Submit  ".into())),
            Some("Submit".into())
        );
    }

    #[test]
    fn retained_element_survives_concurrent_snapshot_replace() {
        let value = CFString::new("cua-driver-uaf-test-element-placeholder");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(ptr));
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        let guard = resolve(&cache, snapshot, 0).unwrap();
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 2);
        cache.publish(1, 2, AxSnapshot::from_nodes(&[]));
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        assert!(resolve(&cache, snapshot, 0).is_none());
        drop(guard);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base);
    }

    #[test]
    fn admitted_element_survives_cache_destruction_until_native_work_finishes() {
        let value = CFString::new("cua-driver-invariant-admitted-native-work");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(ptr));
        let guard = resolve(&cache, snapshot, 0).unwrap();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            finish_rx.recv().unwrap();
            assert_eq!(guard.as_ptr(), ptr);
            drop(guard);
        });
        drop(cache);
        let retained = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(retained, base + 1);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base);
    }

    #[test]
    fn missing_index_returns_none() {
        let cache = Snapshots::new();
        assert!(resolve(&cache, 0, 0).is_none());
        let snapshot = cache.publish(1, 2, AxSnapshot::from_nodes(&[]));
        assert!(resolve(&cache, snapshot, 0).is_none());
        assert!(resolve(&cache, snapshot, 5).is_none());
    }

    #[test]
    fn abandoned_preparation_releases_native_payload_without_replacing_snapshot() {
        let original = CFString::new("cua-driver-original-published-native-work");
        let replacement = CFString::new("cua-driver-abandoned-prepared-native-work");
        let original_ptr = original.as_concrete_TypeRef() as usize;
        let replacement_ptr = replacement.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(replacement_ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(original_ptr));
        let prepared = payload(replacement_ptr);
        assert_eq!(resolve(&cache, snapshot, 0).unwrap().as_ptr(), original_ptr);
        drop(prepared);
        assert_eq!(
            unsafe { CFGetRetainCount(replacement_ptr as CFTypeRef) },
            base
        );
        assert_eq!(resolve(&cache, snapshot, 0).unwrap().as_ptr(), original_ptr);
    }
}
