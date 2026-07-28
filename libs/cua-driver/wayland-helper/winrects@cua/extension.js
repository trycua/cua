import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Cairo from 'cairo';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

Gio._promisify(Shell.Screenshot.prototype, 'screenshot');

const IFACE = `<node><interface name="org.cua.WinRects">
<method name="GetVersion"><arg type="u" direction="out" name="version"/></method>
<method name="GetRects"><arg type="s" direction="out" name="json"/></method>
<method name="Capture"><arg type="s" direction="out" name="png_base64"/></method>
<method name="Activate"><arg type="u" direction="in" name="id"/><arg type="b" direction="out" name="activated"/></method>
<method name="MoveCursor"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="ClickPulse"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="SetCursorColor"><arg type="s" direction="in" name="fill_color"/></method>
<method name="SetCursorState"><arg type="s" direction="in" name="action"/><arg type="s" direction="in" name="delivery"/><arg type="s" direction="in" name="target"/><arg type="b" direction="in" name="active"/></method>
<method name="SetSessionLabel"><arg type="s" direction="in" name="label"/></method>
<method name="HideCursor"></method>
</interface></node>`;

// GNOME cannot host the Rust renderer directly, so this Shell actor mirrors
// the embedded cua.default vector theme and semantic state vocabulary.
const CANVAS_SIZE = 128;
const DISPLAY_SIZE = 48;
const ACTOR_SIZE = 112;
const ACTOR_CENTER = ACTOR_SIZE / 2;
const SCALE = DISPLAY_SIZE / CANVAS_SIZE;
const FLOAT_DURATION = 4.0;
const GLOW_SURFACE_SCALE = 3;
const GLOW_PADDING = 24;
const BADGE_Y_OFFSET = 34;
const ACTIONS = new Set([
    'idle', 'observe', 'click', 'drag', 'scroll', 'text',
    'key', 'navigate', 'app', 'transfer', 'record', 'system',
]);
const ONE_SHOT_ACTIONS = new Set(['click', 'key', 'navigate', 'app', 'system']);
const ACTION_DURATIONS = {
    idle: 4.0,
    click: 0.67,
    observe: 1.6,
    drag: 1.6,
    scroll: 1.6,
    text: 1.6,
    key: 1.6,
    navigate: 1.6,
    app: 1.6,
    transfer: 1.6,
    record: 1.6,
    system: 1.6,
};

function nowSeconds() {
    return GLib.get_monotonic_time() / 1_000_000;
}

function easeInOut(value) {
    const t = Math.max(0, Math.min(1, value));
    return t * t * (3 - 2 * t);
}

function triangleWave(value) {
    const t = ((value % 1) + 1) % 1;
    return t < 0.5 ? t * 2 : (1 - t) * 2;
}

function setInk(cr, alpha = 1) {
    cr.setSourceRGBA(1, 1, 1, alpha);
}

function setPaper(cr, alpha = 1) {
    cr.setSourceRGBA(1, 1, 1, alpha);
}

function setFill(cr, color, alpha = 1) {
    cr.setSourceRGBA(color[0] / 255, color[1] / 255, color[2] / 255, alpha);
}

function glowPath(cr, width, alpha, glowColor) {
    if (!glowColor)
        return;
    const layers = 12;
    const outerExpansion = 13;
    const innerExpansion = 2.5;
    const maxOpacity = 0.17;
    let accumulatedOpacity = 0;
    for (let layer = 0; layer < layers; layer++) {
        const progress = (layer + 1) / layers;
        const expansion = outerExpansion + (innerExpansion - outerExpansion) * progress;
        const targetOpacity = maxOpacity * Math.max(0, Math.min(1, alpha)) *
            Math.pow(progress, 1.6);
        const layerOpacity = (targetOpacity - accumulatedOpacity) /
            Math.max(0.001, 1 - accumulatedOpacity);
        accumulatedOpacity = targetOpacity;
        cr.setLineWidth(width + expansion);
        cr.setLineCap(Cairo.LineCap.ROUND);
        cr.setLineJoin(Cairo.LineJoin.ROUND);
        setFill(cr, glowColor, layerOpacity);
        cr.strokePreserve();
    }
}

