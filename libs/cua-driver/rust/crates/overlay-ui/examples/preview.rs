use cursor_overlay::{theme::paint_default_theme, CursorVisualState};
use overlay_ui::{
    render_consent, render_indicator, ConsentCard, ConsentVisualState, IndicatorCard,
};
use tiny_skia::{Color, Paint, Pixmap, PixmapPaint, Rect, Transform};

fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: cargo run -p overlay-ui --example preview -- <output.png>");
    let mut canvas = Pixmap::new(1200, 720).expect("canvas");
    canvas.fill(Color::from_rgba8(231, 231, 227, 255));

    // A quiet Codex-like application backdrop to show the real overlay scale.
    rounded_panel(&mut canvas, 78.0, 80.0, 1044.0, 560.0, [250, 250, 248, 255]);
    rounded_panel(&mut canvas, 79.0, 81.0, 238.0, 558.0, [242, 242, 239, 255]);
    rounded_panel(&mut canvas, 107.0, 116.0, 178.0, 32.0, [229, 229, 225, 255]);
    rounded_panel(&mut canvas, 107.0, 171.0, 178.0, 18.0, [235, 235, 232, 255]);
    rounded_panel(&mut canvas, 107.0, 205.0, 142.0, 18.0, [235, 235, 232, 255]);
    rounded_panel(&mut canvas, 355.0, 116.0, 716.0, 46.0, [246, 246, 243, 255]);
    rounded_panel(&mut canvas, 355.0, 190.0, 590.0, 14.0, [231, 231, 227, 255]);
    rounded_panel(&mut canvas, 355.0, 218.0, 654.0, 14.0, [238, 238, 234, 255]);
    rounded_panel(&mut canvas, 355.0, 246.0, 520.0, 14.0, [238, 238, 234, 255]);

    let consent = render_consent(
        &ConsentCard {
            operation: "browser.existing_profile.attach".to_owned(),
            risk_label: "authenticated session".to_owned(),
            summary:
                "Cua can access signed-in sites in the “Work” profile until you stop the session."
                    .to_owned(),
            request_digest: "preview".to_owned(),
            expires_unix_ms: 0,
        },
        1.0,
        ConsentVisualState {
            accept_armed: true,
            accept_hovered: false,
            decline_hovered: false,
        },
    )
    .expect("consent render");
    canvas.draw_pixmap(
        518,
        288,
        consent.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    let indicator = render_indicator(
        &IndicatorCard {
            indicator_id: "preview".to_owned(),
            summary: "Chrome — Work".to_owned(),
        },
        1.0,
        false,
    )
    .expect("indicator render");
    canvas.draw_pixmap(
        706,
        100,
        indicator.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    draw_cursor(&mut canvas, 491.0, 270.0);

    image::save_buffer(
        output,
        canvas.data(),
        canvas.width(),
        canvas.height(),
        image::ColorType::Rgba8,
    )
    .expect("write preview");
}

fn rounded_panel(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, rgba: [u8; 4]) {
    let Some(rect) = Rect::from_xywh(x, y, width, height) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn draw_cursor(pixmap: &mut Pixmap, x: f32, y: f32) {
    paint_default_theme(
        pixmap,
        &CursorVisualState::default(),
        x,
        y,
        std::f32::consts::FRAC_PI_4,
        1.0,
        1.0,
    );
}
