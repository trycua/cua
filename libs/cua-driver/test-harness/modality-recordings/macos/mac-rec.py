#!/usr/bin/env python3
# macOS AppKit modality recorder — parity with the Linux GTK3 / Windows WPF golden recorders.
# Harness LEFT (CuaTestHarness.AppKit), Chrome --app dashboard RIGHT.
# Per-action: no-foreground contract (held / STOLE) + effect verifier (worked / no-op).
import json, os, subprocess, sys, time, re, glob, signal

MODE = sys.argv[1] if len(sys.argv) > 1 else "ax-bg"
SURFACE = sys.argv[2] if len(sys.argv) > 2 else "appkit"
HOME = os.path.expanduser("~")
DRV = f"{HOME}/cua/libs/cua-driver/rust/target/release/cua-driver"
DASH_HTML = "/tmp/mac-dashboard.html"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PORT = 8147

# ---------- per-surface configuration ----------
SURF = {
 "appkit": {
   "bundle":"com.trycua.harness.appkit", "title":"CuaTestHarness AppKit",
   "dash":"AppKit", "appmatch":"CuaTestHarness",
   # window-local POINTS (measured live against the rebuilt parity harness, window 700x1112).
   "coords":{"click_target":(129.0,304.0),"slider":(180.0,372.0),"scroll":(279.0,791.0),
             "txt":(140.0,232.0),"increment":(62.0,96.0)},
 },
 "swiftui": {
   "bundle":"com.trycua.harness.swiftui", "title":"CuaTestHarness SwiftUI",
   "dash":"SwiftUI", "appmatch":"CuaTestHarness",
   # window-local POINTS (measured live against the rebuilt parity harness, window 700x820).
   "coords":{"click_target":(179.0,348.0),"slider":(230.0,424.0),"scroll":(210.0,690.0),
             "txt":(190.0,268.0),"increment":(112.0,100.0),"popover":(125.0,904.0)},
 },
 "electron": {
   "bundle":None, "launch_match":"CuaTestHarness Electron", "title":"CuaTestHarness Electron",
   "dash":"Electron", "appmatch":"Electron",
   "coords":{"click_target":(167.0,459.0),"slider":(45.0,367.0),"txt":(106.0,282.0),
             "scroll":(340.0,400.0),"increment":(70.0,197.0)},
 },
 "wkwebview": {  # Apple WebKit (NOT Chromium); same shared web DOM as Electron
   "bundle":"com.trycua.harness.wkwebview", "title":"CuaTestHarness WKWebView",
   "dash":"WKWebView", "appmatch":"CuaTestHarness",
   "coords":{"click_target":(167.0,459.0),"slider":(45.0,367.0),"txt":(106.0,282.0),
             "scroll":(340.0,400.0),"increment":(70.0,197.0)},
 },
}
WEBISH = ("electron", "wkwebview")  # web-AX harnesses sharing the same DOM/verifier
S = SURF[SURFACE]
BUNDLE = S["bundle"]; TITLE = S["title"]; APPMATCH = S["appmatch"]
WORK = f"/tmp/cua-mac-{SURFACE}-{MODE}"
REC = f"{WORK}/rec"

# window layout (points). screen ~1512x982, menu bar ~33.
HX, HY = 0, 40                 # harness top-left
DASH_X, DASH_Y = 712, 40      # dashboard top-left
DASH_W, DASH_H = 792, 900

META = {
 "ax-fg":         {"title":"AX - FOREGROUND","see":"accessibility tree (element-level)","fg":True, "expect":"App kept in FRONT on purpose. Each action runs via the accessibility tree / element_index; we measure the foreground."},
 "ax-bg":         {"title":"AX - BACKGROUND","see":"accessibility tree (element-level)","fg":False,"expect":"App should stay in the BACKGROUND. Each action runs via the accessibility tree / element_index; we measure which actions steal focus."},
 "vision-fg":     {"title":"VISION - FOREGROUND","see":"screenshot only (pixels)","fg":True, "expect":"Pure pixel-driven, app kept in FRONT. We measure the foreground."},
 "vision-bg":     {"title":"VISION - BACKGROUND","see":"screenshot only (pixels)","fg":False,"expect":"Pure pixel-driven, app should stay in the BACKGROUND. We measure which pixel actions steal focus."},
 "vision-desktop":{"title":"VISION - FULL DESKTOP","see":"full-screen screenshot","fg":True,"expect":"capture_scope=desktop (full-display capture). Pure pixel-driven, app in FRONT. On macOS pixel dispatch is window-anchored, so actions route to the on-screen window."},
}
m = META[MODE]
VISION = m["see"].startswith("screenshot")
DESKTOP = MODE == "vision-desktop"
SESS = f"macd-{MODE}-{int(time.time())}"