function strokePath(cr, width, alpha = 1, glowColor = null) {
    glowPath(cr, width, alpha, glowColor);
    cr.setLineWidth(glowColor ? width + 1.5 : width);
    cr.setLineCap(Cairo.LineCap.ROUND);
    cr.setLineJoin(Cairo.LineJoin.ROUND);
    setInk(cr, alpha);
    if (!glowColor) {
        cr.stroke();
        return;
    }
    cr.strokePreserve();
    cr.setLineWidth(Math.max(1.5, width - 1));
    setFill(cr, glowColor, alpha);
    cr.stroke();
}

function fillPath(cr, glowWidth, alpha = 1, glowColor = null) {
    glowPath(cr, glowWidth, alpha, glowColor);
    cr.setLineWidth(3);
    cr.setLineCap(Cairo.LineCap.ROUND);
    cr.setLineJoin(Cairo.LineJoin.ROUND);
    setInk(cr, alpha);
    cr.strokePreserve();
    setFill(cr, glowColor, alpha);
    cr.fill();
}

function linePath(cr, points, width = 4, alpha = 1, glowColor = null) {
    if (points.length === 0)
        return;
    cr.moveTo(points[0][0], points[0][1]);
    for (let i = 1; i < points.length; i++)
        cr.lineTo(points[i][0], points[i][1]);
    strokePath(cr, width, alpha, glowColor);
}

function roundedRect(cr, x, y, width, height, radius) {
    const r = Math.max(0, Math.min(radius, width / 2, height / 2));
    const k = 0.5522848;
    cr.moveTo(x + r, y);
    cr.lineTo(x + width - r, y);
    cr.curveTo(x + width - r + r * k, y, x + width, y + r - r * k, x + width, y + r);
    cr.lineTo(x + width, y + height - r);
    cr.curveTo(x + width, y + height - r + r * k, x + width - r + r * k, y + height, x + width - r, y + height);
    cr.lineTo(x + r, y + height);
    cr.curveTo(x + r - r * k, y + height, x, y + height - r + r * k, x, y + height - r);
    cr.lineTo(x, y + r);
    cr.curveTo(x, y + r - r * k, x + r - r * k, y, x + r, y);
    cr.closePath();
}

function traceCursorBody(cr) {
    cr.moveTo(55, 30);
    cr.curveTo(48, 28, 42, 33, 43, 41);
    cr.lineTo(64, 98);
    cr.curveTo(67, 106, 73, 106, 77, 99);
    cr.lineTo(86, 79);
    cr.curveTo(88, 75, 91, 72, 95, 70);
    cr.lineTo(108, 63);
    cr.curveTo(115, 59, 114, 53, 107, 50);
    cr.closePath();
}

function drawCursorGlowShape(cr, fillColor) {
    const layers = 36;
    const outerWidth = 44;
    const innerWidth = 7;
    const maxOpacity = 0.34;
    let accumulatedOpacity = 0;

    for (let layer = 0; layer < layers; layer++) {
        const progress = (layer + 1) / layers;
        const width = outerWidth + (innerWidth - outerWidth) * progress;
        const targetOpacity = maxOpacity * Math.pow(progress, 1.65);
        const layerOpacity =
            (targetOpacity - accumulatedOpacity) / Math.max(0.001, 1 - accumulatedOpacity);
        accumulatedOpacity = targetOpacity;
        if (layerOpacity <= 0)
            continue;

        traceCursorBody(cr);
        cr.setLineWidth(width);
        cr.setLineCap(Cairo.LineCap.ROUND);
        cr.setLineJoin(Cairo.LineJoin.ROUND);
        setFill(cr, fillColor, layerOpacity);
        cr.stroke();
    }
}

