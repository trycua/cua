//! Protected consent surface for interactive Linux X11/XWayland sessions.
//!
//! Pure Wayland deliberately fails closed: the protocol does not expose global
//! cursor coordinates or arbitrary window positioning, and GNOME does not
//! implement layer-shell. A future portal/compositor protocol can replace this
//! limitation without changing the shared UI or provider contract.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use overlay_ui::{
    place_near_pointer, render_consent, render_indicator, surface_fits, ConsentInteraction,
    ConsentVisualState, HelperDecision, HelperEvent, HelperRequest, InteractionOutcome, Point,
    Rect, ACCEPT_RECT, CONSENT_SIZE, CONTROL_ARM_DELAY, DECLINE_RECT, INDICATOR_SIZE, STOP_RECT,
};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, ImageFormat, InputFocus, MapState, PropMode, Rectangle, StackMode,
    VisualClass, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

pub fn interactive_surface_available() -> bool {
    std::env::var_os("DISPLAY").is_some() && x11rb::connect(None).is_ok()
}

pub fn run(request: HelperRequest) -> anyhow::Result<()> {
    if !interactive_surface_available() {
        anyhow::bail!(
            "protected Linux UI requires X11/XWayland; pure Wayland has no certified protected-positioning path"
        );
    }
    run_x11(request)
}

enum SurfaceMode {
    Consent {
        card: overlay_ui::ConsentCard,
        interaction: ConsentInteraction,
    },
    Indicator {
        card: overlay_ui::IndicatorCard,
        stop_pressed: bool,
    },
}

