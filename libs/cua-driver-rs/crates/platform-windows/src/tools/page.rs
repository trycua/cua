//! Windows implementation of the cross-platform `PageBackend` trait.
//!
//! - `get_text` — find the Document UIA element under the active tab's web-
//!   content pane and read `TextPattern.DocumentRange.GetText(-1)`.  Falls
//!   back to the window-wide tree text if no Document control is present
//!   (e.g. Firefox in some configurations exposes the content under a
//!   different control type).
//! - `query_dom` — `FindAll(TreeScope_Subtree, condition)` rooted at the web
//!   Document.  Common CSS selectors map to UIA `ControlType` conditions
//!   (`a[href]` → `Hyperlink`, `button` → `Button`, `input[type=*]` →
//!   `Edit`, `h1`-`h6` → `Heading`, `li` → `ListItem`, etc.). `#id` /
//!   `.class` / `[role=*]` filter the matches after the fact by reading the
//!   element's `AutomationId` / IA2 attributes.  `[data-*]` is not
//!   reachable via UIA/IA2 — that path errors with a pointer at
//!   `execute_javascript`.
//! - `execute_javascript` — two-tier:
//!     1. **Bookmark-URL bypass** ([`super::page_bookmark::try_bookmark_exec`])
//!        — uses UIA `ValuePattern::SetValue` on the Edit-favorite dialog's
//!        URL field to install a `javascript:` bookmark, then UIA
//!        `InvokePattern::Invoke` on the favorites-bar item to run it.
//!        Result is read back from `document.title`.  Requires the
//!        favorites bar to be visible (sent Ctrl+Shift+B if not) and a
//!        pre-existing `cua-driver-eval` bookmark (creation flow is
//!        described in the docs; not yet automated end-to-end).  Zero
//!        config, no launch flag.
//!     2. **CDP fallback** — uses the shared `mcp_server::cdp` helper.
//!        Requires `--remote-debugging-port=N` and `CUA_DRIVER_CDP_PORT=N`.
//!        Returns an actionable error if neither path works.

use async_trait::async_trait;
use mcp_server::page::PageBackend;

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    UIA_ComboBoxControlTypeId,
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    TreeScope_Subtree, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_ControlTypePropertyId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_HeaderControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId, UIA_ListItemControlTypeId,
    UIA_NamePropertyId, UIA_PaneControlTypeId, UIA_TextControlTypeId, UIA_TextPatternId,
    UIA_ValueValuePropertyId,
};

pub struct WindowsPageBackend;

impl WindowsPageBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsPageBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PageBackend for WindowsPageBackend {
    async fn get_text(&self, _pid: i32, window_id: u32) -> anyhow::Result<String> {
        let hwnd = window_id as u64;
        tokio::task::spawn_blocking(move || unsafe { get_text_blocking(hwnd) })
            .await
            .map_err(|e| anyhow::anyhow!("join error: {e}"))?
    }