function createGlowSurface(fillColor) {
    const surface = new Cairo.ImageSurface(
        Cairo.Format.ARGB32,
        (CANVAS_SIZE + GLOW_PADDING * 2) * GLOW_SURFACE_SCALE,
        (CANVAS_SIZE + GLOW_PADDING * 2) * GLOW_SURFACE_SCALE
    );
    const cr = new Cairo.Context(surface);
    cr.scale(GLOW_SURFACE_SCALE, GLOW_SURFACE_SCALE);
    cr.translate(GLOW_PADDING, GLOW_PADDING);
    drawCursorGlowShape(cr, fillColor);
    cr.$dispose();
    return surface;
}

function sharedFloatMotion(progress) {
    const angle = progress * Math.PI * 2;
    return {
        dx: Math.sin(angle) * 5,
        dy: Math.cos(angle) * 6 - 5,
        rotation: Math.cos(angle) * (2.5 * Math.PI / 180),
        scale: 1,
    };
}

function cursorBodyMotion(progress, action) {
    let dx = 0;
    let dy = 0;
    let rotation = 0;
    let scale = 1;

    if (action === 'click') {
        if (progress < 0.35)
            scale = 1 - easeInOut(progress / 0.35) * 0.07;
        else if (progress < 0.6)
            scale = 0.93 + easeInOut((progress - 0.35) / 0.25) * 0.10;
        else
            scale = 1.03 - easeInOut((progress - 0.6) / 0.4) * 0.03;
    } else if (action === 'drag') {
        const held = easeInOut(triangleWave(progress));
        dx = held * 7;
        dy = held * 3;
    }

    return {dx, dy, rotation, scale};
}

function applyCursorBodyMotion(cr, motion) {
    cr.translate(64 + motion.dx, 64 + motion.dy);
    cr.rotate(motion.rotation);
    cr.scale(motion.scale, motion.scale);
    cr.translate(-64, -64);
}

function drawCursorGlow(cr, progress, action, glowSurface) {
    const motion = cursorBodyMotion(progress, action);
    cr.save();
    applyCursorBodyMotion(cr, motion);
    cr.translate(-GLOW_PADDING, -GLOW_PADDING);
    cr.scale(1 / GLOW_SURFACE_SCALE, 1 / GLOW_SURFACE_SCALE);
    cr.setSourceSurface(glowSurface, 0, 0);
    cr.paint();
    cr.restore();
}

function drawCursorBody(cr, progress, action, fillColor) {
    const motion = cursorBodyMotion(progress, action);
    cr.save();
    applyCursorBodyMotion(cr, motion);
    traceCursorBody(cr);
    setFill(cr, fillColor);
    cr.fillPreserve();
    cr.setLineWidth(5);
    cr.setLineCap(Cairo.LineCap.ROUND);
    cr.setLineJoin(Cairo.LineJoin.ROUND);
    setPaper(cr);
    cr.stroke();
    cr.restore();
}

