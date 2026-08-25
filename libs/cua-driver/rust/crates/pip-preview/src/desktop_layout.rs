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
    pub title_bar_height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopLayout {
    pub menu_bar: LayoutRect,
    pub desktop: LayoutRect,
    pub dock: LayoutRect,
    pub dock_icons: Vec<LayoutRect>,
    pub targets: Vec<TargetLayout>,
}

/// Lay out a miniature desktop in top-left-origin coordinates.
///
/// Targets keep a stable row-major order. Columns receive widths based on the
/// average aspect ratio of their targets, which gives landscape windows more
/// room while keeping portrait and square windows compact. Narrow containers
/// collapse to one column instead of shrinking previews beyond usefulness.
pub fn layout_desktop(width: f64, height: f64, targets: &[TargetSize]) -> DesktopLayout {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let short_edge = width.min(height);
    let outer_gap = (short_edge * 0.018).clamp(5.0, 11.0);
    let menu_height = (height * 0.052).clamp(18.0, 26.0).min(height * 0.16);
    let dock_height = (height * 0.13).clamp(36.0, 60.0).min(height * 0.24);
    let dock_y = (height - dock_height - outer_gap).max(menu_height + outer_gap);
    let desktop_y = menu_height + outer_gap;
    let desktop_height = (dock_y - outer_gap - desktop_y).max(1.0);
    let desktop = LayoutRect {
        x: outer_gap,
        y: desktop_y,
        width: (width - 2.0 * outer_gap).max(1.0),
        height: desktop_height,
    };

    let dock_gap = 6.0;
    let icon_count = targets.len().max(1);
    let max_icon_width = ((width - 4.0 * outer_gap - dock_gap * (icon_count - 1) as f64)
        / icon_count as f64)
        .max(16.0);
    let icon_size = (dock_height - 12.0).min(max_icon_width).clamp(18.0, 46.0);
    let dock_width = (icon_count as f64 * icon_size + (icon_count - 1) as f64 * dock_gap + 18.0)
        .min(width - 2.0 * outer_gap)
        .max(icon_size + 18.0);
    let dock = LayoutRect {
        x: (width - dock_width) / 2.0,
        y: dock_y,
        width: dock_width,
        height: dock_height,
    };
    let icons_width = icon_count as f64 * icon_size + (icon_count - 1) as f64 * dock_gap;
    let icon_x = dock.x + (dock.width - icons_width) / 2.0;
    let icon_y = dock.y + (dock.height - icon_size) / 2.0 - 1.0;
    let dock_icons = (0..targets.len())
        .map(|index| LayoutRect {
            x: icon_x + index as f64 * (icon_size + dock_gap),
            y: icon_y,
            width: icon_size,
            height: icon_size,
        })
        .collect::<Vec<_>>();

    if targets.is_empty() {
        return DesktopLayout {
            menu_bar: LayoutRect {
                x: 0.0,
                y: 0.0,
                width,
                height: menu_height,
            },
            desktop,
            dock,
            dock_icons,
            targets: Vec::new(),
        };
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
            desired_height = desired_height.max(column_width / target.aspect() + 22.0);
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

    DesktopLayout {
        menu_bar: LayoutRect {
            x: 0.0,
            y: 0.0,
            width,
            height: menu_height,
        },
        desktop,
        dock,
        dock_icons,
        targets: target_layouts,
    }
}

fn fit_target(cell: LayoutRect, aspect: f64) -> TargetLayout {
    let inset = 1.0;
    let title_bar_height = (cell.height * 0.1)
        .clamp(17.0, 23.0)
        .min(cell.height * 0.28);
    let available_width = (cell.width - 2.0 * inset).max(1.0);
    let available_content_height = (cell.height - title_bar_height - 2.0 * inset).max(1.0);
    let content_width = available_width.min(available_content_height * aspect);
    let content_height = (content_width / aspect).min(available_content_height);
    let window_width = content_width;
    let window_height = content_height + title_bar_height;
    let window = LayoutRect {
        x: cell.x + (cell.width - window_width) / 2.0,
        y: cell.y + (cell.height - window_height) / 2.0,
        width: window_width,
        height: window_height,
    };
    let content = LayoutRect {
        x: window.x,
        y: window.y + title_bar_height,
        width: content_width,
        height: content_height,
    };
    TargetLayout {
        window,
        content,
        title_bar_height,
    }
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