    async fn query_dom(
        &self,
        _pid: i32,
        window_id: u32,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<String> {
        let hwnd = window_id as u64;
        let selector = css_selector.to_owned();
        let attrs: Vec<String> = attributes.to_vec();
        tokio::task::spawn_blocking(move || unsafe { query_dom_blocking(hwnd, &selector, &attrs) })
            .await
            .map_err(|e| anyhow::anyhow!("join error: {e}"))?
    }

    async fn execute_javascript(
        &self,
        pid: i32,
        window_id: u32,
        javascript: &str,
    ) -> anyhow::Result<String> {
        // 1) Bookmark-based UIA exec — zero config, no launch flag needed.
        //    Requires that a `cua-driver-eval` bookmark exists in the
        //    Favorites bar (or that we can summon the bar and one already
        //    sits there).  Any failure (favorites bar hidden + Ctrl+Shift+B
        //    fails to summon, dialog drift, title-poll timeout) is logged
        //    and falls through to the CDP path.
        match super::page_bookmark::try_bookmark_exec(pid, window_id, javascript).await {
            Ok(v) => {
                return Ok(format!("uia.bookmark_exec: {v}"));
            }
            Err(e) => tracing::debug!(
                target: "page.execute_javascript",
                "bookmark exec path failed: {e:?}; falling back to CDP"
            ),
        }

        // 2) CDP fallback.  Discover CDP port from env.  Extracting
        //    `--remote-debugging-port` from the process's command line is
        //    PEB-reading territory; the conventional env var is good
        //    enough for now and fails with an actionable message if
        //    neither path applies.
        let port: u16 = match std::env::var("CUA_DRIVER_CDP_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
        {
            Some(p) => p,
            None => anyhow::bail!(
                "Chromium's `execute_javascript` on Windows tried the bookmark-URL UIA \
                 bypass and the CDP fallback. The bookmark path failed (see debug logs); \
                 for the CDP path, relaunch the browser with `--remote-debugging-port=N` \
                 and export CUA_DRIVER_CDP_PORT=N before starting cua-driver. \
                 Alternatively, create a bookmark named `cua-driver-eval` (any URL — the \
                 driver overwrites it on first use) in the Favorites bar to enable the \
                 bookmark-URL bypass.  Or use `get_text` / `query_dom` for read-only access."
            ),
        };
        let result = mcp_server::cdp::evaluate(port, javascript, true).await?;
        Ok(format!("cdp.runtime.evaluate.user_gesture: {result}"))
    }
}

// ── implementation ────────────────────────────────────────────────────────

unsafe fn get_text_blocking(hwnd: u64) -> anyhow::Result<String> {
    let (automation, root) = uia_root_for(hwnd)?;
    let document = find_document_descendant(&automation, &root)?;
    if let Some(doc) = document {
        text_pattern_dump(&doc)
    } else {
        // No Document control — try TextPattern on the root.
        match text_pattern_dump(&root) {
            Ok(s) => Ok(s),
            Err(_) => Ok(walk_text_blocking(&root)),
        }
    }
}

unsafe fn query_dom_blocking(
    hwnd: u64,
    selector: &str,
    attributes: &[String],
) -> anyhow::Result<String> {
    let (automation, root) = uia_root_for(hwnd)?;
    let root_for_search = find_document_descendant(&automation, &root)?.unwrap_or(root);

    let parsed = parse_selector(selector)?;

    if parsed.is_data_attr {
        anyhow::bail!(
            "`[data-*]` selectors are not reachable via UIA/IA2 on Windows. \
             Use `execute_javascript` for full DOM access (requires the browser launched \
             with `--remote-debugging-port`)."
        );
    }

    // Build a UIA condition for the ControlType (if any) and FindAll.
    let condition = match parsed.control_type {
        Some(ct) => automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &ct.0.into(),
        )?,
        None => automation.CreateTrueCondition()?,
    };

    let matches = root_for_search.FindAll(TreeScope_Subtree, &condition)?;
    let count = matches.Length().unwrap_or(0);

    let mut formatted: Vec<String> = Vec::new();
    for i in 0..count {
        let elem = match matches.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Post-filter by id/class. Skip elements that don't satisfy.
        if !parsed.id_filter.is_empty() {
            let aid = read_string(&elem, |e| {
                e.CurrentAutomationId().map(|b| b.to_string()).map_err(|e| e.into())
            });
            if aid.as_deref().unwrap_or_default() != parsed.id_filter {
                continue;
            }
        }

        let name = read_string(&elem, |e| {
            e.CurrentName().map(|b| b.to_string()).map_err(|e| e.into())
        })
        .unwrap_or_default();

        // attributes the caller asked for. We map a small set of well-known
        // CSS attribute names onto UIA properties.
        let mut attr_parts: Vec<String> = Vec::new();
        for a in attributes {
            let v = match a.as_str() {
                "name" => Some(name.clone()),
                "id" => read_string(&elem, |e| {
                    e.CurrentAutomationId().map(|b| b.to_string()).map_err(|e| e.into())
                }),
                "value" => read_value_value(&elem),
                "href" => {
                    // No native UIA "href" — closest is the AccessibleValue
                    // for hyperlinks (Chromium populates this with the URL).
                    read_value_value(&elem)
                }
                "textContent" | "text" => Some(name.clone()),
                _ => None,
            };
            if let Some(v) = v {
                attr_parts.push(format!("{a}=\"{}\"", escape_attr(&v)));
            }
        }

        let ct = read_control_type(&elem).unwrap_or_else(|| "Unknown".to_owned());
        let line = if attr_parts.is_empty() {
            format!("- {ct} \"{}\"", escape_attr(&name))
        } else {
            format!("- {ct} \"{}\" [{}]", escape_attr(&name), attr_parts.join(" "))
        };
        formatted.push(line);
    }

    if formatted.is_empty() {
        Ok("No elements found.".to_owned())
    } else {
        Ok(formatted.join("\n"))
    }
}

// ── UIA helpers ───────────────────────────────────────────────────────────

/// Initialise COM (best-effort) and resolve the UIA root element for `hwnd`.
unsafe fn uia_root_for(hwnd: u64) -> anyhow::Result<(IUIAutomation, IUIAutomationElement)> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| anyhow::anyhow!("UIA init failed: {e}"))?;
    let hwnd_win = HWND(hwnd as *mut _);
    let root = automation
        .ElementFromHandle(hwnd_win)
        .map_err(|e| anyhow::anyhow!("ElementFromHandle failed: {e}"))?;
    Ok((automation, root))
}