function drawActionCue(cr, action, progress, fillColor) {
    const wave = triangleWave(progress);
    const strokeCue = (width, alpha = 1) =>
        strokePath(cr, width, alpha, fillColor);
    const lineCue = (points, width = 4, alpha = 1) =>
        linePath(cr, points, width, alpha, fillColor);
    cr.save();
    switch (action) {
    case 'observe': {
        const opacity = Math.min(1, progress * 7);
        cr.translate(8, -10);
        cr.moveTo(38, 28);
        cr.curveTo(27, 29, 20, 38, 20, 49);
        strokeCue(4, opacity);
        cr.moveTo(42, 19);
        cr.curveTo(23, 19, 11, 33, 11, 51);
        strokeCue(4, opacity);
        break;
    }
    case 'click': {
        const cueProgress = Math.max(0, Math.min(1, progress / 0.65));
        const opacity = Math.max(0, Math.min(1,
            Math.min(1 - Math.pow(cueProgress, 1.35), cueProgress * 5)));
        const cueScale = 1.1 + easeInOut(cueProgress) * 0.4;
        cr.translate(35, 28);
        cr.scale(cueScale, cueScale);
        cr.translate(-25, -25);
        lineCue([[35, 20], [34, 11]], 4, opacity);
        lineCue([[27, 25], [19, 19]], 4, opacity);
        lineCue([[25, 34], [15, 34]], 4, opacity);
        break;
    }
    case 'drag': {
        const offset = easeInOut(wave) * 7;
        cr.translate(offset, offset * 0.43);
        lineCue([[28, 38], [16, 35]], 4, 0.2 + wave * 0.8);
        lineCue([[26, 48], [12, 45]], 4, 0.2 + wave * 0.8);
        break;
    }
    case 'scroll': {
        cr.translate(-5, 4 - wave * 8);
        lineCue([[23, 31], [31, 22], [39, 31]], 4, 0.42 + wave * 0.58);
        lineCue([[23, 49], [31, 58], [39, 49]], 4, 0.42 + wave * 0.58);
        break;
    }
    case 'text': {
        const opacity = progress < 0.34 || progress > 0.64 ? 1 : 0.18;
        cr.translate(-4, 0);
        lineCue([[31, 22], [31, 58]], 4, opacity);
        lineCue([[24, 22], [38, 22]], 4, opacity);
        lineCue([[24, 58], [38, 58]], 4, opacity);
        break;
    }
    case 'key': {
        const bounce = Math.sin(progress * Math.PI * 2) * (1 - progress) * 3;
        cr.translate(-9, bounce);
        roundedRect(cr, 14, 25, 28, 28, 6);
        strokeCue(3.5);
        lineCue([[23, 32], [23, 46], [23, 39], [33, 32], [24, 39], [34, 46]], 3.5);
        break;
    }
    case 'navigate': {
        cr.translate(-10 + easeInOut(progress) * 9, 0);
        lineCue([[15, 29], [25, 40], [15, 51]], 4, 0.2 + wave * 0.8);
        lineCue([[29, 29], [39, 40], [29, 51]], 4, 0.2 + wave * 0.8);
        break;
    }
    case 'app': {
        const s = 0.2 + easeInOut(Math.min(1, progress * 2)) * 0.8;
        cr.translate(21, 39);
        cr.scale(s, s);
        cr.translate(-26, -39);
        for (const [x, y] of [[13, 26], [29, 26], [13, 42], [29, 42]]) {
            roundedRect(cr, x, y, 10, 10, 2);
            strokeCue(3.5);
        }
        break;
    }
    case 'transfer':
        cr.translate(-9, 6 - wave * 12);
        lineCue([[22, 50], [22, 20], [14, 28], [22, 20], [30, 28]], 4, 0.38 + wave * 0.62);
        lineCue([[37, 28], [37, 58], [29, 50], [37, 58], [45, 50]], 4, 0.38 + wave * 0.62);
        break;
    case 'record':
        cr.translate(-12, 0);
        cr.arc(29, 39, 17, 0, Math.PI * 2);
        strokeCue(4);
        cr.arc(29, 39, 3.6 + wave * 2.1, 0, Math.PI * 2);
        fillPath(cr, (3.6 + wave * 2.1) * 1.25, 0.42 + wave * 0.58, fillColor);
        break;
    case 'system': {
        cr.translate(15, 39);
        cr.rotate((easeInOut(progress) * 68 - 18) * Math.PI / 180);
        cr.translate(-29, -39);
        for (const radius of [12, 4]) {
            cr.arc(29, 39, radius, 0, Math.PI * 2);
            strokeCue(3.5);
        }
        for (const points of [
            [[29, 20], [29, 25]], [[29, 53], [29, 58]],
            [[10, 39], [15, 39]], [[43, 39], [48, 39]],
            [[16, 26], [20, 30]], [[38, 48], [42, 52]],
            [[16, 52], [20, 48]], [[38, 30], [42, 26]],
        ])
            lineCue(points, 3.5);
        break;
    }
    default:
        break;
    }
    cr.restore();
}

