#!/usr/bin/python
"""External portal/PipeWire recorder: exactly two user-selected WINDOW sources.

Never requests monitor capture. No restore token or audio. Stop with SIGINT.
The user must select only Synthetic A and B; portal selection is not an exact
identity attestation. Both original source streams, not a desktop crop, are used.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import time
import uuid

import dbus
from dbus.mainloop.glib import DBusGMainLoop
import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst

parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
if args.output.exists():
    raise SystemExit("refusing to overwrite recording")
args.output.parent.mkdir(parents=True, exist_ok=True)
DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
portal = dbus.Interface(bus.get_object("org.freedesktop.portal.Desktop",
                                      "/org/freedesktop/portal/desktop"),
                        "org.freedesktop.portal.ScreenCast")


def request(method, *parameters, **options):
    token = "cua3506" + uuid.uuid4().hex
    options["handle_token"] = dbus.String(token)
    sender = bus.get_unique_name()[1:].replace(".", "_")
    path = "/org/freedesktop/portal/desktop/request/" + sender + "/" + token
    loop = GLib.MainLoop()
    received = []

    def response(code, results):
        received.append((int(code), results))
        loop.quit()

    match = bus.add_signal_receiver(response, signal_name="Response",
                                    dbus_interface="org.freedesktop.portal.Request", path=path)
    try:
        actual = str(method(*parameters, dbus.Dictionary(options, signature="sv")))
        if actual != path:
            raise RuntimeError("unexpected_portal_request_path")
        loop.run()  # User-owned consent dialog, not automated approval.
    finally:
        match.remove()
    if received[0][0] != 0:
        raise RuntimeError("portal_request_refused_" + str(received[0][0]))
    return received[0][1]


sessions = []
pipeline = None
fds = []
sources = []
try:
    for label in ("A", "B"):
        print("Select only Cua 3506 Synthetic " + label, flush=True)
        session = str(request(portal.CreateSession,
                              session_handle_token="cua3506session" + uuid.uuid4().hex)["session_handle"])
        sessions.append(session)
        request(portal.SelectSources, dbus.ObjectPath(session), types=dbus.UInt32(2),
                multiple=dbus.Boolean(False), cursor_mode=dbus.UInt32(1), persist_mode=dbus.UInt32(0))
        streams = request(portal.Start, dbus.ObjectPath(session), "")["streams"]
        print(json.dumps({"selected_count": len(streams), "source_types": [
            int(properties.get("source_type", 0)) for _, properties in streams]}), flush=True)
        if len(streams) != 1 or any("source_type" in properties and int(properties["source_type"]) != 2
                                   for _, properties in streams):
            raise RuntimeError("requires_one_window_source_per_selection")
        # Some backends omit optional source_type; SelectSources still restricts
        # the offered sources to windows. User selection is not UUID attestation.
        fd = portal.OpenPipeWireRemote(dbus.ObjectPath(session),
                                       dbus.Dictionary({}, signature="sv")).take()
        fds.append(fd)
        sources.append((fd, int(streams[0][0])))
    Gst.init(None)
    # Both sources scaled only after per-window capture; no private monitor pixels.
    pipeline = Gst.parse_launch(
        "compositor name=mix sink_0::xpos=0 sink_1::xpos=640 ! "
        "video/x-raw,width=1280,height=480 ! videoconvert ! "
        "vp8enc deadline=1 cpu-used=4 ! webmmux ! filesink name=output "
        f"pipewiresrc fd={sources[0][0]} path={sources[0][1]} do-timestamp=true ! queue ! "
        "videoconvert ! videoscale ! video/x-raw,width=640,height=480 ! mix.sink_0 "
        f"pipewiresrc fd={sources[1][0]} path={sources[1][1]} do-timestamp=true ! queue ! "
        "videoconvert ! videoscale ! video/x-raw,width=640,height=480 ! mix.sink_1")
    pipeline.get_by_name("output").set_property("location", str(args.output))
    loop = GLib.MainLoop()

    def stop(_signal=None, _frame=None):
        assert pipeline is not None
        pipeline.send_event(Gst.Event.new_eos())

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    gst_bus = pipeline.get_bus()
    gst_bus.add_signal_watch()
    errors = []

    def message(_bus, message):
        if message.type == Gst.MessageType.ERROR:
            error, debug = message.parse_error()
            errors.append(str(error))
            print(json.dumps({"recorder_error": str(error)}), flush=True)
            loop.quit()
        elif message.type == Gst.MessageType.EOS:
            loop.quit()

    gst_bus.connect("message", message)
    pipeline.set_state(Gst.State.PLAYING)
    print(json.dumps({"recorder": "started", "time_ns": time.monotonic_ns(),
                      "sources": "two user-selected windows", "path": str(args.output)}), flush=True)
    loop.run()
    if errors:
        raise RuntimeError("recording_failed")
    print(json.dumps({"recorder": "finished", "time_ns": time.monotonic_ns(),
                      "path": str(args.output)}), flush=True)
finally:
    if pipeline is not None:
        pipeline.set_state(Gst.State.NULL)
    for fd in fds:
        os.close(fd)
    for session in sessions:
        dbus.Interface(bus.get_object("org.freedesktop.portal.Desktop", session),
                       "org.freedesktop.portal.Session").Close()