def D(tool, payload, t=25):
    try:
        p = subprocess.run([DRV, "call", tool], input=json.dumps(payload),
                           capture_output=True, text=True, timeout=t)
        return p.stdout
    except Exception:
        return ""

def DJ(tool, payload, t=25):
    try: return json.loads(D(tool, payload, t) or "{}")
    except Exception: return {}

def osa(script, t=6):
    try: return subprocess.run(["osascript","-e",script], capture_output=True, text=True, timeout=t).stdout.strip()
    except Exception: return ""

def frontmost():
    return osa('tell application "System Events" to get name of first application process whose frontmost is true')

def app_is_front():
    return APPMATCH in frontmost()

def activate_harness():
    osa(f'tell application "System Events" to set frontmost of (first process whose unix id is {PID}) to true')

def activate_dash():
    osa('tell application "Google Chrome" to activate')

def position_harness():
    # pin position AND size: window height otherwise varies (832/858) between launches,
    # which shifts the screenshot dims and breaks pixel-coord calibration.
    osa(f'''tell application "System Events"
      set p to first process whose unix id is {PID}
      set w to first window of p whose title contains "{TITLE}"
      set position of w to {{{HX}, {HY}}}
      set size of w to {{700, 820}}
    end tell''')

# ---------- coords (window-local screenshot-px; window 700x858 -> shot 1279x1568) ----------
# Targets stored as WINDOW-LOCAL POINTS (stable across window-height drift, top-anchored
# layout). Converted to live screenshot-pixels at runtime: px = pt * shot_dim / win_dim.
# Calibrated against a pinned 700x820 window (shot 1339x1568).
WIN_W = 700.0
COORDS_PT = S["coords"]
# live dims, measured right before recording
SHOT_W, SHOT_H, WIN_H = 1339.0, 1568.0, 820.0
def coord(sel):
    px, py = COORDS_PT[sel]
    return (px * SHOT_W / WIN_W, py * SHOT_H / WIN_H)

# ---------- plan (per surface) ----------
PLANS = {
 "appkit": [
   ("click","click_target","left-click the click-target"),
   ("double","click_target","double-click the click-target"),
   ("right","click_target","right-click the click-target"),
   ("drag","click_target","drag across the click-target"),
   ("scroll","scroll","scroll the list"),
   ("setval","txt","set_value on the text box"),
   ("type","txt","type into the text box"),
   ("key","txt","press a key (Tab)"),
 ],
 "swiftui": [
   ("click","click_target","left-click the click-target"),
   ("double","click_target","double-click the click-target"),
   ("right","click_target","right-click the click-target"),
   ("drag","slider","drag the slider"),
   ("scroll","scroll","scroll the list"),
   ("setval","txt","set_value on the text box"),
   ("type","txt","type into the text box"),
   ("key","txt","press a key (Tab)"),
 ],
 "electron": [
   ("click","click_target","left-click the click-target"),
   ("double","click_target","double-click the click-target"),
   ("right","click_target","right-click the click-target"),
   ("drag","slider","drag the slider"),
   ("scroll","scroll","scroll the page"),
   ("setval","txt","set_value on the text box"),
   ("type","txt","type into the text box"),
   ("key","txt","press a key (Tab)"),
 ],
}
PLANS["wkwebview"] = PLANS["electron"]
PLAN = list(PLANS[SURFACE])
if VISION:  PLAN = [p for p in PLAN if p[0] != "setval"]
if DESKTOP: PLAN = [p for p in PLAN if p[0] in ("click","scroll","type","key")]

