#!/usr/bin/python
"""Supported KWin scripting activation observation; never dispatches input.

The callback is a private D-Bus observation, not a log or input capability.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import uuid

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

SERVICE = "org.cua.KWinTarget"
PATH = "/org/cua/KWinTarget"


def exact_record(snapshot, pid, token, generation, internal_id=None):
    if snapshot.get("generation") != generation:
        raise ValueError("helper_generation_changed")
    uuid.UUID(generation.strip("{}"))
    records = snapshot["windows"]
    if len({r["token"] for r in records}) != len(records):
        raise ValueError("ambiguous_tokens")
    if len({r["internal_id"] for r in records}) != len(records):
        raise ValueError("ambiguous_internal_ids")
    matches = [r for r in records if r["pid"] == pid and r["token"] == token]
    if len(matches) != 1:
        raise ValueError("exact_target_missing_or_ambiguous")
    target = matches[0]
    uuid.UUID(target["internal_id"].strip("{}"))
    if internal_id is not None and target["internal_id"] != internal_id:
        raise ValueError("internal_identity_changed")
    if target["minimized"]:
        raise ValueError("minimized_target")
    return target


def trusted_snapshot(bus):
    owner = str(bus.get_name_owner(SERVICE))
    if owner != str(bus.get_name_owner("org.kde.KWin")):
        raise ValueError("helper_owner_not_kwin")
    dbus_api = dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus"),
                              "org.freedesktop.DBus")
    pid = int(dbus_api.GetConnectionUnixProcessID(owner))
    if int(dbus_api.GetConnectionUnixUser(owner)) != os.getuid():
        raise ValueError("helper_wrong_uid")
    if Path(f"/proc/{pid}/comm").read_text().strip() != "kwin_wayland":
        raise ValueError("helper_wrong_process")
    api = dbus.Interface(bus.get_object(owner, PATH), SERVICE)
    if int(api.GetVersion()) != 1:
        raise ValueError("v1_discovery_incompatible")
    snapshot = json.loads(str(api.GetIdentitySnapshot()))
    if owner != str(bus.get_name_owner(SERVICE)):
        raise ValueError("helper_owner_changed")
    return owner, pid, snapshot


def observe(bus, pid, token, generation, activate=True, timeout_ms=1000,
            drop_confirmation=False):
    owner, kwin_pid, snapshot = trusted_snapshot(bus)
    target = exact_record(snapshot, pid, token, generation)
    nonce = uuid.uuid4().hex
    started = time.monotonic_ns()
    loop = GLib.MainLoop()
    replies = []

    class Receiver(dbus.service.Object):
        @dbus.service.method("org.cua.ProofObserver", in_signature="ss", out_signature="",
                             sender_keyword="sender")
        def Observed(self, received_nonce, payload, sender=None):
            if str(sender) != owner or str(received_nonce) != nonce:
                return
            if drop_confirmation:
                return
            replies.append({"received_ns": time.monotonic_ns(), **json.loads(str(payload))})
            loop.quit()

    receiver = Receiver(bus, "/Proof" + nonce)
    parameters = json.dumps({"uuid": target["internal_id"], "pid": pid,
                             "activate": activate, "nonce": nonce,
                             "destination": bus.get_unique_name(), "path": "/Proof" + nonce})
    script = """const p = PARAMETERS;
let notifications = 0;
workspace.windowActivated.connect(function(window) { notifications += 1; });
const matches = workspace.windowList().filter(function(window) {
    return String(window.internalId) === p.uuid && Number(window.pid) === p.pid;
});
let requested = false;
if (matches.length === 1 && p.activate) {
    workspace.activeWindow = matches[0];
    requested = true;
}
const active = workspace.activeWindow;
callDBus(p.destination, p.path, "org.cua.ProofObserver", "Observed", p.nonce,
    JSON.stringify({matches: matches.length, requested: requested,
        confirmed: matches.length === 1 && active === matches[0] &&
            String(active.internalId) === p.uuid && Number(active.pid) === p.pid,
        active_uuid: active ? String(active.internalId) : null,
        active_pid: active ? Number(active.pid) : null,
        activation_notifications: notifications}));
""".replace("PARAMETERS", parameters)
    scripting = dbus.Interface(bus.get_object(owner, "/Scripting"), "org.kde.kwin.Scripting")
    name = "cua-3506-proof-" + nonce
    deadline = GLib.timeout_add(timeout_ms, lambda: (loop.quit(), False)[1])
    try:
        with tempfile.TemporaryDirectory(prefix="cua3506-script-") as directory:
            source = Path(directory) / "activate.js"
            source.write_text(script)
            script_id = int(scripting.loadScript(str(source), name, signature="ss"))
            if script_id < 0:
                raise ValueError("script_load_failed")
            # Installed introspection verifies this per-script path and run method.
            loaded = dbus.Interface(bus.get_object(owner, f"/Scripting/Script{script_id}"),
                                    "org.kde.kwin.Script")
            # Asynchronous D-Bus call: the KWin callback needs this process's GLib loop.
            errors = []
            loaded.run(reply_handler=lambda: None,
                       error_handler=lambda e: (errors.append(str(e)), loop.quit()))
            loop.run()
            if errors:
                raise ValueError("script_run_failed: " + errors[0])
    finally:
        if GLib.MainContext.default().find_source_by_id(deadline) is not None:
            GLib.source_remove(deadline)
        scripting.unloadScript(name)
        if bool(scripting.isScriptLoaded(name)):
            raise RuntimeError("temporary_script_not_unloaded")
        receiver.remove_from_connection()
    if not replies:
        raise ValueError("confirmation_timeout_zero_input")
    latest_owner, _, latest = trusted_snapshot(bus)
    if latest_owner != owner:
        raise ValueError("owner_changed_after_observation")
    fresh = exact_record(latest, pid, token, generation, target["internal_id"])
    result = replies[-1]
    result.update(started_ns=started, finished_ns=time.monotonic_ns(),
                  helper_owner=owner, kwin_pid=kwin_pid, generation=generation,
                  token=token, pid=pid, internal_id=target["internal_id"],
                  fresh_active=bool(fresh["active"]))
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--token", type=int, required=True)
    parser.add_argument("--generation", required=True)
    parser.add_argument("--read-only", action="store_true")
    parser.add_argument("--drop-confirmation", action="store_true")
    args = parser.parse_args()
    DBusGMainLoop(set_as_default=True)
    print(json.dumps(observe(dbus.SessionBus(), args.pid, args.token, args.generation,
                             not args.read_only, drop_confirmation=args.drop_confirmation)))
