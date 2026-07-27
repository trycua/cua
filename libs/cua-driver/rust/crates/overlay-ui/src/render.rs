use std::sync::{Arc, OnceLock};

use tiny_skia::{Pixmap, Transform};

use crate::{sanitize_label, sanitize_summary, ConsentCard, IndicatorCard, Rect, Size};

pub const CONSENT_SIZE: Size = Size {
    width: 424.0,
    height: 274.0,
};
pub const INDICATOR_SIZE: Size = Size {
    width: 380.0,
    height: 76.0,
};

pub const DECLINE_RECT: Rect = Rect {
    x: 32.0,
    y: 202.0,
    width: 172.0,
    height: 44.0,
};
pub const ACCEPT_RECT: Rect = Rect {
    x: 220.0,
    y: 202.0,
    width: 172.0,
    height: 44.0,
};
pub const STOP_RECT: Rect = Rect {
    x: 296.0,
    y: 14.0,
    width: 64.0,
    height: 44.0,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ConsentVisualState {
    pub accept_armed: bool,
    pub accept_hovered: bool,
    pub decline_hovered: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid render scale")]
    InvalidScale,
    #[error("could not allocate overlay pixmap")]
    Allocation,
    #[error("could not parse generated overlay SVG: {0}")]
    Svg(String),
}

pub fn render_consent(
    card: &ConsentCard,
    scale: f32,
    state: ConsentVisualState,
) -> Result<Pixmap, RenderError> {
    let title = operation_title(&sanitize_label(&card.operation));
    let summary = sanitize_summary(&card.summary);
    let risk_label = sanitize_label(&card.risk_label);
    let lines = wrap_text(&summary, 50, 2);
    let accept_fill = if state.accept_armed {
        if state.accept_hovered {
            "#2A2A29"
        } else {
            "#111110"
        }
    } else {
        "#D9D9D5"
    };
    let accept_text = if state.accept_armed {
        "#FFFFFF"
    } else {
        "#969691"
    };
    let decline_fill = if state.decline_hovered {
        "#E7E7E3"
    } else {
        "#F5F5F2"
    };
    let summary_svg = text_lines_svg(&lines, 112.0, 14.0, 19.0, "#5F5F5A");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="424" height="274" viewBox="0 0 424 274">
<rect x="10" y="12" width="404" height="252" rx="20" fill="#171716" opacity=".13"/>
<rect x="6" y="6" width="412" height="256" rx="20" fill="#FBFBF9" stroke="#D5D5D1" stroke-width="1"/>
<circle cx="34" cy="32" r="10" fill="#1C1C1B"/>
<path d="M30 32h8M34 28v8" fill="none" stroke="#FFFFFF" stroke-width="1.5" stroke-linecap="round"/>
<text x="52" y="36" fill="#767670" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="12" font-weight="600">Cua Driver</text>
<text x="32" y="82" fill="#1C1C1A" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="20" font-weight="600">{title}</text>
{summary_svg}
<path d="M34 169v-2.5a4 4 0 018 0v2.5m-9 0h10v8h-10z" fill="none" stroke="#8A8A84" stroke-width="1.2" stroke-linejoin="round"/>
<text x="51" y="176" fill="#777771" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="11.5">Local desktop confirmation · {risk_label}</text>
<rect x="32" y="202" width="172" height="44" rx="10" fill="{decline_fill}" stroke="#D6D6D2"/>
<text x="118" y="229" text-anchor="middle" fill="#2B2B29" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="13.5" font-weight="600">Don’t allow</text>
<rect x="220" y="202" width="172" height="44" rx="10" fill="{accept_fill}"/>
<text x="306" y="229" text-anchor="middle" fill="{accept_text}" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="13.5" font-weight="600">Allow once</text>
</svg>"##,
        risk_label = escape_xml(&risk_label)
    );
    render_svg(&svg, CONSENT_SIZE, scale)
}

