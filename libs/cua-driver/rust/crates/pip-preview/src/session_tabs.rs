use crate::{LayoutRect, PipWorkspaceSummary};

const MAX_VISIBLE_TABS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionTab {
    pub workspace_id: String,
    pub label: String,
    pub rect: LayoutRect,
    pub selected: bool,
    pub accent: (u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionTabsLayout {
    pub tabs: Vec<SessionTab>,
    pub overflow: usize,
}

impl SessionTabsLayout {
    pub fn hit_test(&self, x: f64, y: f64) -> Option<&str> {
        self.tabs
            .iter()
            .find(|tab| {
                x >= tab.rect.x
                    && x <= tab.rect.x + tab.rect.width
                    && y >= tab.rect.y
                    && y <= tab.rect.y + tab.rect.height
            })
            .map(|tab| tab.workspace_id.as_str())
    }
}

pub fn layout_session_tabs(
    panel: LayoutRect,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) -> SessionTabsLayout {
    if workspaces.len() <= 1 {
        return SessionTabsLayout::default();
    }
    let visible = workspaces.len().min(MAX_VISIBLE_TABS);
    let gap = 6.0;
    let height = (panel.height * 0.09).clamp(28.0, 42.0);
    let width = ((panel.width - gap * (visible.saturating_sub(1)) as f64) / visible as f64)
        .clamp(90.0, 190.0);
    let tabs = workspaces
        .iter()
        .take(visible)
        .enumerate()
        .map(|(index, workspace)| SessionTab {
            workspace_id: workspace.workspace_id.clone(),
            label: workspace.workspace_label.clone(),
            rect: LayoutRect {
                x: panel.x + index as f64 * (width + gap),
                y: panel.y,
                width,
                height,
            },
            selected: selected_workspace_id == Some(workspace.workspace_id.as_str()),
            accent: session_accent(&workspace.workspace_id),
        })
        .collect();
    SessionTabsLayout {
        tabs,
        overflow: workspaces.len().saturating_sub(visible),
    }
}

pub fn session_accent(workspace_id: &str) -> (u8, u8, u8) {
    let hash = workspace_id
        .bytes()
        .fold(0u32, |hash, byte| hash.wrapping_mul(16777619) ^ byte as u32);
    const COLORS: [(u8, u8, u8); 6] = [
        (55, 148, 255),
        (255, 166, 54),
        (54, 190, 160),
        (228, 96, 135),
        (139, 112, 246),
        (66, 181, 225),
    ];
    COLORS[hash as usize % COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, updated_ms: u64) -> PipWorkspaceSummary {
        PipWorkspaceSummary {
            workspace_id: id.into(),
            workspace_label: id.into(),
            target_count: 1,
            updated_ms,
        }
    }

    #[test]
    fn hides_single_session_and_directly_hit_tests_tabs() {
        let panel = LayoutRect {
            x: 10.0,
            y: 12.0,
            width: 600.0,
            height: 400.0,
        };
        assert!(layout_session_tabs(panel, &[workspace("a", 1)], Some("a"))
            .tabs
            .is_empty());
        let layout = layout_session_tabs(panel, &[workspace("b", 2), workspace("a", 1)], Some("a"));
        assert_eq!(
            layout.hit_test(layout.tabs[0].rect.x + 2.0, layout.tabs[0].rect.y + 2.0),
            Some("b")
        );
        assert!(layout.tabs[1].selected);
    }

    #[test]
    fn caps_visible_tabs_and_reports_overflow() {
        let workspaces = (0..8)
            .map(|i| workspace(&format!("agent-{i}"), i))
            .collect::<Vec<_>>();
        let layout = layout_session_tabs(
            LayoutRect {
                x: 0.0,
                y: 0.0,
                width: 900.0,
                height: 500.0,
            },
            &workspaces,
            None,
        );
        assert_eq!(layout.tabs.len(), 6);
        assert_eq!(layout.overflow, 2);
        assert_eq!(session_accent("stable"), session_accent("stable"));
    }
}