steps = [{"label":l,"state":"pending","result":"","verified":""} for (_,_,l) in PLAN]
state = {"steals":0,"actions":0}

# ---------- harness state reader (parse AX tree_markdown) ----------
def hstate():
    d = DJ("get_window_state", {"pid":PID,"window_id":WID,"capture_mode":"ax"})
    tm = d.get("tree_markdown","") or ""
    if SURFACE in WEBISH:
        # web harness exposes plain-text status labels (counter=N, mirror=..., etc.)
        st = {"last":"none","clicks":0,"slider":0,"counter":0,"txt":""}
        m1 = re.search(r"last_action=(\w+)", tm);  st["last"] = m1.group(1) if m1 else "none"
        m2 = re.search(r"clicks=(\d+)", tm);       st["clicks"] = int(m2.group(1)) if m2 else 0
        m3 = re.search(r"slider_value=(\d+)", tm); st["slider"] = int(m3.group(1)) if m3 else 0
        m4 = re.search(r"counter=(\d+)", tm);      st["counter"] = int(m4.group(1)) if m4 else 0
        m5 = re.search(r"mirror=([^\"]*)\"", tm);  st["txt"] = m5.group(1) if m5 else ""
        return st
    # NATIVE (appkit/swiftui): rebuilt parity harness exposes the SAME web-style status
    # labels in the AX tree_markdown — last_action=, clicks=, slider_value=, scroll_offset=,
    # agreed=, menu_action=. Text mirror is the AXTextField value (id=txt-input).
    st = {"last":"none","clicks":0,"slider":0,"scroll":0,"menu":"none","agreed":"false","txt":""}
    m1 = re.search(r"last_action=(\w+)", tm);    st["last"]   = m1.group(1) if m1 else "none"
    m2 = re.search(r"clicks=(\d+)", tm);         st["clicks"] = int(m2.group(1)) if m2 else 0
    m3 = re.search(r"slider_value=(\d+)", tm);   st["slider"] = int(m3.group(1)) if m3 else 0
    m4 = re.search(r"scroll_offset=(\d+)", tm);  st["scroll"] = int(m4.group(1)) if m4 else 0
    m5 = re.search(r"menu_action=([^\"]+)\"", tm); st["menu"] = m5.group(1) if m5 else "none"
    m6 = re.search(r"agreed=(\w+)", tm);         st["agreed"] = m6.group(1) if m6 else "false"
    # txt-input value (placeholder "Type here…" when empty — substring checks below won't match it)
    tv = re.search(r'AXTextField = "([^"]*)" \[id=txt-input', tm) or re.search(r'AXTextField = "([^"]*)"', tm)
    if tv: st["txt"] = tv.group(1)
    return st

AXMODE = (not VISION) and (not DESKTOP)
def verify(t, b, a):
    if SURFACE in WEBISH:
        if t == "click":  return "ok" if ("left_click" in a["last"] and a["clicks"]>b["clicks"]) else "fail"
        if t == "double": return "ok" if "double_click" in a["last"] else "fail"
        if t == "right":  return "ok" if "right_click" in a["last"] else "fail"
        if t == "drag":   return "ok" if a["slider"]>b["slider"] else "fail"
        if t == "setval": return "ok" if "set-by-cua" in a["txt"] else "fail"
        if t == "type":   return "ok" if "typed-by-cua" in a["txt"] else "fail"
        return "na"   # scroll/key: no verifiable readout
    # NATIVE appkit/swiftui — real parity signals from the click-target / slider /
    # scroll-target / text box (matches the WEBISH verifier; only the readouts differ).
    if t == "click":  return "ok" if (a["clicks"]>b["clicks"] or a["last"] in ("click","left_click")) else "fail"
    # AppKit's click-target reports last_action=double_click for a pixel double; SwiftUI's
    # does not distinguish it, so a landed double shows up as clicks incrementing (+2).
    if t == "double": return "ok" if ("double" in a["last"] or a["clicks"]>b["clicks"]) else "fail"
    # right-click: last_action=right_click (AppKit) or a context menu_action change.
    if t == "right":  return "ok" if ("right" in a["last"] or a["menu"]!=b["menu"]) else "fail"
    if t == "drag":   return "ok" if a["slider"]>b["slider"] else "fail"
    if t == "scroll": return "ok" if a["scroll"]>b["scroll"] else "fail"
    if t == "setval": return "ok" if "set-by-cua" in a["txt"] else "fail"
    if t == "type":   return "ok" if "typed-by-cua" in a["txt"] else "fail"
    return "na"   # key: no verifiable readout