pub fn render_indicator(
    card: &IndicatorCard,
    scale: f32,
    stop_hovered: bool,
) -> Result<Pixmap, RenderError> {
    let summary = sanitize_summary(&card.summary);
    let label = ellipsize(&summary, 31);
    let stop_fill = if stop_hovered { "#E7E7E3" } else { "#F5F5F2" };
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="380" height="76" viewBox="0 0 380 76">
<rect x="8" y="9" width="364" height="60" rx="18" fill="#171716" opacity=".12"/>
<rect x="4" y="4" width="372" height="64" rx="18" fill="#FBFBF9" stroke="#D5D5D1"/>
<circle cx="28" cy="36" r="9" fill="#1C1C1B"/>
<circle cx="28" cy="36" r="3" fill="#FFFFFF"/>
<text x="48" y="31" fill="#252523" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="12.5" font-weight="600">Cua is driving this session</text>
<text x="48" y="49" fill="#777771" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="10.5">{label}</text>
<rect x="296" y="14" width="64" height="44" rx="10" fill="{stop_fill}" stroke="#D6D6D2"/>
<text x="328" y="41" text-anchor="middle" fill="#242422" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="12.5" font-weight="600">Stop</text>
</svg>"##
    );
    render_svg(&svg, INDICATOR_SIZE, scale)
}

fn render_svg(svg: &str, size: Size, scale: f32) -> Result<Pixmap, RenderError> {
    if !scale.is_finite() || scale <= 0.0 || scale > 4.0 {
        return Err(RenderError::InvalidScale);
    }
    let mut options = usvg::Options::default();
    options.fontdb = font_database();
    let tree =
        usvg::Tree::from_str(svg, &options).map_err(|error| RenderError::Svg(error.to_string()))?;
    let width = (size.width as f32 * scale).round() as u32;
    let height = (size.height as f32 * scale).round() as u32;
    let mut pixmap = Pixmap::new(width, height).ok_or(RenderError::Allocation)?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn font_database() -> Arc<usvg::fontdb::Database> {
    static DATABASE: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DATABASE
        .get_or_init(|| {
            let mut database = usvg::fontdb::Database::new();
            database.load_system_fonts();
            Arc::new(database)
        })
        .clone()
}

fn operation_title(operation: &str) -> String {
    match operation {
        "browser.existing_profile.attach" => "Allow Cua to use Chrome?".to_owned(),
        other if !other.is_empty() => other.replace(['.', '_'], " "),
        _ => "Allow a protected action".to_owned(),
    }
}

fn wrap_text(input: &str, max_columns: usize, max_lines: usize) -> Vec<String> {
    if input.is_empty() {
        return vec!["Review this protected action before continuing.".to_owned()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in input.split_whitespace() {
        let extra = usize::from(!line.is_empty());
        if line.chars().count() + extra + word.chars().count() > max_columns && !line.is_empty() {
            lines.push(line);
            line = String::new();
            if lines.len() == max_lines {
                break;
            }
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if lines.len() < max_lines && !line.is_empty() {
        lines.push(line);
    }
    if lines.len() == max_lines {
        if let Some(last) = lines.last_mut() {
            *last = ellipsize(last, max_columns);
        }
    }
    lines
}

fn ellipsize(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    let mut output = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

fn text_lines_svg(lines: &[String], y: f64, size: f64, line_height: f64, fill: &str) -> String {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                r#"<text x="32" y="{}" fill="{}" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif" font-size="{}">{}</text>"#,
                y + index as f64 * line_height,
                fill,
                size,
                escape_xml(line)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> ConsentCard {
        ConsentCard {
            operation: "browser.existing_profile.attach".to_owned(),
            risk_label: "authenticated session".to_owned(),
            summary: "Attach to Chrome profile “Work” for this session.".to_owned(),
            request_digest: "abc".to_owned(),
            expires_unix_ms: 1,
        }
    }

    #[test]
    fn consent_renders_at_requested_scale() {
        let pixmap = render_consent(&card(), 2.0, ConsentVisualState::default()).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (848, 548));
        assert!(pixmap.data().iter().any(|byte| *byte != 0));
    }

    #[test]
    fn untrusted_xml_is_escaped() {
        let mut card = card();
        card.summary = "<script>&\"".to_owned();
        render_consent(&card, 1.0, ConsentVisualState::default()).unwrap();
    }

    #[test]
    fn indicator_renders() {
        let pixmap = render_indicator(
            &IndicatorCard {
                indicator_id: "one".to_owned(),
                summary: "Chrome — Work".to_owned(),
            },
            1.0,
            false,
        )
        .unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (380, 76));
    }
}