/// Find the first descendant Document control under `root`. The web view's
/// `Document` is the root of the rendered DOM in both Chromium and Firefox.
unsafe fn find_document_descendant(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> anyhow::Result<Option<IUIAutomationElement>> {
    let doc_id: i32 = UIA_DocumentControlTypeId.0;
    let cond = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &doc_id.into())?;
    match root.FindFirst(TreeScope_Subtree, &cond) {
        Ok(elem) => Ok(Some(elem)),
        Err(_) => Ok(None),
    }
}

/// Try `TextPattern.DocumentRange.GetText(-1)` on `elem`. Returns an error if
/// the element does not support TextPattern.
unsafe fn text_pattern_dump(elem: &IUIAutomationElement) -> anyhow::Result<String> {
    let pattern_raw = elem
        .GetCurrentPattern(UIA_TextPatternId)
        .map_err(|e| anyhow::anyhow!("element does not support TextPattern: {e}"))?;
    let text_pattern: IUIAutomationTextPattern = pattern_raw
        .cast()
        .map_err(|e| anyhow::anyhow!("cast to IUIAutomationTextPattern failed: {e}"))?;
    let range = text_pattern
        .DocumentRange()
        .map_err(|e| anyhow::anyhow!("DocumentRange failed: {e}"))?;
    let bstr = range
        .GetText(-1)
        .map_err(|e| anyhow::anyhow!("GetText(-1) failed: {e}"))?;
    Ok(bstr.to_string())
}

/// Last-resort: concatenate Name properties of every descendant. Used when
/// no Document control is present and the root has no TextPattern (very
/// rare for Chromium/Firefox).
unsafe fn walk_text_blocking(root: &IUIAutomationElement) -> String {
    let mut acc = String::new();
    if let Ok(name) = root.CurrentName() {
        let s = name.to_string();
        if !s.trim().is_empty() {
            acc.push_str(&s);
            acc.push('\n');
        }
    }
    if let Ok(children) = root.FindAll(
        TreeScope_Subtree,
        // Match every element with a non-empty name.
        &match create_true_condition_or_fallback() {
            Some(c) => c,
            None => return acc,
        },
    ) {
        let count = children.Length().unwrap_or(0);
        for i in 0..count {
            if let Ok(c) = children.GetElement(i) {
                if let Ok(name) = c.CurrentName() {
                    let s = name.to_string();
                    if !s.trim().is_empty() {
                        acc.push_str(&s);
                        acc.push('\n');
                    }
                }
            }
        }
    }
    acc
}

