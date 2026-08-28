//! Platform-neutral geometry for the Agent View miniature desktop.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LayoutRect {
    #[cfg(test)]
    fn right(self) -> f64 {
        self.x + self.width
    }

    #[cfg(test)]
    fn bottom(self) -> f64 {
        self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetSize {
    pub width: u32,
    pub height: u32,
}

impl TargetSize {
    fn aspect(self) -> f64 {
        if self.width == 0 || self.height == 0 {
            return 16.0 / 10.0;
        }
        (self.width as f64 / self.height as f64).clamp(0.4, 2.8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetLayout {
    pub window: LayoutRect,
    pub content: LayoutRect,
}

/// Shell chrome idiom for the miniature desktop.
///
/// Only the dock band differs between styles. Target geometry is identical
/// under every style, so a session's cards never move when the host platform
/// swaps its shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellStyle {
    /// No permanent launcher or taskbar chrome.
    #[default]
    None,
    /// Centered dock floating clear of every edge (macOS and Linux).
    FloatingDock,
    /// Full-width bar flush with the bottom screen edge (Windows).
    EdgeTaskbar,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopLayout {
    pub desktop: LayoutRect,
    pub dock: LayoutRect,
    pub dock_icons: Vec<LayoutRect>,
    pub targets: Vec<TargetLayout>,
    pub shell: ShellStyle,
    /// Leading launcher slot. `EdgeTaskbar` only.
    pub start_button: Option<LayoutRect>,
    /// Trailing status band reserved for a clock and tray glyphs.
    /// `EdgeTaskbar` only.
    pub tray: Option<LayoutRect>,
    /// Running-app marks, one per dock icon in the same order.
    /// `EdgeTaskbar` only.
    pub indicators: Vec<LayoutRect>,
}

#[derive(Debug, Clone, PartialEq)]
struct ShellChrome {
    dock: LayoutRect,
    dock_icons: Vec<LayoutRect>,
    start_button: Option<LayoutRect>,
    tray: Option<LayoutRect>,
    indicators: Vec<LayoutRect>,
}

const DOCK_ICON_GAP: f64 = 8.0;

/// Lay out a miniature desktop with the floating dock shell.
///
/// Targets keep a stable row-major order. Columns receive widths based on the
/// average aspect ratio of their targets, which gives landscape windows more
/// room while keeping portrait and square windows compact. Narrow containers
/// collapse to one column instead of shrinking previews beyond usefulness.
pub fn layout_desktop(width: f64, height: f64, targets: &[TargetSize]) -> DesktopLayout {
    layout_desktop_with_shell(width, height, targets, ShellStyle::None)
}

/// Lay out a miniature desktop for one platform shell idiom.
///
/// The desktop band, and therefore every `TargetLayout`, is derived from the
/// dock's top edge alone. Both shells compute that edge identically, so the
/// shell only decides how the dock band itself is drawn.
pub fn layout_desktop_with_shell(
    width: f64,
    height: f64,
    targets: &[TargetSize],
    shell: ShellStyle,
) -> DesktopLayout {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let short_edge = width.min(height);
    // Keep target shadows and rounded corners visibly separated from the
    // miniature desktop frame, including at the minimum supported panel size.
    let outer_gap = (short_edge * 0.045).clamp(16.0, 26.0);
    let dock_height = (height * 0.12).clamp(38.0, 58.0).min(height * 0.22);
    let dock_bottom_margin = (height * 0.028).clamp(10.0, 16.0);
    let dock_y = if shell == ShellStyle::None {
        height - outer_gap
    } else {
        (height - dock_height - dock_bottom_margin).max(outer_gap)
    };
    let desktop_y = outer_gap;
    let desktop_height = (dock_y - outer_gap - desktop_y).max(1.0);
    let desktop = LayoutRect {
        x: outer_gap,
        y: desktop_y,
        width: (width - 2.0 * outer_gap).max(1.0),
        height: desktop_height,
    };

    let chrome = match shell {
        ShellStyle::None => ShellChrome {
            dock: LayoutRect {
                x: 0.0,
                y: height,
                width: 0.0,
                height: 0.0,
            },
            dock_icons: Vec::new(),
            start_button: None,
            tray: None,
            indicators: Vec::new(),
        },
        ShellStyle::FloatingDock => {
            floating_dock_chrome(width, outer_gap, dock_y, dock_height, targets.len())
        }
        ShellStyle::EdgeTaskbar => edge_taskbar_chrome(width, height, dock_y, targets.len()),
    };

    if targets.is_empty() {
        return DesktopLayout::assemble(desktop, shell, chrome, Vec::new());
    }

    let columns = if targets.len() == 1 || desktop.width < 430.0 {
        1
    } else if targets.len() <= 4 || desktop.width < 760.0 {
        2
    } else {
        3
    };
    let rows = targets.len().div_ceil(columns);
    let cell_gap = (short_edge * 0.014).clamp(5.0, 10.0);
    let usable_width = (desktop.width - cell_gap * (columns - 1) as f64).max(1.0);

    let mut column_weights = vec![0.0; columns];
    let mut column_counts = vec![0usize; columns];
    for (index, target) in targets.iter().enumerate() {
        let column = index % columns;
        column_weights[column] += target.aspect().clamp(0.65, 2.0);
        column_counts[column] += 1;
    }
    for column in 0..columns {
        column_weights[column] = if column_counts[column] == 0 {
            1.0
        } else {
            (column_weights[column] / column_counts[column] as f64).clamp(0.7, 1.8)
        };
    }
    let total_column_weight = column_weights.iter().sum::<f64>().max(1.0);
    let column_widths = column_weights
        .iter()
        .map(|weight| usable_width * weight / total_column_weight)
        .collect::<Vec<_>>();

    let mut row_weights = vec![1.0; rows];
    for (row, row_weight) in row_weights.iter_mut().enumerate() {
        let mut desired_height: f64 = 1.0;
        for (column, column_width) in column_widths.iter().enumerate() {
            let index = row * columns + column;
            let Some(target) = targets.get(index) else {
                continue;
            };
            desired_height = desired_height.max(column_width / target.aspect());
        }
        *row_weight = desired_height;
    }
    let usable_height = (desktop.height - cell_gap * (rows - 1) as f64).max(1.0);
    let total_row_weight = row_weights.iter().sum::<f64>().max(1.0);
    let row_heights = row_weights
        .iter()
        .map(|weight| usable_height * weight / total_row_weight)
        .collect::<Vec<_>>();

    let mut column_x = Vec::with_capacity(columns);
    let mut x = desktop.x;
    for column_width in &column_widths {
        column_x.push(x);
        x += column_width + cell_gap;
    }
    let mut row_y = Vec::with_capacity(rows);
    let mut y = desktop.y;
    for row_height in &row_heights {
        row_y.push(y);
        y += row_height + cell_gap;
    }

    let target_layouts = targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let column = index % columns;
            let row = index / columns;
            fit_target(
                LayoutRect {
                    x: column_x[column],
                    y: row_y[row],
                    width: column_widths[column],
                    height: row_heights[row],
                },
                target.aspect(),
            )
        })
        .collect::<Vec<_>>();

    DesktopLayout::assemble(desktop, shell, chrome, target_layouts)
}

impl DesktopLayout {
    fn assemble(
        desktop: LayoutRect,
        shell: ShellStyle,
        chrome: ShellChrome,
        targets: Vec<TargetLayout>,
    ) -> Self {
        Self {
            desktop,
            dock: chrome.dock,
            dock_icons: chrome.dock_icons,
            targets,
            shell,
            start_button: chrome.start_button,
            tray: chrome.tray,
            indicators: chrome.indicators,
        }
    }
}

fn floating_dock_chrome(
    width: f64,
    outer_gap: f64,
    dock_y: f64,
    dock_height: f64,
    target_count: usize,
) -> ShellChrome {
    let icon_count = target_count.max(1);
    let max_icon_width = ((width - 4.0 * outer_gap - DOCK_ICON_GAP * (icon_count - 1) as f64)
        / icon_count as f64)
        .max(16.0);
    let icon_size = (dock_height - 16.0).min(max_icon_width).clamp(20.0, 42.0);
    let dock_width =
        (icon_count as f64 * icon_size + (icon_count - 1) as f64 * DOCK_ICON_GAP + 28.0)
            .min(width - 2.0 * outer_gap)
            .max(icon_size + 20.0);
    let dock = LayoutRect {
        x: (width - dock_width) / 2.0,
        y: dock_y,
        width: dock_width,
        height: dock_height,
    };
    let icons_width = icon_count as f64 * icon_size + (icon_count - 1) as f64 * DOCK_ICON_GAP;
    let icon_x = dock.x + (dock.width - icons_width) / 2.0;
    let icon_y = dock.y + (dock.height - icon_size) / 2.0 - 1.0;
    let dock_icons = (0..target_count)
        .map(|index| LayoutRect {
            x: icon_x + index as f64 * (icon_size + DOCK_ICON_GAP),
            y: icon_y,
            width: icon_size,
            height: icon_size,
        })
        .collect::<Vec<_>>();
    ShellChrome {
        dock,
        dock_icons,
        start_button: None,
        tray: None,
        indicators: Vec::new(),
    }
}

/// Windows 11-style bar: full width, flush with the bottom edge, with the
/// launcher and app icons centered as one group and the tray trailing.
///
/// The bar takes its top edge from the shared `dock_y` and reaches the bottom
/// by absorbing the floating dock's bottom margin, so it gains height without
/// moving the desktop band above it.
fn edge_taskbar_chrome(width: f64, height: f64, dock_y: f64, target_count: usize) -> ShellChrome {
    let dock = LayoutRect {
        x: 0.0,
        y: dock_y,
        width,
        height: (height - dock_y).max(1.0),
    };
    let tray_width = (width * 0.16).clamp(56.0, 104.0).min(width * 0.34);
    let tray = LayoutRect {
        x: (width - tray_width).max(0.0),
        y: dock.y,
        width: tray_width,
        height: dock.height,
    };

    // Reserve the tray band on both sides so the icon group stays centered on
    // the whole bar rather than on the space left of the clock, and let icons
    // shrink instead of sliding under the tray on a very narrow panel.
    let slots = target_count + 1;
    let slot_span =
        (width - 2.0 * (tray_width + 6.0) - DOCK_ICON_GAP * (slots - 1) as f64) / slots as f64;
    let icon_size = (dock.height * 0.55).min(slot_span).clamp(8.0, 32.0);
    let group_width = slots as f64 * icon_size + (slots - 1) as f64 * DOCK_ICON_GAP;
    let group_x = ((width - group_width) / 2.0).max(4.0);
    // Sit the icons slightly high in the band to leave room for the marks.
    let icon_y = dock.y + (dock.height - icon_size) / 2.0 - 2.0;
    let slot_x = |index: usize| group_x + index as f64 * (icon_size + DOCK_ICON_GAP);

    let start_button = LayoutRect {
        x: slot_x(0),
        y: icon_y,
        width: icon_size,
        height: icon_size,
    };
    let dock_icons = (0..target_count)
        .map(|index| LayoutRect {
            x: slot_x(index + 1),
            y: icon_y,
            width: icon_size,
            height: icon_size,
        })
        .collect::<Vec<_>>();

    let indicator_height = (icon_size * 0.1).clamp(2.0, 3.0);
    let indicator_width = (icon_size * 0.42).max(4.0);
    let indicator_y = (icon_y + icon_size + 3.0).min(dock.y + dock.height - indicator_height - 1.0);
    let indicators = dock_icons
        .iter()
        .map(|icon| LayoutRect {
            x: icon.x + (icon.width - indicator_width) / 2.0,
            y: indicator_y,
            width: indicator_width,
            height: indicator_height,
        })
        .collect::<Vec<_>>();

    ShellChrome {
        dock,
        dock_icons,
        start_button: Some(start_button),
        tray: Some(tray),
        indicators,
    }
}

fn fit_target(cell: LayoutRect, aspect: f64) -> TargetLayout {
    let inset = 1.0;
    let available_width = (cell.width - 2.0 * inset).max(1.0);
    let available_content_height = (cell.height - 2.0 * inset).max(1.0);
    let content_width = available_width.min(available_content_height * aspect);
    let content_height = (content_width / aspect).min(available_content_height);
    let window_width = content_width;
    let window_height = content_height;
    let window = LayoutRect {
        x: cell.x + (cell.width - window_width) / 2.0,
        y: cell.y + (cell.height - window_height) / 2.0,
        width: window_width,
        height: window_height,
    };
    let content = LayoutRect {
        x: window.x,
        y: window.y,
        width: content_width,
        height: content_height,
    };
    TargetLayout { window, content }
}

pub fn png_dimensions(bytes: &[u8]) -> Option<TargetSize> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some(TargetSize { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> TargetSize {
        TargetSize { width, height }
    }

    fn intersects(a: LayoutRect, b: LayoutRect) -> bool {
        a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
    }

    #[test]
    fn mixed_form_factors_make_the_landscape_column_wider() {
        let layout = layout_desktop(
            720.0,
            520.0,
            &[
                size(1600, 900),
                size(700, 1200),
                size(1400, 900),
                size(900, 900),
            ],
        );
        assert_eq!(layout.targets.len(), 4);
        assert!(layout.targets[0].window.width > layout.targets[1].window.width);
        assert!(layout.targets[2].window.width > layout.targets[3].window.width);
    }

    #[test]
    fn narrow_desktop_reflows_to_one_column() {
        let layout = layout_desktop(380.0, 620.0, &[size(1600, 900), size(700, 1200)]);
        assert!(layout.targets[1].window.y > layout.targets[0].window.y);
        assert!((layout.targets[0].window.x - layout.targets[1].window.x).abs() < 80.0);
    }

    #[test]
    fn target_windows_stay_inside_the_desktop_and_do_not_overlap() {
        let layout = layout_desktop(
            840.0,
            560.0,
            &[
                size(1600, 900),
                size(700, 1200),
                size(1400, 900),
                size(900, 900),
                size(1000, 700),
                size(800, 1100),
            ],
        );
        for target in &layout.targets {
            assert!(target.window.x >= layout.desktop.x - 0.01);
            assert!(target.window.y >= layout.desktop.y - 0.01);
            assert!(target.window.right() <= layout.desktop.right() + 0.01);
            assert!(target.window.bottom() <= layout.desktop.bottom() + 0.01);
        }
        for (index, target) in layout.targets.iter().enumerate() {
            for other in layout.targets.iter().skip(index + 1) {
                assert!(!intersects(target.window, other.window));
            }
        }
    }

    #[test]
    fn content_preserves_the_source_aspect_ratio() {
        let targets = [size(1600, 900), size(700, 1200), size(900, 900)];
        let layout = layout_desktop(720.0, 520.0, &targets);
        for (target, source) in layout.targets.iter().zip(targets) {
            let rendered_aspect = target.content.width / target.content.height;
            assert!((rendered_aspect - source.aspect()).abs() < 0.001);
            assert_eq!(target.window, target.content);
        }
    }

    #[test]
    fn layout_reserves_padding_but_no_menu_or_target_title_bars() {
        let layout = layout_desktop(720.0, 520.0, &[size(1600, 900)]);
        assert!((16.0..=26.0).contains(&layout.desktop.y));
        assert_eq!(layout.targets[0].window, layout.targets[0].content);
    }

    #[test]
    fn minimum_panel_keeps_targets_away_from_the_frame() {
        let layout = layout_desktop(360.0, 260.0, &[size(900, 700)]);
        let target = layout.targets[0].window;
        assert!(target.x >= 16.0);
        assert!(target.y >= 16.0);
        assert!(target.right() <= 344.0);
    }

    #[test]
    fn dock_stays_below_targets_and_inside_the_panel() {
        let height = 520.0;
        let layout = layout_desktop_with_shell(
            720.0,
            height,
            &[size(1600, 900), size(700, 1200), size(900, 900)],
            ShellStyle::FloatingDock,
        );
        let lowest_target = layout
            .targets
            .iter()
            .map(|target| target.window.bottom())
            .fold(0.0, f64::max);
        assert!(layout.dock.y >= lowest_target);
        assert!(layout.dock.bottom() <= height - 10.0);
    }

    #[test]
    fn no_shell_is_the_default_and_carries_no_launcher_metadata() {
        let layout = layout_desktop(720.0, 520.0, &[size(1600, 900), size(900, 900)]);
        assert_eq!(layout.shell, ShellStyle::None);
        assert_eq!(layout.shell, ShellStyle::default());
        assert!(layout.start_button.is_none());
        assert!(layout.tray.is_none());
        assert!(layout.indicators.is_empty());
        assert_eq!(layout.dock.width, 0.0);
        assert!(layout.dock_icons.is_empty());
    }

    #[test]
    fn edge_taskbar_spans_the_full_width_and_reaches_the_bottom_edge() {
        let width = 720.0;
        let height = 520.0;
        let targets = [size(1600, 900), size(900, 900), size(700, 1200)];
        let floating = layout_desktop_with_shell(width, height, &targets, ShellStyle::FloatingDock);
        let taskbar = layout_desktop_with_shell(width, height, &targets, ShellStyle::EdgeTaskbar);

        assert_eq!(taskbar.shell, ShellStyle::EdgeTaskbar);
        assert_eq!(taskbar.dock.x, 0.0);
        assert_eq!(taskbar.dock.width, width);
        assert_eq!(taskbar.dock.bottom(), height);
        // The bar keeps the floating dock's top edge and only absorbs the
        // margin that used to sit below it.
        assert_eq!(taskbar.dock.y, floating.dock.y);
        assert!(taskbar.dock.height > floating.dock.height);
        assert!(taskbar.dock.height - floating.dock.height <= 16.0 + f64::EPSILON);
    }

    #[test]
    fn edge_taskbar_preserves_every_floating_dock_target_layout() {
        let targets = [
            size(1600, 900),
            size(700, 1200),
            size(1400, 900),
            size(900, 900),
            size(1000, 700),
        ];
        for (width, height) in [(720.0, 520.0), (380.0, 620.0), (360.0, 260.0)] {
            let floating =
                layout_desktop_with_shell(width, height, &targets, ShellStyle::FloatingDock);
            let taskbar =
                layout_desktop_with_shell(width, height, &targets, ShellStyle::EdgeTaskbar);
            assert_eq!(floating.desktop, taskbar.desktop);
            assert_eq!(floating.targets, taskbar.targets);
        }
    }

    #[test]
    fn edge_taskbar_centers_the_start_slot_with_the_app_icons() {
        let width = 720.0;
        let targets = [size(1600, 900), size(900, 900)];
        let layout = layout_desktop_with_shell(width, 520.0, &targets, ShellStyle::EdgeTaskbar);
        let start = layout.start_button.unwrap();
        let tray = layout.tray.unwrap();

        assert_eq!(layout.dock_icons.len(), targets.len());
        assert!(start.x < layout.dock_icons[0].x);
        assert_eq!(start.y, layout.dock_icons[0].y);
        assert_eq!(start.width, layout.dock_icons[0].width);

        let group_left = start.x;
        let group_right = layout.dock_icons.last().unwrap().right();
        assert!(((group_left + group_right) / 2.0 - width / 2.0).abs() < 1.0);
        assert!(group_right <= tray.x);
        assert_eq!(tray.right(), width);
        assert_eq!(tray.y, layout.dock.y);
        assert_eq!(tray.height, layout.dock.height);
    }

    #[test]
    fn edge_taskbar_marks_every_running_icon_inside_the_bar() {
        let targets = [size(1600, 900), size(900, 900), size(700, 1200)];
        let layout = layout_desktop_with_shell(720.0, 520.0, &targets, ShellStyle::EdgeTaskbar);
        assert_eq!(layout.indicators.len(), layout.dock_icons.len());
        for (indicator, icon) in layout.indicators.iter().zip(&layout.dock_icons) {
            assert!(indicator.width < icon.width);
            assert!(indicator.y >= icon.bottom());
            assert!(indicator.bottom() <= layout.dock.bottom());
            assert!(
                ((indicator.x + indicator.width / 2.0) - (icon.x + icon.width / 2.0)).abs() < 1.0
            );
        }
    }

    #[test]
    fn edge_taskbar_keeps_the_start_slot_when_no_target_has_a_card() {
        let layout = layout_desktop_with_shell(640.0, 420.0, &[], ShellStyle::EdgeTaskbar);
        assert!(layout.start_button.is_some());
        assert!(layout.tray.is_some());
        assert!(layout.dock_icons.is_empty());
        assert!(layout.indicators.is_empty());
        assert!(layout.targets.is_empty());
    }

    #[test]
    fn crowded_narrow_taskbar_shrinks_icons_instead_of_sliding_under_the_tray() {
        let targets = vec![size(1200, 800); 12];
        let layout = layout_desktop_with_shell(320.0, 260.0, &targets, ShellStyle::EdgeTaskbar);
        let tray = layout.tray.unwrap();
        assert!(layout.start_button.unwrap().x >= 0.0);
        assert!(layout.dock_icons.last().unwrap().right() <= tray.x);
        for icon in &layout.dock_icons {
            assert!(icon.width >= 8.0);
        }
    }

    #[test]
    fn reads_png_ihdr_dimensions_without_decoding_the_image() {
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&1440u32.to_be_bytes());
        png[20..24].copy_from_slice(&900u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some(size(1440, 900)));
        assert_eq!(png_dimensions(b"not png"), None);
    }
}