fn run_x11(request: HelperRequest) -> anyhow::Result<()> {
    let (connection, screen_number) = x11rb::connect(None)?;
    let screen = &connection.setup().roots[screen_number];
    let root = screen.root;
    let logical_size = match &request {
        HelperRequest::Consent(_) => CONSENT_SIZE,
        HelperRequest::Indicator(_) => INDICATOR_SIZE,
    };
    let pointer = connection.query_pointer(root)?.reply()?;
    let work = Rect {
        x: 0.0,
        y: 0.0,
        width: screen.width_in_pixels as f64,
        height: screen.height_in_pixels as f64,
    };
    if !surface_fits(work, logical_size) {
        anyhow::bail!("the active display work area is too small for the local confirmation");
    }
    let origin = match &request {
        HelperRequest::Consent(_) => place_near_pointer(
            Point {
                x: pointer.root_x as f64,
                y: pointer.root_y as f64,
            },
            work,
            logical_size,
        ),
        HelperRequest::Indicator(_) => Point {
            x: work.width - logical_size.width - 18.0,
            y: 18.0,
        },
    };
    let (visual, depth, base_colormap) = find_argb_visual(screen).unwrap_or((
        screen.root_visual,
        screen.root_depth,
        screen.default_colormap,
    ));
    let colormap = if visual != screen.root_visual {
        let id = connection.generate_id()?;
        connection.create_colormap(ColormapAlloc::NONE, id, root, visual)?;
        id
    } else {
        base_colormap
    };
    let window = connection.generate_id()?;
    connection.create_window(
        depth,
        window,
        root,
        origin.x.round() as i16,
        origin.y.round() as i16,
        logical_size.width as u16,
        logical_size.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .override_redirect(1)
            .event_mask(
                EventMask::POINTER_MOTION
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::KEY_PRESS
                    | EventMask::EXPOSURE
                    | EventMask::STRUCTURE_NOTIFY,
            ),
    )?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"Cua local confirmation",
    )?;
    set_window_metadata(
        &connection,
        window,
        matches!(&request, HelperRequest::Indicator(_)),
    )?;
    let gc = connection.generate_id()?;
    connection.create_gc(gc, window, &CreateGCAux::new())?;

    let mut mode = match request {
        HelperRequest::Consent(card) => SurfaceMode::Consent {
            card,
            interaction: ConsentInteraction::new(Instant::now(), ACCEPT_RECT, DECLINE_RECT),
        },
        HelperRequest::Indicator(card) => SurfaceMode::Indicator {
            card,
            stop_pressed: false,
        },
    };
    let mut local_pointer = Point { x: -1.0, y: -1.0 };
    let mut outcome = None;
    paint(&connection, window, depth, gc, &mode, local_pointer)?;
    connection.map_window(window)?;
    connection.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )?;
    let _ = connection.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME);
    connection.flush()?;
    let visibility_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if connection.get_window_attributes(window)?.reply()?.map_state == MapState::VIEWABLE {
            break;
        }
        if Instant::now() >= visibility_deadline {
            anyhow::bail!("the local confirmation window did not become viewable");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    paint(&connection, window, depth, gc, &mode, local_pointer)?;
    crate::protected_consent_event(&HelperEvent::Ready)?;

    let shown = Instant::now();
    let mut armed_redrawn = false;
    loop {
        while let Some(event) = connection.poll_for_event()? {
            match event {
                Event::MotionNotify(event) => {
                    local_pointer = Point {
                        x: event.event_x as f64,
                        y: event.event_y as f64,
                    };
                    if let SurfaceMode::Consent { interaction, .. } = &mut mode {
                        interaction.pointer_moved(local_pointer, Instant::now());
                    }
                    paint(&connection, window, depth, gc, &mode, local_pointer)?;
                }
                Event::ButtonPress(event) if event.detail == 1 => match &mut mode {
                    SurfaceMode::Consent { interaction, .. } => {
                        interaction.pointer_down(
                            Point {
                                x: event.event_x as f64,
                                y: event.event_y as f64,
                            },
                            overlay_ui::PointerButton::Primary,
                            Instant::now(),
                        );
                    }
                    SurfaceMode::Indicator { stop_pressed, .. } => {
                        *stop_pressed = STOP_RECT.contains(Point {
                            x: event.event_x as f64,
                            y: event.event_y as f64,
                        });
                    }
                },
                Event::ButtonRelease(event) if event.detail == 1 => {
                    let point = Point {
                        x: event.event_x as f64,
                        y: event.event_y as f64,
                    };
                    outcome = match &mut mode {
                        SurfaceMode::Consent { card, interaction } => {
                            let action = match interaction.pointer_up(point) {
                                InteractionOutcome::Accept => Some(HelperDecision::Accept),
                                InteractionOutcome::Decline => Some(HelperDecision::Decline),
                                _ => None,
                            };
                            action.map(|action| HelperEvent::Decision {
                                action,
                                request_digest: card.request_digest.clone(),
                            })
                        }
                        SurfaceMode::Indicator { card, stop_pressed } => {
                            let pressed = std::mem::take(stop_pressed);
                            (pressed && STOP_RECT.contains(point)).then(|| HelperEvent::Stop {
                                indicator_id: card.indicator_id.clone(),
                            })
                        }
                    };
                }
                // The canonical Xorg Escape keycode. Pointer decline remains
                // available when a non-Xorg keymap uses another code.
                Event::KeyPress(event) if event.detail == 9 => {
                    outcome = Some(fallback_outcome(&mode));
                }
                Event::DestroyNotify(_) => {
                    outcome = Some(fallback_outcome(&mode));
                }
                _ => {}
            }
            if outcome.is_some() {
                break;
            }
        }
        if outcome.is_some() {
            break;
        }
        if !armed_redrawn && shown.elapsed() >= CONTROL_ARM_DELAY {
            paint(&connection, window, depth, gc, &mode, local_pointer)?;
            armed_redrawn = true;
        }
        if consent_expired(&mode) {
            outcome = Some(fallback_outcome(&mode));
            break;
        }
        match mode {
            SurfaceMode::Indicator { .. } => {
                // Persistent indicator is fully event-driven: zero repaint or
                // timer wakeups while idle.
                let event = connection.wait_for_event()?;
                // Feed the blocking event through the same loop on the next
                // iteration by handling it directly here.
                match event {
                    Event::MotionNotify(event) => {
                        local_pointer = Point {
                            x: event.event_x as f64,
                            y: event.event_y as f64,
                        };
                        paint(&connection, window, depth, gc, &mode, local_pointer)?;
                    }
                    Event::ButtonPress(event) if event.detail == 1 => {
                        if let SurfaceMode::Indicator { stop_pressed, .. } = &mut mode {
                            *stop_pressed = STOP_RECT.contains(Point {
                                x: event.event_x as f64,
                                y: event.event_y as f64,
                            });
                        }
                    }
                    Event::ButtonRelease(event) if event.detail == 1 => {
                        if let SurfaceMode::Indicator { card, stop_pressed } = &mut mode {
                            let point = Point {
                                x: event.event_x as f64,
                                y: event.event_y as f64,
                            };
                            if std::mem::take(stop_pressed) && STOP_RECT.contains(point) {
                                outcome = Some(HelperEvent::Stop {
                                    indicator_id: card.indicator_id.clone(),
                                });
                            }
                        }
                    }
                    Event::DestroyNotify(_) => outcome = Some(fallback_outcome(&mode)),
                    _ => {}
                }
            }
            SurfaceMode::Consent { .. } => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    connection.destroy_window(window).ok();
    connection.flush().ok();
    crate::protected_consent_event(&outcome.unwrap_or_else(|| fallback_outcome(&mode)))?;
    Ok(())
}

fn set_window_metadata(
    connection: &impl Connection,
    window: u32,
    indicator: bool,
) -> anyhow::Result<()> {
    let atom = |name: &str| -> anyhow::Result<u32> {
        Ok(connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    };
    let wm_class = atom("WM_CLASS")?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        wm_class,
        AtomEnum::STRING,
        b"cua-driver\0CuaDriver\0",
    )?;

    let cardinal = AtomEnum::CARDINAL;
    connection.change_property32(
        PropMode::REPLACE,
        window,
        atom("_NET_WM_PID")?,
        cardinal,
        &[std::process::id()],
    )?;

    let window_type = atom("_NET_WM_WINDOW_TYPE")?;
    let concrete_type = atom(if indicator {
        "_NET_WM_WINDOW_TYPE_NOTIFICATION"
    } else {
        "_NET_WM_WINDOW_TYPE_DIALOG"
    })?;
    connection.change_property32(
        PropMode::REPLACE,
        window,
        window_type,
        AtomEnum::ATOM,
        &[concrete_type],
    )?;

    let state = atom("_NET_WM_STATE")?;
    let states = [
        atom("_NET_WM_STATE_ABOVE")?,
        atom("_NET_WM_STATE_STICKY")?,
        atom("_NET_WM_STATE_SKIP_TASKBAR")?,
        atom("_NET_WM_STATE_SKIP_PAGER")?,
    ];
    connection.change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &states)?;
    Ok(())
}

fn find_argb_visual(screen: &x11rb::protocol::xproto::Screen) -> Option<(u32, u8, u32)> {
    for entry in &screen.allowed_depths {
        if entry.depth != 32 {
            continue;
        }
        for visual in &entry.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some((visual.visual_id, 32, screen.default_colormap));
            }
        }
    }
    None
}