unsafe fn create_true_condition_or_fallback() -> Option<windows::Win32::UI::Accessibility::IUIAutomationCondition>
{
    let automation: IUIAutomation =
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    automation.CreateTrueCondition().ok()
}

unsafe fn read_string<F>(elem: &IUIAutomationElement, f: F) -> Option<String>
where
    F: FnOnce(&IUIAutomationElement) -> anyhow::Result<String>,
{
    let s = f(elem).ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

unsafe fn read_control_type(elem: &IUIAutomationElement) -> Option<String> {
    // Match against the raw i32 value — using the `UIA_*ControlTypeId`
    // constants directly in a match arm triggers `non_upper_case_globals`.
    let ct = elem.CurrentControlType().ok()?;
    let name = if ct.0 == UIA_ButtonControlTypeId.0 {
        "Button"
    } else if ct.0 == UIA_HyperlinkControlTypeId.0 {
        "Hyperlink"
    } else if ct.0 == UIA_EditControlTypeId.0 {
        "Edit"
    } else if ct.0 == UIA_ListItemControlTypeId.0 {
        "ListItem"
    } else if ct.0 == UIA_ImageControlTypeId.0 {
        "Image"
    } else if ct.0 == UIA_TextControlTypeId.0 {
        "Text"
    } else if ct.0 == UIA_DocumentControlTypeId.0 {
        "Document"
    } else if ct.0 == UIA_PaneControlTypeId.0 {
        "Pane"
    } else if ct.0 == UIA_HeaderControlTypeId.0 {
        "Header"
    } else {
        return Some(format!("ControlType({})", ct.0));
    };
    Some(name.to_owned())
}

unsafe fn read_value_value(elem: &IUIAutomationElement) -> Option<String> {
    let v = elem.GetCurrentPropertyValue(UIA_ValueValuePropertyId).ok()?;
    let raw = v.as_raw();
    if raw.Anonymous.Anonymous.vt != 8 {
        // VT_BSTR = 8 — anything else (empty, VT_EMPTY=0) means no value.
        return None;
    }
    // Read the BSTR pointer; we don't free it (windows-rs handles VARIANT
    // lifetime on drop of `v`).
    let bstr_ptr = raw.Anonymous.Anonymous.Anonymous.bstrVal;
    if bstr_ptr.is_null() {
        return None;
    }
    let bstr = windows::core::BSTR::from_raw(bstr_ptr);
    let s = bstr.to_string();
    // Don't double-free: hand the BSTR back to the VARIANT by leaking ours.
    std::mem::forget(bstr);
    Some(s)
}

// Avoid an unused-import warning when none of the secondary properties are
// actually read above.
#[allow(dead_code)]
const _UNUSED_NAME_PROP: UIA_PROPERTY_ID_T = UIA_NamePropertyId_VAL;

#[allow(non_camel_case_types)]
type UIA_PROPERTY_ID_T = windows::Win32::UI::Accessibility::UIA_PROPERTY_ID;
#[allow(non_upper_case_globals)]
const UIA_NamePropertyId_VAL: UIA_PROPERTY_ID_T = UIA_NamePropertyId;

// ── selector parsing ──────────────────────────────────────────────────────

struct ParsedSelector {
    control_type: Option<UIA_CONTROLTYPE_ID>,
    id_filter: String,
    is_data_attr: bool,
}

fn parse_selector(sel: &str) -> anyhow::Result<ParsedSelector> {
    let s = sel.trim();

    // Wildcard / empty → match-all, no post-filter.
    if s.is_empty() || s == "*" {
        return Ok(ParsedSelector {
            control_type: None,
            id_filter: String::new(),
            is_data_attr: false,
        });
    }

    // Reject `[data-*]` immediately — not reachable via UIA.
    if s.contains("[data-") {
        return Ok(ParsedSelector {
            control_type: None,
            id_filter: String::new(),
            is_data_attr: true,
        });
    }

    // Strip [attribute=value] suffix once we've consumed the tag.
    let (tag, rest) = match s.find('[') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };

    // ID suffix on tag (e.g. `div#foo`). `.class` is NOT supported — UIA's
    // `AutomationId` maps to HTML `id`, not to the class list. Supporting
    // class selectors would need an IA2 `attributes`-string parse (tracked
    // as a follow-up in the PR description).
    if tag.contains('.') {
        anyhow::bail!(
            "`.class` selectors are not supported by the Windows UIA backend. \
             Use a simple tag (a, button, input, textarea, h1-h6, img, li, p, \
             span, select), a `tag#id` selector, an `[attr=value]` selector \
             including `[role=…]`, or `execute_javascript` for full DOM access \
             (requires the browser launched with `--remote-debugging-port`)."
        );
    }
    let (tag_clean, id_filter) = match tag.find('#') {
        Some(i) => (&tag[..i], tag[i + 1..].to_owned()),
        None => (tag, String::new()),
    };

    let ct = control_type_for_tag(tag_clean);

    // For `[role=X]` selectors with no tag, try to map role → ControlType.
    let ct = if ct.is_none() && rest.starts_with("[role=") {
        let role = rest
            .trim_start_matches("[role=")
            .trim_end_matches(']')
            .trim_matches(|c: char| c == '"' || c == '\'');
        control_type_for_role(role)
    } else {
        ct
    };

    // After mapping, if we have neither a control_type nor an id_filter, the
    // selector wasn't understood — refuse so we don't dump the entire subtree
    // as a false-positive match.
    if ct.is_none() && id_filter.is_empty() && tag_clean != "*" && !tag_clean.is_empty() {
        anyhow::bail!(
            "Selector '{sel}' is not supported by the Windows UIA backend. \
             Use a simple tag (a, button, input, textarea, h1-h6, img, li, p, \
             span, select), a `tag#id` selector, `[role=…]`, or \
             `execute_javascript` for full DOM access (requires the browser \
             launched with `--remote-debugging-port`)."
        );
    }

    Ok(ParsedSelector {
        control_type: ct,
        id_filter,
        is_data_attr: false,
    })
}

