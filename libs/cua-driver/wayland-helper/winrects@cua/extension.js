import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE = `<node><interface name="org.cua.WinRects">
<method name="GetRects"><arg type="s" direction="out" name="json"/></method>
<method name="MoveCursor"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="ClickPulse"><arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/></method>
<method name="HideCursor"></method>
</interface></node>`;

export default class WinRectsExtension extends Extension {
    enable() {
        this._impl = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._impl.export(Gio.DBus.session, '/org/cua/WinRects');
        this._nameId = Gio.bus_own_name(Gio.BusType.SESSION, 'org.cua.WinRects',
            Gio.BusNameOwnerFlags.REPLACE, null, null, null);
        this._cursor = new St.Widget({
            style: 'background-color: rgba(30,144,255,0.85); border: 3px solid white; border-radius: 15px;',
            width: 30, height: 30, visible: false, reactive: false, can_focus: false,
        });
        this._cursor.set_pivot_point(0.5, 0.5);
        Main.layoutManager.addTopChrome(this._cursor);
    }
    disable() {
        if (this._cursor) { this._cursor.destroy(); this._cursor = null; }
        if (this._impl) { this._impl.unexport(); this._impl = null; }
        if (this._nameId) { Gio.bus_unown_name(this._nameId); this._nameId = 0; }
    }
    GetRects() {
        const out = [];
        for (const a of global.get_window_actors()) {
            const w = a.meta_window;
            if (!w) continue;
            const r = w.get_frame_rect();
            out.push({pid: w.get_pid(), title: w.get_title(), x: r.x, y: r.y, w: r.width, h: r.height});
        }
        return JSON.stringify(out);
    }
    MoveCursor(x, y) {
        if (!this._cursor) return;
        this._cursor.show();
        this._cursor.ease({x: x-15, y: y-15, duration: 480, mode: Clutter.AnimationMode.EASE_OUT_CUBIC});
    }
    ClickPulse(x, y) {
        if (!this._cursor) return;
        this._cursor.set_position(x-15, y-15);
        this._cursor.show();
        this._cursor.ease({scale_x: 1.7, scale_y: 1.7, duration: 130, mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            onComplete: () => { if (this._cursor) this._cursor.ease({scale_x: 1, scale_y: 1, duration: 130}); }});
    }
    HideCursor() { if (this._cursor) this._cursor.hide(); }
}