function drawModifiers(cr, delivery, target, fillColor) {
    const strokeCue = (width, alpha = 1) =>
        strokePath(cr, width, alpha, fillColor);
    const lineCue = (points, width = 4, alpha = 1) =>
        linePath(cr, points, width, alpha, fillColor);
    if (delivery === 'background') {
        cr.save();
        cr.setDash([1, 5], 0);
        cr.moveTo(34, 23);
        cr.curveTo(20, 31, 17, 48, 21, 63);
        cr.curveTo(25, 80, 38, 95, 53, 106);
        strokeCue(2.5, 0.75);
        cr.restore();
    } else if (delivery === 'foreground') {
        cr.arc(25, 90, 9, 0, Math.PI * 2);
        strokeCue(3);
    }

    switch (target) {
    case 'ax':
        for (const [x, y] of [[104, 89], [94, 104], [114, 104]]) {
            cr.arc(x, y, 3, 0, Math.PI * 2);
            strokeCue(2.5);
        }
        lineCue([[104, 92], [104, 97], [94, 101], [104, 97], [114, 101]], 2.5);
        break;
    case 'pixel':
        cr.save();
        cr.setDash([2, 3], 0);
        cr.rectangle(94, 91, 19, 19);
        strokeCue(2.5);
        cr.restore();
        break;
    case 'browser':
        cr.arc(104, 100, 10, 0, Math.PI * 2);
        strokeCue(2.5);
        lineCue([[94, 100], [114, 100]], 2.5);
        lineCue([[104, 90], [100, 100], [104, 110]], 2.5);
        lineCue([[104, 90], [108, 100], [104, 110]], 2.5);
        break;
    case 'desktop':
        roundedRect(cr, 93, 90, 21, 15, 2);
        strokeCue(2.5);
        lineCue([[103.5, 105], [103.5, 110], [97, 110], [110, 110]], 2.5);
        break;
    default:
        break;
    }
}