# ---------- status.json writer for dashboard ----------
_front_cache = {"v":"", "t":0}
def flush(refresh_front=True):
    if refresh_front:
        _front_cache["v"] = frontmost()
    fg = _front_cache["v"]
    af = APPMATCH in fg
    st = {"run":m["title"],"expect":m["expect"],"fgmode":m["fg"],"foreground":fg,"appFront":af,
          "steals":state["steals"],"actions":state["actions"],"steps":steps}
    with open(f"{WORK}/status.json","w") as f: json.dump(st, f)

def pulse(sec):
    end = time.time()+sec
    while time.time() < end:
        flush(); time.sleep(0.18)

# ---------- setup ----------
os.makedirs(REC, exist_ok=True)
subprocess.run("pkill -f mac-dashboard; pkill -f 'http.server 8147'", shell=True)
subprocess.run(f"pkill -f CuaTestHarness", shell=True)
if SURFACE == "electron":
    subprocess.run("pkill -f 'electron/dist/Electron'; pkill -f 'Electron.app/Contents/MacOS/Electron'", shell=True)
time.sleep(2.0)
# dashboard.html with per-surface title suffix (h1 "... - AppKit" -> "... - <surface>")
_html = open(DASH_HTML).read().replace("single-modality run - AppKit", f"single-modality run - {S['dash']}")
open(f"{WORK}/dashboard.html","w").write(_html)
with open(f"{WORK}/status.json","w") as f: json.dump({"run":"","steps":[]}, f)
log = open(f"{WORK}/setup.log","w")
def L(s): log.write(s+"\n"); log.flush()

# http server for dashboard
subprocess.Popen(["python3","-m","http.server",str(PORT),"--directory",WORK],
                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1)

# launch harness
if BUNDLE:
    la = DJ("launch_app", {"bundle_id":BUNDLE})
    PID = la.get("pid")
    wins = la.get("windows", [])
    main = next((w for w in wins if TITLE in (w.get("title") or "")), None)
    WID = main["window_id"] if main else (wins[0]["window_id"] if wins else None)
