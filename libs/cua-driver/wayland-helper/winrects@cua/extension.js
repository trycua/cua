import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Cairo from 'cairo';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE = `<node><interface name="org.cua.WinRects">
<method name="GetRects"><arg type="s" direction="out" name="json"/></method>
<method name="Activate"><arg type="u" direction="in" name="id"/><arg type="b" direction="out" name="activated"/></method>
<method name="MoveCursor"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="ClickPulse"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="HideCursor"></method>
</interface></node>`;

// The same agent cursor cua-driver renders on every other platform: the
// procedural gradient arrow from cursor-overlay (verts tip-at-+x, `default_blue`
// palette). Rotated to point up-left and translated so the tip sits at the
// actor's (TIPX, TIPY); MoveCursor/ClickPulse place that tip on the target.
const VERTS = [[14, 0], [-8, -9], [-3, 0], [-8, 9]];
const ANGLE = Math.PI * 1.25;           // tip points up-left
const TIPX = 2, TIPY = 2;
function arrowPoints() {
    const ca = Math.cos(ANGLE), sa = Math.sin(ANGLE);
    const rot = VERTS.map(([x, y]) => [ca * x - sa * y, sa * x + ca * y]);
    const tx = TIPX - rot[0][0], ty = TIPY - rot[0][1];
    return rot.map(([x, y]) => [x + tx, y + ty]);
}

export default class WinRectsExtension extends Extension {
    enable() {
        this._impl = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._impl.export(Gio.DBus.session, '/org/cua/WinRects');
        this._nameId = Gio.bus_own_name(Gio.BusType.SESSION, 'org.cua.WinRects',
            Gio.BusNameOwnerFlags.REPLACE, null, null, null);
        this._cursor = new St.DrawingArea({width: 30, height: 30, visible: false, reactive: false, can_focus: false});
        this._cursor.connect('repaint', (area) => {
            const cr = area.get_context();
            const P = arrowPoints();
            cr.moveTo(P[0][0], P[0][1]);
            for (let i = 1; i < P.length; i++) cr.lineTo(P[i][0], P[i][1]);
            cr.closePath();
            const tail = [(P[1][0] + P[3][0]) / 2, (P[1][1] + P[3][1]) / 2];
            try {
                const g = new Cairo.LinearGradient(P[0][0], P[0][1], tail[0], tail[1]);
                g.addColorStopRGBA(0.00, 219 / 255, 238 / 255, 255 / 255, 0.97);
                g.addColorStopRGBA(0.53, 94 / 255, 192 / 255, 232 / 255, 0.97);
                g.addColorStopRGBA(1.00, 84 / 255, 205 / 255, 160 / 255, 0.97);
                cr.setSource(g);
            } catch (e) {
                cr.setSourceRGBA(94 / 255, 192 / 255, 232 / 255, 0.97);
            }
            cr.fillPreserve();
            cr.setLineWidth(1.4); cr.setSourceRGBA(1, 1, 1, 0.95); cr.stroke();
            cr.$dispose();
        });
        this._cursor.set_pivot_point(TIPX / 30, TIPY / 30);
        Main.layoutManager.addTopChrome(this._cursor);
    }
    disable() {
        if (this._cursor) { this._cursor.destroy(); this._cursor = null; }
        if (this._impl) { this._impl.unexport(); this._impl = null; }
        if (this._nameId) { Gio.bus_unown_name(this._nameId); this._nameId = 0; }
    }
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
            const minimized = Boolean(w.minimized);
            out.push({
                id: w.get_stable_sequence(),
                pid: w.get_pid(),
                title: w.get_title() ?? '',
                x: r.x,
                y: r.y,
                w: r.width,
                h: r.height,
                focused: focusedWindow === w,
                minimized,
                visible: Boolean(actor?.visible) && !minimized,
                stacking,
            });
        }
        return JSON.stringify(out);
    }
    Activate(id) {
        const target = global.get_window_actors()
            .map(actor => actor.meta_window)
            .find(window => window?.get_stable_sequence() === id);
        if (!target)
            return false;
        target.activate(global.get_current_time());
        return true;
    }
    MoveCursor(x, y) {
        if (!this._cursor) return;
        this._cursor.show();
        this._cursor.ease({x: x - TIPX, y: y - TIPY, duration: 480, mode: Clutter.AnimationMode.EASE_OUT_CUBIC});
    }
    ClickPulse(x, y) {
        if (!this._cursor) return;
        this._cursor.set_position(x - TIPX, y - TIPY);
        this._cursor.show();
        this._cursor.ease({scale_x: 1.5, scale_y: 1.5, duration: 130, mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            onComplete: () => { if (this._cursor) this._cursor.ease({scale_x: 1, scale_y: 1, duration: 130}); }});
    }
    HideCursor() { if (this._cursor) this._cursor.hide(); }
}