fn paint(
    connection: &impl Connection,
    window: u32,
    depth: u8,
    gc: u32,
    mode: &SurfaceMode,
    pointer: Point,
) -> anyhow::Result<()> {
    let pixmap = match mode {
        SurfaceMode::Consent { card, interaction } => render_consent(
            card,
            1.0,
            ConsentVisualState {
                accept_armed: interaction.accept_armed(Instant::now()),
                accept_hovered: ACCEPT_RECT.contains(pointer),
                decline_hovered: DECLINE_RECT.contains(pointer),
            },
        )?,
        SurfaceMode::Indicator { card, .. } => {
            render_indicator(card, 1.0, STOP_RECT.contains(pointer))?
        }
    };
    let mut bgra = Vec::with_capacity(pixmap.data().len());
    let mut shape = Vec::new();
    for (y, row) in pixmap
        .data()
        .chunks_exact((pixmap.width() * 4) as usize)
        .enumerate()
    {
        let mut run = None;
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            if pixel[3] != 0 {
                run.get_or_insert(x);
            } else if let Some(start) = run.take() {
                shape.push(Rectangle {
                    x: start as i16,
                    y: y as i16,
                    width: (x - start) as u16,
                    height: 1,
                });
            }
        }
        if let Some(start) = run {
            shape.push(Rectangle {
                x: start as i16,
                y: y as i16,
                width: pixmap.width() as u16 - start as u16,
                height: 1,
            });
        }
    }
    connection.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &shape,
    )?;
    connection.put_image(
        ImageFormat::Z_PIXMAP,
        window,
        gc,
        pixmap.width() as u16,
        pixmap.height() as u16,
        0,
        0,
        0,
        depth,
        &bgra,
    )?;
    connection.flush()?;
    Ok(())
}

fn consent_expired(mode: &SurfaceMode) -> bool {
    let SurfaceMode::Consent { card, .. } = mode else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    now >= u128::from(card.expires_unix_ms)
}

fn fallback_outcome(mode: &SurfaceMode) -> HelperEvent {
    match mode {
        SurfaceMode::Consent { card, .. } => HelperEvent::Decision {
            action: HelperDecision::Cancel,
            request_digest: card.request_digest.clone(),
        },
        SurfaceMode::Indicator { card, .. } => HelperEvent::Stop {
            indicator_id: card.indicator_id.clone(),
        },
    }
}