export default class WinRectsExtension extends Extension {
    enable() {
        this._impl = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._impl.export(Gio.DBus.session, '/org/cua/WinRects');
        this._nameId = Gio.bus_own_name(Gio.BusType.SESSION, 'org.cua.WinRects',
            Gio.BusNameOwnerFlags.REPLACE, null, null, null);
        this._action = 'idle';
        this._delivery = '';
        this._target = '';
        this._actionStarted = nowSeconds();
        this._endingAt = null;
        this._fillColor = [94, 192, 232];
        this._fillColorCss = '#5ec0e8';
        this._glowSurface = createGlowSurface(this._fillColor);
        this._cursor = new St.DrawingArea({
            width: ACTOR_SIZE,
            height: ACTOR_SIZE,
            visible: false,
            reactive: false,
            can_focus: false,
        });
        this._cursor.connect('repaint', (area) => {
            const cr = area.get_context();
            const duration = ACTION_DURATIONS[this._action] ?? 1.6;
            const elapsed = nowSeconds() - this._actionStarted;
            const progress = (elapsed % duration) / duration;
            const floatProgress = (elapsed % FLOAT_DURATION) / FLOAT_DURATION;
            cr.save();
            cr.translate(ACTOR_CENTER, ACTOR_CENTER);
            cr.scale(SCALE, SCALE);
            cr.translate(-CANVAS_SIZE / 2, -CANVAS_SIZE / 2);
            applyCursorBodyMotion(cr, sharedFloatMotion(floatProgress));
            drawCursorGlow(cr, progress, this._action, this._glowSurface);
            drawActionCue(cr, this._action, progress, this._fillColor);
            drawCursorBody(cr, progress, this._action, this._fillColor);
            drawModifiers(cr, this._delivery, this._target, this._fillColor);
            cr.restore();
            cr.$dispose();
        });
        this._cursor.set_pivot_point(0.5, 0.5);
        Main.layoutManager.addTopChrome(this._cursor);
        this._badgeDot = new St.Widget({
            width: 5,
            height: 5,
            style: `background-color: ${this._fillColorCss}; border-radius: 3px;`,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._badgeLabel = new St.Label({
            text: '',
            y_align: Clutter.ActorAlign.CENTER,
            style: 'font-size: 11px; font-weight: 600; color: white;',
        });
        this._badge = new St.BoxLayout({
            visible: false,
            reactive: false,
            can_focus: false,
            style: [
                'spacing: 6px',
                'padding: 5px 10px',
                'background-color: rgba(18, 22, 28, 0.88)',
                'border: 1px solid rgba(255, 255, 255, 0.36)',
                'border-radius: 12px',
                'box-shadow: 0 2px 8px rgba(0, 0, 0, 0.30)',
            ].join(';'),
        });
        this._badge.add_child(this._badgeDot);
        this._badge.add_child(this._badgeLabel);
        Main.layoutManager.addTopChrome(this._badge);
        this._cursorX = null;
        this._cursorY = null;
        this._frameId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 33, () => {
            if (!this._cursor)
                return GLib.SOURCE_REMOVE;
            const now = nowSeconds();
            if (this._endingAt !== null && now >= this._endingAt)
                this._setCursorState('idle', '', '');
            if (ONE_SHOT_ACTIONS.has(this._action) &&
                now - this._actionStarted >= ACTION_DURATIONS[this._action])
                this._setCursorState('idle', '', '');
            if (this._cursor.visible)
                this._cursor.queue_repaint();
            return GLib.SOURCE_CONTINUE;
        });
    }
    disable() {
        if (this._frameId) {
            GLib.source_remove(this._frameId);
            this._frameId = 0;
        }
        if (this._cursor) { this._cursor.destroy(); this._cursor = null; }
        if (this._badge) { this._badge.destroy(); this._badge = null; }
        this._badgeDot = null;
        this._badgeLabel = null;
        if (this._glowSurface) {
            this._glowSurface.finish();
            this._glowSurface = null;
        }
        if (this._impl) { this._impl.unexport(); this._impl = null; }
        if (this._nameId) { Gio.bus_unown_name(this._nameId); this._nameId = 0; }
    }
    GetVersion() { return 6; }
    GetRects() {
        const actors = global.get_window_actors();
        const actorByWindow = new Map();
        for (const actor of actors) {
            if (actor.meta_window)
                actorByWindow.set(actor.meta_window, actor);
        }
        const windows = global.display.sort_windows_by_stacking([...actorByWindow.keys()]);
        const focusedWindow = global.display.focus_window;
        const out = [];
        for (let stacking = 0; stacking < windows.length; stacking++) {
            const w = windows[stacking];
            const actor = actorByWindow.get(w);
            const r = w.get_frame_rect();
            let buffer = r;
            try {
                buffer = w.get_buffer_rect();
            } catch (_error) {
                // Older Shell releases may not expose the buffer rectangle.
            }
            const minimized = Boolean(w.minimized);
            out.push({
                id: w.get_stable_sequence(),
                pid: w.get_pid(),
                title: w.get_title() ?? '',
                x: r.x,
                y: r.y,
                w: r.width,
                h: r.height,
                buffer_x: buffer.x,
                buffer_y: buffer.y,
                focused: focusedWindow === w,
                minimized,
                visible: Boolean(actor?.visible) && !minimized,
                stacking,
            });
        }
        return JSON.stringify(out);
    }
    async CaptureAsync(_params, invocation) {
        try {
            const shooter = new Shell.Screenshot();
            const stream = Gio.MemoryOutputStream.new_resizable();
            const [width, height] = global.display.get_size();
            if (width <= 0 || height <= 0)
                throw new Error(`Invalid display size ${width}x${height}`);
            // GNOME 50's stage-content capture copies the real cursor sprite
            // after painting the stage. Remote/headless pointer seats can
            // expose a 0x0 sprite, and that copy can crash Shell. The stream
            // API has an explicit include_cursor flag, so keep cursor capture
            // disabled; cua-driver draws its own agent cursor.
            await shooter.screenshot(false, stream);
            stream.close(null);
            const encoded = GLib.base64_encode(stream.steal_as_bytes().get_data());
            invocation.return_value(new GLib.Variant('(s)', [encoded]));
        } catch (error) {
            invocation.return_dbus_error('org.cua.WinRects.CaptureFailed', String(error));
        }
    }
    ActivateAsync([id], invocation) {
        const target = global.get_window_actors()
            .map(actor => actor.meta_window)
            .find(window => window?.get_stable_sequence() === id);
        if (!target) {
            invocation.return_value(new GLib.Variant('(b)', [false]));
            return;
        }
        target.activate(global.get_current_time());
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, 100, () => {
            invocation.return_value(new GLib.Variant(
                '(b)',
                [global.display.focus_window === target]
            ));
            return GLib.SOURCE_REMOVE;
        });
    }
    MoveCursor(x, y) {
        if (!this._cursor) return;
        this._cursorX = x;
        this._cursorY = y;
        this._cursor.show();
        this._cursor.ease({
            x: x - ACTOR_CENTER,
            y: y - ACTOR_CENTER,
            duration: 480,
            mode: Clutter.AnimationMode.EASE_OUT_CUBIC,
        });
        this._positionBadge(x, y, 480);
    }
    ClickPulse(x, y) {
        if (!this._cursor) return;
        this._cursorX = x;
        this._cursorY = y;
        this._cursor.set_position(x - ACTOR_CENTER, y - ACTOR_CENTER);
        this._positionBadge(x, y, 0);
        this._setCursorState('click', this._delivery, this._target);
        this._cursor.show();
        this._cursor.queue_repaint();
    }
    SetCursorColor(fillColor) {
        const match = /^#([0-9a-fA-F]{6})$/.exec(fillColor);
        if (!match)
            return;
        const rgb = Number.parseInt(match[1], 16);
        this._fillColor = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff];
        this._fillColorCss = `#${match[1].toLowerCase()}`;
        if (this._badgeDot)
            this._badgeDot.set_style(
                `background-color: ${this._fillColorCss}; border-radius: 3px;`
            );
        if (this._glowSurface)
            this._glowSurface.finish();
        this._glowSurface = createGlowSurface(this._fillColor);
        if (this._cursor)
            this._cursor.queue_repaint();
    }
    SetCursorState(action, delivery, target, active) {
        if (!ACTIONS.has(action))
            return;
        if (active) {
            this._setCursorState(action, delivery, target);
        } else if (this._action === action && !ONE_SHOT_ACTIONS.has(action)) {
            this._endingAt = nowSeconds() + 0.4;
        }
    }
    SetSessionLabel(label) {
        if (!this._badgeLabel || !this._badge)
            return;
        this._badgeLabel.set_text(label);
        if (label.length === 0 || !this._cursor?.visible) {
            this._badge.hide();
            return;
        }
        if (this._cursorX !== null && this._cursorY !== null)
            this._positionBadge(this._cursorX, this._cursorY, 0);
    }
    _positionBadge(x, y, duration) {
        if (!this._badge || !this._badgeLabel ||
            this._badgeLabel.get_text().length === 0)
            return;
        const [, naturalWidth] = this._badge.get_preferred_width(-1);
        const badgeX = Math.round(x - naturalWidth / 2);
        const badgeY = Math.round(y + BADGE_Y_OFFSET);
        this._badge.show();
        if (duration > 0) {
            this._badge.ease({
                x: badgeX,
                y: badgeY,
                duration,
                mode: Clutter.AnimationMode.EASE_OUT_CUBIC,
            });
        } else {
            this._badge.set_position(badgeX, badgeY);
        }
    }
    _setCursorState(action, delivery, target) {
        this._action = ACTIONS.has(action) ? action : 'idle';
        this._delivery = delivery ?? '';
        this._target = target ?? '';
        this._actionStarted = nowSeconds();
        this._endingAt = null;
        if (this._cursor)
            this._cursor.queue_repaint();
    }
    HideCursor() {
        if (this._cursor) this._cursor.hide();
        if (this._badge) this._badge.hide();
    }
}
