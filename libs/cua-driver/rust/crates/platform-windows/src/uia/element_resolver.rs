use super::UiaNode;
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
use std::sync::Arc;
use windows::core::{IUnknown, Interface};
use windows::Win32::UI::Accessibility::IUIAutomationElement;

#[derive(Debug)]
struct MtaUsage(usize);

impl MtaUsage {
    fn acquire() -> windows::core::Result<Arc<Self>> {
        use windows::Win32::System::Com::CoIncrementMTAUsage;
        let usage = Arc::new(Self(unsafe { CoIncrementMTAUsage()? }.0 as usize));
        Self::check_thread()?;
        Ok(usage)
    }

    fn check_thread() -> windows::core::Result<()> {
        use windows::Win32::System::Com::{
            CoGetApartmentType, APTTYPE, APTTYPEQUALIFIER, APTTYPE_MTA,
        };
        let (mut apartment, mut qualifier) = (APTTYPE::default(), APTTYPEQUALIFIER::default());
        unsafe {
            CoGetApartmentType(&mut apartment, &mut qualifier)?;
        }
        if apartment != APTTYPE_MTA {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::RPC_E_CHANGED_MODE,
            ));
        }
        Ok(())
    }
}

impl Drop for MtaUsage {
    fn drop(&mut self) {
        use windows::Win32::System::Com::{CoDecrementMTAUsage, CO_MTA_USAGE_COOKIE};
        let _ = unsafe { CoDecrementMTAUsage(CO_MTA_USAGE_COOKIE(self.0 as *mut _)) };
    }
}

#[derive(Debug)]
struct Binding {
    observed: UiaNode,
    target: ElementTarget,
}