else:
    # electron: launched out-of-band (see ELECTRON_CMD); poll list_windows by title substring
    if os.environ.get("ELECTRON_CMD"):
        subprocess.Popen(os.environ["ELECTRON_CMD"], shell=True,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    PID = WID = None
    for _ in range(40):
        for w in DJ("list_windows", {}).get("windows", []):
            if S["launch_match"] in (w.get("title") or ""):
                PID, WID = w["pid"], w["window_id"]; break
        if PID: break
        time.sleep(0.5)
L(f"launched pid={PID} wid={WID}")
if not PID or not WID:
    L("FATAL no harness"); open(f"{WORK}/metric.log","w").write("FATAL no harness"); sys.exit(1)

# make harness visible (recording carve-out) + position LEFT
activate_harness(); time.sleep(0.6)
position_harness(); time.sleep(0.4)
# refresh window id (re-list, pick on-screen main window)
wl = DJ("list_windows", {"pid":PID}).get("windows", [])
mm = next((w for w in wl if TITLE in (w.get("title") or "") and w.get("is_on_screen")), None)
if mm: WID = mm["window_id"]; WIN_ORIGIN = (mm["bounds"]["x"], mm["bounds"]["y"])
else:  WIN_ORIGIN = (HX, HY)
L(f"after position wid={WID} origin={WIN_ORIGIN} onscreen_main={bool(mm)}")

# launch Chrome --app dashboard, positioned RIGHT
prof = f"/tmp/cua-dash-profile-{SURFACE}-{MODE}"
subprocess.Popen([CHROME, f"--app=http://127.0.0.1:{PORT}/dashboard.html",
                  f"--user-data-dir={prof}", "--no-first-run", "--no-default-browser-check",
                  f"--window-position={DASH_X},{DASH_Y}", f"--window-size={DASH_W},{DASH_H}"],
                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(4)
# re-position dashboard via System Events as a backstop
osa(f'''tell application "System Events"
  set cp to first process whose name is "Google Chrome"
  try
    set position of front window of cp to {{{DASH_X}, {DASH_Y}}}
    set size of front window of cp to {{{DASH_W}, {DASH_H}}}
  end try
end tell''')
time.sleep(1)
L(f"frontmost after chrome: {frontmost()}")

# resolve element indices FRESH from a snapshot (first occurrence = primary window subtree).
# Re-resolved right before each AX action to honour the snapshot-before-action invariant.
def axidx():
    d = DJ("get_window_state", {"pid":PID,"window_id":WID,"capture_mode":"ax"})
    tm = d.get("tree_markdown","") or ""
    idx = {}
    for mt in re.finditer(r'\[(\d+)\] (AX\w+)[^\n]*?\[id=([\w-]+)', tm):
        if mt.group(3) not in idx: idx[mt.group(3)] = int(mt.group(1))
    ta = re.search(r'\[(\d+)\] AXTextArea', tm)
    if ta: idx["scroll-area"] = int(ta.group(1))
    # role/label fallbacks (Electron web-AX exposes no [id=...])
    if "txt-input" not in idx:
        mt = re.search(r'\[(\d+)\] AXTextField', tm)
        if mt: idx["txt-input"] = int(mt.group(1))
    if "slider" not in idx:
        mt = re.search(r'\[(\d+)\] AXSlider', tm)
        if mt: idx["slider"] = int(mt.group(1))
    if "btn-increment" not in idx:
        mt = re.search(r'\[(\d+)\] AXButton "Increment"', tm)
        if mt: idx["btn-increment"] = int(mt.group(1))
    return idx

IDX0 = axidx()
open(f"{WORK}/resolve.log","w").write("IDX="+json.dumps(IDX0))
L(f"resolved idx={IDX0}")

# agent cursor
D("start_session", {"session":SESS})
D("set_agent_cursor_enabled", {"enabled":True,"session":SESS})
D("set_agent_cursor_motion", {"session":SESS,"cursor_color":"#FF2D2D","cursor_label":"cua-driver",
                              "glide_duration_ms":600,"dwell_after_click_ms":650,"idle_hide_ms":120000})

# re-pin size and measure LIVE screenshot dims + window height (drift-proof coords)
position_harness(); time.sleep(0.6)
ws = DJ("get_window_state", {"pid":PID,"window_id":WID,"capture_mode":"som"})
if ws.get("screenshot_width"):  SHOT_W = float(ws["screenshot_width"])
if ws.get("screenshot_height"): SHOT_H = float(ws["screenshot_height"])
wl2 = DJ("list_windows", {"pid":PID}).get("windows", [])
mm2 = next((w for w in wl2 if w.get("window_id")==WID), None)
if mm2:
    WIN_H = float(mm2["bounds"]["height"]); WIN_ORIGIN = (mm2["bounds"]["x"], mm2["bounds"]["y"])
L(f"live dims shot={SHOT_W}x{SHOT_H} win_h={WIN_H} origin={WIN_ORIGIN}")

# foreground/background precondition
if m["fg"]: activate_harness()
else:       activate_dash()
time.sleep(0.8)

# seed cursor on-screen before recording (so AX actions glide visibly)
D("move_cursor", {"pid":PID,"window_id":WID,"x":480,"y":760,"session":SESS})
time.sleep(0.4)


if DESKTOP:
    D("set_config", {"key":"capture_scope","value":"desktop"})
D("start_recording", {"output_dir":REC,"record_video":True})
time.sleep(0.5)
pulse(2.0)

# ---------- action dispatch ----------
def to_screen(px, py):
    # screenshot-px -> screen points
    return (WIN_ORIGIN[0] + px*WIN_W/SHOT_W, WIN_ORIGIN[1] + py*WIN_H/SHOT_H)

def glide(sel):
    px, py = coord(sel)
    if DESKTOP:
        sx, sy = to_screen(px, py)
        D("move_cursor", {"x":sx,"y":sy,"session":SESS})
    else:
        D("move_cursor", {"pid":PID,"window_id":WID,"x":px,"y":py,"session":SESS})
    time.sleep(0.5)

def do_electron(t, sel):
    # rich web harness: click-target span (pixel; no AX press) distinguishes
    # left/right/double; slider for drag; txt-input + status labels for verify.
    IDX = axidx() if AXMODE else {}
    txt = IDX.get("txt-input")
    cpx, cpy = coord("click_target"); spx, spy = coord("slider")
    if t == "click":
        glide("click_target"); D("click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"session":SESS})
    elif t == "double":
        glide("click_target"); D("double_click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"count":2,"session":SESS})
    elif t == "right":
        glide("click_target"); D("right_click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"session":SESS})
    elif t == "drag":
        glide("slider"); D("drag", {"pid":PID,"window_id":WID,"from_x":spx,"from_y":spy,"to_x":spx+220,"to_y":spy,"session":SESS})
    elif t == "scroll":
        glide("scroll"); rpx, rpy = coord("scroll")
        D("scroll", {"pid":PID,"window_id":WID,"x":rpx,"y":rpy,"direction":"down","amount":5,"session":SESS})
    elif t == "setval":
        glide("txt")
        if txt is not None: D("set_value", {"pid":PID,"window_id":WID,"element_index":txt,"value":"set-by-cua","session":SESS})
    elif t == "type":
        glide("txt"); tpx, tpy = coord("txt")
        if AXMODE and txt is not None:
            D("set_value", {"pid":PID,"window_id":WID,"element_index":txt,"value":"","session":SESS}); time.sleep(0.4)
            D("click", {"pid":PID,"window_id":WID,"element_index":txt,"session":SESS}); time.sleep(0.3)
            D("type_text", {"pid":PID,"window_id":WID,"element_index":txt,"text":"typed-by-cua","session":SESS})
        else:
            D("click", {"pid":PID,"window_id":WID,"x":tpx,"y":tpy,"session":SESS}); time.sleep(0.3)
            D("type_text", {"pid":PID,"text":"typed-by-cua","session":SESS})
    elif t == "key":
        D("press_key", {"pid":PID,"key":"tab","session":SESS})

def do_native(t, sel):
    # appkit / swiftui — rebuilt parity controls (click-target / slider / scroll-target /
    # text box), mirroring the electron plan. AX press lands the click-target's primary
    # action (last_action=click, works backgrounded). double/right/drag are real
    # mouse-button gestures the view records via pixel events. scroll: AppKit exposes an
    # AXTextArea (id=scroll-tall) addressable by element_index; SwiftUI's nested ScrollView
    # is pixel-only. type: a PIXEL click grabs first responder (AX-press on a textfield does
    # not), then keyboard type_text — set_value clears first in AX modes.
    IDX = axidx() if AXMODE else {}
    ct  = IDX.get("btn-clicktarget"); txt = IDX.get("txt-input")
    sa  = IDX.get("scroll-tall") or IDX.get("scroll-area")  # AppKit AXTextArea (multiline value hides its [id=])
    cpx, cpy = coord("click_target"); spx, spy = coord("slider"); tpx, tpy = coord("txt")
    if t == "click":
        glide("click_target")
        if AXMODE and ct is not None: D("click", {"pid":PID,"window_id":WID,"element_index":ct,"session":SESS})
        else: D("click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"session":SESS})
    elif t == "double":
        glide("click_target"); D("double_click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"count":2,"session":SESS})
    elif t == "right":
        glide("click_target"); D("right_click", {"pid":PID,"window_id":WID,"x":cpx,"y":cpy,"session":SESS})
        D("press_key", {"pid":PID,"key":"escape","session":SESS})  # dismiss any context menu
    elif t == "drag":
        glide("slider"); D("drag", {"pid":PID,"window_id":WID,"from_x":spx,"from_y":spy,"to_x":spx+220,"to_y":spy,"session":SESS})
    elif t == "scroll":
        glide("scroll"); rpx, rpy = coord("scroll")
        if AXMODE and sa is not None:
            D("scroll", {"pid":PID,"window_id":WID,"element_index":sa,"direction":"down","amount":5,"session":SESS})
        else:
            D("scroll", {"pid":PID,"window_id":WID,"x":rpx,"y":rpy,"direction":"down","amount":8,"session":SESS})
    elif t == "setval":
        glide("txt")
        if AXMODE and txt is not None:
            D("set_value", {"pid":PID,"window_id":WID,"element_index":txt,"value":"set-by-cua","session":SESS})
    elif t == "type":
        glide("txt")
        if AXMODE and txt is not None:
            D("set_value", {"pid":PID,"window_id":WID,"element_index":txt,"value":"","session":SESS}); time.sleep(0.4)
        D("click", {"pid":PID,"window_id":WID,"x":tpx,"y":tpy,"session":SESS}); time.sleep(0.4)
        D("type_text", {"pid":PID,"text":"typed-by-cua","session":SESS})
    elif t == "key":
        D("press_key", {"pid":PID,"key":"tab","session":SESS})

def do(t, sel):
    if SURFACE in WEBISH:
        return do_electron(t, sel)
    return do_native(t, sel)

# ---------- run ----------
for i,(t,sel,label) in enumerate(PLAN):
    steps[i]["state"]="active"; flush()
    if not m["fg"]:
        activate_dash(); time.sleep(0.4)   # re-assert background precondition
    before = hstate()
    do(t, sel)
    stole=False; end=time.time()+1.6
    while time.time()<end:
        flush()
        if app_is_front(): stole=True
        time.sleep(0.12)
    after = hstate()
    if SURFACE in WEBISH and verify(t, before, after) == "fail":
        # Chromium web-AX status text can lag the DOM by >1.5s, esp. backgrounded;
        # re-poll so a landed-but-late effect isn't mis-scored as a no-op.
        for _ in range(7):
            time.sleep(0.4); after = hstate()
            if verify(t, before, after) != "fail": break
    steps[i]["verified"] = verify(t, before, after)
    if t == "popover":   # dismiss so it doesn't poison later snapshots
        D("press_key", {"pid":PID,"key":"escape","session":SESS}); time.sleep(0.3)
    state["actions"]+=1
    if m["fg"]: steps[i]["result"]="front"
    elif stole: state["steals"]+=1; steps[i]["result"]="stole"
    else: steps[i]["result"]="held"
    steps[i]["state"]="done"; flush()
    L(f"step {t}: result={steps[i]['result']} verified={steps[i]['verified']} before={before} after={after}")

pulse(2.5)
if DESKTOP:
    D("set_config", {"key":"capture_scope","value":"window"})
D("stop_recording", {}); time.sleep(3)
D("end_session", {"session":SESS})
subprocess.run("pkill -f mac-dashboard; pkill -f 'http.server 8147'", shell=True)
subprocess.run(f"pkill -f 'user-data-dir=/tmp/cua-dash-profile-{SURFACE}-{MODE}'", shell=True)
if SURFACE == "electron":
    subprocess.run("pkill -f 'Electron.app/Contents/MacOS/Electron'", shell=True)

mp4 = (glob.glob(f"{REC}/**/*.mp4", recursive=True) or [None])[0]
size = os.path.getsize(mp4) if mp4 else 0
worked = sum(1 for s in steps if s.get("verified")=="ok")
ver = sum(1 for s in steps if s.get("verified") in ("ok","fail"))
verdict = ("foreground-mode, %d actions"%state["actions"]) if m["fg"] else ("%d/%d stole focus"%(state["steals"],state["actions"]))
line = "MODE=%s MEASURE=%s EFFECTS=%d/%d_landed MP4=%s SIZE=%d" % (MODE, verdict, worked, ver, mp4, size)
open(f"{WORK}/metric.log","w").write(line); print(line)