fn control_type_for_tag(tag: &str) -> Option<UIA_CONTROLTYPE_ID> {
    Some(match tag.to_ascii_lowercase().as_str() {
        "a" => UIA_HyperlinkControlTypeId,
        "button" => UIA_ButtonControlTypeId,
        "input" | "textarea" => UIA_EditControlTypeId,
        "select" => UIA_ComboBoxControlTypeId,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => UIA_HeaderControlTypeId,
        "img" => UIA_ImageControlTypeId,
        "li" => UIA_ListItemControlTypeId,
        "p" | "span" => UIA_TextControlTypeId,
        "" | "*" => return None,
        _ => return None,
    })
}

fn control_type_for_role(role: &str) -> Option<UIA_CONTROLTYPE_ID> {
    Some(match role.to_ascii_lowercase().as_str() {
        "button" => UIA_ButtonControlTypeId,
        "link" => UIA_HyperlinkControlTypeId,
        "textbox" | "searchbox" => UIA_EditControlTypeId,
        "heading" => UIA_HeaderControlTypeId,
        "img" | "image" => UIA_ImageControlTypeId,
        "listitem" => UIA_ListItemControlTypeId,
        _ => return None,
    })
}

fn escape_attr(s: &str) -> String {
    s.replace('"', "\\\"")
}