#[derive(Debug)]
pub struct RetainedElement {
    ptr: usize,
    actions: Vec<String>,
    mta: Option<Arc<MtaUsage>>,
    scope: Option<(i32, u64)>,
    binding: Option<Arc<Binding>>,
    pub kind: ElementBackend,
    pub center: (i32, i32),
    pub rect: Option<(i32, i32, i32, i32)>,
    pub msaa_role: Option<i32>,
}

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.ptr
    }

    pub fn checked_ptr(&self) -> anyhow::Result<usize> {
        cua_driver_core::tool::check_native_dispatch()?;
        MtaUsage::check_thread()?;
        if self
            .scope
            .is_some_and(|(pid, window)| crate::win32::window_owner_pid(window) != Some(pid as u32))
        {
            anyhow::bail!("target window ownership changed before dispatch");
        }
        if self.ptr == 0 {
            anyhow::bail!("native element is unavailable");
        }
        if let (Some(binding), Some((_, window))) = (&self.binding, self.scope) {
            self.verify_live(window, binding)?;
        }
        Ok(self.ptr)
    }

    fn verify_live(&self, window: u64, binding: &Binding) -> anyhow::Result<()> {
        use windows::Win32::{System::Com::*, UI::Accessibility::*};
        unsafe {
            let element =
                std::mem::ManuallyDrop::new(IUIAutomationElement::from_raw(self.ptr as *mut _));
            if !element.CurrentIsEnabled()?.as_bool() {
                anyhow::bail!("target is disabled");
            }
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root =
                automation.ElementFromHandle(windows::Win32::Foundation::HWND(window as *mut _))?;
            let walker = automation.ControlViewWalker()?;
            let mut current = binding.observed.clone();
            let optional = |value: windows::core::BSTR| {
                let value = value.to_string();
                (!value.is_empty()).then_some(value)
            };
            current.control_type = super::control_type_name(element.CurrentControlType()?.0);
            current.name = optional(element.CurrentName()?);
            current.automation_id = optional(element.CurrentAutomationId()?);
            current.help_text = optional(element.CurrentHelpText()?);
            let mut complete = true;
            current.actions = super::detect_actions(&element, true, &mut complete, false);
            if !complete {
                anyhow::bail!("current native action availability is incomplete");
            }
            current.in_web_content = false;
            let mut ancestor = IUIAutomationElement::clone(&element);
            for depth in 0..element_token::MAX_NATIVE_ANCESTORS {
                if automation.CompareElements(&ancestor, &root)?.as_bool() {
                    current.depth = depth;
                    if binding
                        .target
                        .matches_identity(&identity_for_node(&current))
                    {
                        return Ok(());
                    }
                    anyhow::bail!("target description changed before dispatch");
                }
                ancestor = walker.GetParentElement(&ancestor)?;
                current.in_web_content |=
                    ancestor.CurrentControlType()? == UIA_DocumentControlTypeId;
            }
        }
        anyhow::bail!("target ancestry is incomplete or outside the requested window")
    }

    pub fn current_center(&self) -> anyhow::Result<(i32, i32)> {
        let pointer = self.checked_ptr()?;
        if !self.is_uia() {
            anyhow::bail!("MSAA geometry cannot be revalidated");
        }
        let element = std::mem::ManuallyDrop::new(unsafe {
            IUIAutomationElement::from_raw(pointer as *mut _)
        });
        let rect = unsafe { element.CurrentBoundingRectangle()? };
        if rect.right <= rect.left || rect.bottom <= rect.top {
            anyhow::bail!("target geometry is unavailable");
        }
        Ok((
            rect.left + (rect.right - rect.left) / 2,
            rect.top + (rect.bottom - rect.top) / 2,
        ))
    }

    pub fn is_uia(&self) -> bool {
        self.kind == ElementBackend::Uia
    }

    pub fn click(
        &self,
        window: u64,
    ) -> Option<(
        cua_driver_core::action_record::ActionTransport,
        windows::core::Result<()>,
    )> {
        use cua_driver_core::action_record::ActionTransport::*;
        use windows::Win32::UI::Accessibility::*;
        if !self.is_uia() {
            return None;
        }
        for (action, pattern_id, transport) in [
            ("invoke", UIA_InvokePatternId, WindowsUiaInvoke),
            ("toggle", UIA_TogglePatternId, WindowsUiaToggle),
            ("select", UIA_SelectionItemPatternId, WindowsUiaSelection),
            (
                "expand",
                UIA_ExpandCollapsePatternId,
                WindowsUiaExpandCollapse,
            ),
        ] {
            if !self.actions.iter().any(|name| name == action) {
                continue;
            }
            let result = (|| unsafe {
                if self.checked_ptr().is_err() {
                    return Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_ABORT,
                    ));
                }
                let element =
                    std::mem::ManuallyDrop::new(IUIAutomationElement::from_raw(self.ptr as *mut _));
                let pattern = element.GetCurrentPattern(pattern_id)?;
                super::fg_bypass::run_with_uwp_bypass(window as isize, || match action {
                    "invoke" => pattern.cast::<IUIAutomationInvokePattern>()?.Invoke(),
                    "toggle" => pattern.cast::<IUIAutomationTogglePattern>()?.Toggle(),
                    "select" => pattern
                        .cast::<IUIAutomationSelectionItemPattern>()?
                        .Select(),
                    "expand" => pattern
                        .cast::<IUIAutomationExpandCollapsePattern>()?
                        .Expand(),
                    _ => Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_NOTIMPL,
                    )),
                })
            })();
            return Some((transport, result));
        }
        None
    }

    pub fn focus_element(&self) -> anyhow::Result<()> {
        self.set_focus(None)
    }

    pub fn focus_background(&self, window: u64) -> anyhow::Result<()> {
        self.set_focus(Some(window))
    }

    fn set_focus(&self, window: Option<u64>) -> anyhow::Result<()> {
        if !self.is_uia() {
            anyhow::bail!("element is an MSAA element, not a UIA element");
        }
        let ptr = self.checked_ptr()?;
        let element = unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
        let action = || unsafe { element.SetFocus() };
        let result = match window {
            Some(window) => super::fg_bypass::run_with_uwp_bypass(window as isize, action),
            None => action(),
        };
        std::mem::forget(element);
        result.map_err(|e| anyhow::anyhow!("UIA SetFocus failed: {e}"))
    }

    pub fn element_has_keyboard_focus(&self) -> Option<bool> {
        if !self.is_uia() {
            return None;
        }
        let pointer = self.checked_ptr().ok()?;
        let element = unsafe { IUIAutomationElement::from_raw(pointer as *mut _) };
        let focused = unsafe { element.CurrentHasKeyboardFocus() }
            .ok()
            .map(|value| value.as_bool());
        std::mem::forget(element);
        focused
    }
}

impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        if self.ptr != 0 {
            unsafe {
                let iface = std::mem::ManuallyDrop::new(IUnknown::from_raw(self.ptr as *mut _));
                std::mem::forget(IUnknown::clone(&iface));
            }
        }
        Self {
            ptr: self.ptr,
            actions: self.actions.clone(),
            mta: self.mta.clone(),
            scope: self.scope,
            binding: self.binding.clone(),
            kind: self.kind,
            center: self.center,
            rect: self.rect,
            msaa_role: self.msaa_role,
        }
    }
}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.ptr != 0 {
            unsafe {
                drop(IUnknown::from_raw(self.ptr as *mut _));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementBackend {
    Uia,
    Msaa,
}

pub struct FreshUiaElements {
    elements: Vec<(Option<usize>, RetainedElement)>,
}

impl FreshUiaElements {
    pub fn from_nodes(nodes: &[UiaNode], kind: ElementBackend) -> Self {
        Self {
            elements: nodes
                .iter()
                .map(|node| {
                    (
                        node.element_index,
                        RetainedElement {
                            ptr: node.element_ptr,
                            actions: node.actions.clone(),
                            mta: None,
                            scope: None,
                            binding: None,
                            kind,
                            center: (node.center_x, node.center_y),
                            rect: node.rect,
                            msaa_role: node.msaa_role,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl FreshUiaElements {
    fn retain_element(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .iter()
            .find(|(observed, element)| *observed == Some(index) && element.ptr != 0)
            .map(|(_, element)| element.clone())
    }
}
pub fn identity_for_node(node: &UiaNode) -> Vec<u8> {
    serde_json::to_vec(&(
        &node.control_type,
        &node.name,
        &node.automation_id,
        &node.help_text,
        &node.actions,
        node.depth,
        node.in_web_content,
        node.msaa_role,
    ))
    .expect("UIA identity tuple")
}
pub async fn resolve_element_args(
    pid: i32,
    args: &serde_json::Value,
    tool: &str,
) -> Result<ResolvedElement<RetainedElement>, ToolResult> {
    element_token::resolve_native(pid, args, tool, move |w, t| resolve_fresh(pid, w, t)).await
}
pub(crate) fn resolve_fresh(
    pid: i32,
    w: u64,
    t: &ElementTarget,
) -> Result<Option<RetainedElement>, String> {
    if crate::win32::window_owner_pid(w) != Some(pid as u32) {
        return Err("target window ownership changed".into());
    }
    let mta =
        MtaUsage::acquire().map_err(|error| format!("UIA requires an MTA worker: {error}"))?;
    let tree = super::walk_tree(w, None);
    let kind = if tree.nodes.iter().any(|n| n.msaa_role.is_some()) {
        ElementBackend::Msaa
    } else {
        ElementBackend::Uia
    };
    let payload = FreshUiaElements::from_nodes(&tree.nodes, kind);
    if kind == ElementBackend::Msaa {
        return Err(
            "MSAA cannot prove retained child identity; semantic targeting is unsupported".into(),
        );
    }
    let matched = t.resolve_unique(
        tree.nodes
            .iter()
            .filter_map(|n| Some((n.element_index?, identity_for_node(n)))),
        tree.complete,
    )?;
    let mut retained = matched.and_then(|i| payload.retain_element(i));
    if let Some(element) = retained.as_mut() {
        element.mta = Some(mta);
        element.scope = Some((pid, w));
        let observed = tree
            .nodes
            .iter()
            .find(|node| node.element_index == matched)
            .ok_or("resolved native node is unavailable")?;
        element.binding = Some(Arc::new(Binding {
            observed: observed.clone(),
            target: t.clone(),
        }));
        element.checked_ptr().map_err(|error| error.to_string())?;
    }
    Ok(retained)
}
