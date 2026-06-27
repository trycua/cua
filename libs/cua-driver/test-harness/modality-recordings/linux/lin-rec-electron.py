#!/usr/bin/env python3
# Linux modality recorder for the ELECTRON harness — parity with the Windows
# Electron lane in wpf-recorder.ps1 and the Linux GTK3 lin-rec.py.
# Runs under: xvfb-run -a --server-args="-screen 0 1024x768x24" dbus-run-session -- python3 lin-rec-electron.py MODE
# Electron LEFT, WebKit dashboard RIGHT (per-action held/STOLE + ✓worked/✗no-op verifier).
#
# Differences vs lin-rec.py (GTK):
#  - launches the Electron app (Chromium) instead of lin-harness.py
#  - resolves the harness window by substring "CuaTestHarness Electron" (cdp-suffixed title)
#  - reads BOTH control geometry AND harness state from the web-AX tree
#    (get_window_state capture_mode=ax), the way the Windows recorder reads UIA Text.
import json, os, subprocess, sys, time, glob, re

MODE = sys.argv[1] if len(sys.argv) > 1 else "ax-bg"
HOME = os.path.expanduser("~")
DRV = f"{HOME}/cua/libs/cua-driver/rust/target/release/cua-driver"
ELECTRON_DIR = f"{HOME}/cua/libs/cua-driver/test-harness/apps/cross-platform/electron"
ELECTRON_BIN = f"{ELECTRON_DIR}/node_modules/electron/dist/electron"
DASH = "/tmp/lin-dash.py"
DASH_HTML = "/tmp/lin-dashboard-electron.html"
PORT = 8146
WORK = f"/tmp/cua-lin-electron-{MODE}"
REC = f"{WORK}/rec"
HARW, HARH = 536, 740
PANX, PANW, PANH = 544, 480, 740
WIN_TITLE = "CuaTestHarness Electron"

META = {
 "ax-fg":         {"title":"AX - FOREGROUND","scope":"window","see":"accessibility tree (element-level)","fg":True, "expect":"App kept in FRONT on purpose. Each action runs via the accessibility tree; we measure the foreground."},
 "ax-bg":         {"title":"AX - BACKGROUND","scope":"window","see":"accessibility tree (element-level)","fg":False,"expect":"App should stay in the BACKGROUND. Each action runs via the accessibility tree; we measure which actions steal focus."},
 "vision-fg":     {"title":"VISION - FOREGROUND","scope":"window","see":"screenshot only (pixels)","fg":True, "expect":"Pure pixel-driven, app kept in FRONT. We measure the foreground."},
 "vision-bg":     {"title":"VISION - BACKGROUND","scope":"window","see":"screenshot only (pixels)","fg":False,"expect":"Pure pixel-driven, app should stay in the BACKGROUND. We measure which pixel actions steal focus."},
 "vision-desktop":{"title":"VISION - FULL DESKTOP","scope":"desktop","see":"full-screen screenshot","fg":True,"expect":"Whole-screen, window-less screen-pixel actions (no window targeted)."},
}
m = META[MODE]
VISION = m["see"].startswith("screenshot") or m["scope"] == "desktop"
DESKTOP = m["scope"] == "desktop"

def sh(args, t=10):
    try: return subprocess.run(args, capture_output=True, text=True, timeout=t).stdout
    except Exception: return ""

def D(tool, payload):
    try:
        p = subprocess.run([DRV, "call", tool], input=json.dumps(payload), capture_output=True, text=True, timeout=25)
        return p.stdout
    except Exception:
        return ""

def DJ(tool, payload):
    try: return json.loads(D(tool, payload) or "{}")
    except Exception: return {}

def active_name():
    return sh(["xdotool", "getactivewindow", "getwindowname"]).strip()

# ---------- plan ----------
PLAN = [
 ("click","chk","left-click a checkbox"),
 ("double","btn","double-click a button"),
 ("right","ctx","right-click (context menu)"),
 ("drag","sld","drag the slider"),
 ("scroll","scr","scroll the panel"),
 ("setval","txt","set_value on the text box"),
 ("type","txt","type into the text box"),
 ("key","txt","press a key (Tab)"),
]
if VISION:  PLAN = [p for p in PLAN if p[0] != "setval"]
if DESKTOP: PLAN = [p for p in PLAN if p[0] in ("click","scroll","type","key")]

steps = [{"label":l,"state":"pending","result":"","verified":""} for (_,_,l) in PLAN]
state = {"steals":0,"actions":0}

# ---------- per-action EFFECT verifier: read the web harness's own status labels via web-AX ----------
def hstate():
    E = els()
    joined = " || ".join(str(e.get("label", e.get("name",""))) for e in E)
    h = {}
    mm = re.search(r'agreed=(\w+)', joined);       h["agreed"] = mm.group(1) if mm else None
    mm = re.search(r'slider_value=(\d+)', joined);  h["slider"] = int(mm.group(1)) if mm else 0
    mm = re.search(r'last_action=(\w+)', joined);   h["last_action"] = mm.group(1) if mm else None
    mm = re.search(r'mirror=([^|]*)', joined);      h["mirror"] = (mm.group(1).strip() if mm else "")
    mm = re.search(r'counter=(\d+)', joined);       h["counter"] = int(mm.group(1)) if mm else 0
    mm = re.search(r'scroll_offset=(\d+)', joined);  h["scroll"] = int(mm.group(1)) if mm else 0
    h["_joined"] = joined
    return h

def verify(t, before, after):
    if t == "click":  return "ok" if after.get("agreed") != before.get("agreed") else "fail"
    if t == "double": return "ok" if after.get("last_action") == "double_click" else "fail"
    if t == "right":  return "ok" if after.get("last_action") == "right_click" else "fail"
    if t == "drag":   return "ok" if after.get("slider", 0) > before.get("slider", 0) else "fail"
    if t == "scroll": return "ok" if after.get("scroll", 0) > before.get("scroll", 0) else "fail"  # scroll_offset= now exposed by web-AX
    if t == "setval": return "ok" if "set-by-cua" in str(after.get("mirror", "")) else "fail"
    if t == "type":   return "ok" if "typed-by-cua" in str(after.get("mirror", "")) else "fail"
    return "na"  # press-key (Tab): no observable harness-state effect

def flush():
    fg = active_name()
    af = WIN_TITLE in fg
    st = {"run":m["title"],"expect":m["expect"],"fgmode":m["fg"],"foreground":fg,"appFront":af,
          "steals":state["steals"],"actions":state["actions"],"steps":steps}
    with open(f"{WORK}/status.json","w") as f: json.dump(st, f)

def pulse(sec):
    end = time.time()+sec
    while time.time() < end: flush(); time.sleep(0.15)

# ---------- setup ----------
os.makedirs(REC, exist_ok=True)
subprocess.run("pkill -x electron; pkill -f lin-dash.py", shell=True)
time.sleep(1)
import shutil; shutil.copy(DASH_HTML, f"{WORK}/dashboard.html")
with open(f"{WORK}/status.json","w") as f: json.dump({"run":"","steps":[]}, f)

def dbgcnt(tag):
    try:
        w = json.loads(D("list_windows", {}) or "{}").get("windows", [])
        line = "%s: %d %s\n" % (tag, len(w), [x.get("title") for x in w])
    except Exception as e:
        line = "%s: ERR %s\n" % (tag, e)
    open(f"{WORK}/setup.log","a").write(line)

open(f"{WORK}/setup.log","a").write("DISPLAY=%s XAUTH=%s\n" % (os.environ.get("DISPLAY"), os.environ.get("XAUTHORITY")))
dbgcnt("at start")
D("set_agent_cursor_enabled", {"enabled":True,"session":"d1"})
D("set_agent_cursor_motion", {"session":"d1","cursor_color":"#FF2D2D","cursor_label":"cua-driver","glide_duration_ms":600,"dwell_after_click_ms":700,"idle_hide_ms":120000})
dbgcnt("after cursor")

subprocess.Popen(["python3","-m","http.server",str(PORT),"--directory",WORK], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

# launch Electron — Chromium needs --no-sandbox under Xvfb, --disable-gpu for the
# software path, --force-renderer-accessibility so the web-AX tree populates via AT-SPI.
eenv = dict(os.environ)
eenv["ELECTRON_DISABLE_SECURITY_WARNINGS"] = "1"
elog = open(f"{WORK}/electron.log","w")
subprocess.Popen([ELECTRON_BIN, ELECTRON_DIR, "--no-sandbox", "--disable-gpu",
                  "--force-renderer-accessibility"],
                 cwd=ELECTRON_DIR, env=eenv, stdout=elog, stderr=elog)
time.sleep(10)
dbgcnt("after electron")
subprocess.Popen(["python3", DASH, f"http://127.0.0.1:{PORT}/dashboard.html", str(PANX),"0",str(PANW),str(PANH)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); time.sleep(4)
dbgcnt("after dashboard")
# position windows: harness LEFT, dashboard RIGHT + above
sh(["wmctrl","-r",WIN_TITLE,"-e","0,0,0,%d,%d"%(HARW,HARH)])
sh(["wmctrl","-r","cua-driver-panel","-e","0,%d,0,%d,%d"%(PANX,PANW,PANH)])
sh(["wmctrl","-r","cua-driver-panel","-b","add,above"])
time.sleep(1)

# resolve harness window
w = None
for _ in range(30):
    wins = DJ("list_windows", {}).get("windows", [])
    w = next((x for x in wins if WIN_TITLE in (x.get("title","") or "")), None)
    if w: break
    time.sleep(0.5)
if not w:
    dbg = "DISPLAY=%s\n" % os.environ.get("DISPLAY")
    dbg += "raw_list_windows=%s\n" % (D("list_windows", {})[:2000])
    dbg += "xwininfo=%s\n" % sh(["bash","-c","xwininfo -root -tree 2>/dev/null | grep -iE 'Cua|panel|Electron|harness'"])
    dbg += "electron_log=%s\n" % (open(f"{WORK}/electron.log").read()[-2000:] if os.path.exists(f"{WORK}/electron.log") else "none")
    open(f"{WORK}/debug.log","w").write(dbg)
    open(f"{WORK}/metric.log","w").write("FATAL: no harness window"); sys.exit(1)
WP, WD = w["pid"], w["window_id"]
WB = w.get("bounds") or w.get("frame") or {"x":0,"y":0,"w":HARW,"h":HARH}

def els():
    return DJ("get_window_state", {"pid":WP,"window_id":WD,"capture_mode":"ax"}).get("elements", [])

def rect(el):
    r = el.get("frame") or el.get("bounds") or {}
    return (int(r.get("x",0)), int(r.get("y",0)), int(r.get("w",0)), int(r.get("h",0)))

def find(E, name=None, role=None, anyname=None):
    for e in E:
        nm = str(e.get("label", e.get("name","")))
        rl = str(e.get("role",""))
        if name and name.lower() not in nm.lower(): continue
        if role and role.lower() not in rl.lower(): continue
        if anyname and not any(a.lower() in nm.lower() for a in anyname): continue
        return e
    return None

def find_scroll(E):
    # scroll-tall is the CLIPPED viewport: a 'section' ~260w x ~120h (border/pad → ~270x131
    # in web-AX), distinct from the inner lines container (h~880) and the 23px line rows.
    for e in E:
        if "section" not in str(e.get("role","")).lower(): continue
        _, _, w, h = rect(e)
        if 250 <= w <= 290 and 110 <= h <= 155:
            return e
    return None

E = []
resolve = {}
for _ in range(16):
    E = els()
    resolve = {
        "chk": find(E, role="check") or find(E, name="agree"),
        "btn": find(E, name="Click target") or find(E, name="click target"),
        "ctx": find(E, name="Click target") or find(E, name="click target"),
        "sld": find(E, role="slider"),
        "scr": find_scroll(E),
        "txt": find(E, role="text") or find(E, role="entry") or find(E, name="type here"),
    }
    if resolve["chk"] or resolve["txt"]: break
    time.sleep(0.8)
dump = "\n".join("[%s] role=%r name=%r frame=%r" % (e.get("element_index"), e.get("role"), e.get("label", e.get("name","")), e.get("frame") or e.get("bounds")) for e in E)
open(f"{WORK}/resolve.log","w").write("WIN bounds=%r\nRESOLVE "%WB+" ".join("%s=%s"%(k,bool(v)) for k,v in resolve.items())+" count=%d\n--- elements ---\n%s"%(len(E),dump))

def gcenter(sel):
    el = resolve.get(sel)
    if not el: return None
    x,y,ww,hh = rect(el); return (x+ww//2, y+hh//2)
def gwinlocal(sel):
    el = resolve.get(sel)
    if not el: return None
    x,y,ww,hh = rect(el); return (x-int(WB["x"])+ww//2, y-int(WB["y"])+hh//2)

# seed agent cursor overlay BEFORE recording
D("move_cursor", {"x":HARW-30,"y":HARH-30,"session":"d1"})
time.sleep(0.4)
if DESKTOP: D("set_config", {"key":"capture_scope","value":"desktop"})
D("start_recording", {"output_dir":REC,"record_video":True})
pulse(2)

def do(t, sel):
    el = resolve.get(sel)
    eidx = el.get("element_index") if el else None
    use_ax = (eidx is not None) and (not VISION) and (not DESKTOP)
    c = gcenter(sel); wl = gwinlocal(sel) or (HARW//2, HARH//2)
    if c: D("move_cursor", {"x":c[0],"y":c[1],"session":"d1"}); time.sleep(0.55)
    if t == "click":
        if DESKTOP and c: D("click", {"x":c[0],"y":c[1],"session":"d1"})
        elif use_ax: D("click", {"pid":WP,"window_id":WD,"element_index":eidx,"session":"d1"})
        elif c: D("click", {"pid":WP,"window_id":WD,"x":wl[0],"y":wl[1],"dispatch":("foreground" if m["fg"] else "background"),"session":"d1"})
    elif t == "double":
        if use_ax: D("double_click", {"pid":WP,"window_id":WD,"element_index":eidx,"session":"d1"})
        elif c: D("double_click", {"pid":WP,"window_id":WD,"x":wl[0],"y":wl[1],"session":"d1"})
    elif t == "right":
        if use_ax: D("right_click", {"pid":WP,"window_id":WD,"element_index":eidx,"session":"d1"})
        elif c: D("right_click", {"pid":WP,"window_id":WD,"x":wl[0],"y":wl[1],"session":"d1"})
        time.sleep(0.5); D("press_key", {"pid":WP,"key":"escape","session":"d1"})
    elif t == "drag":
        if el:
            x,y,ww,hh = rect(el); fx=x-int(WB["x"])+8; fy=y-int(WB["y"])+hh//2
            D("drag", {"pid":WP,"window_id":WD,"from_x":fx,"from_y":fy,"to_x":fx+150,"to_y":fy,"session":"d1"})
    elif t == "scroll":
        # aim at the resolved scroll-tall viewport; fall back to window center if unresolved
        sa = c or (HARW//2, HARH//2)          # screen/abs center of scroll-tall
        sl = wl                               # window-local center of scroll-tall
        if DESKTOP: D("scroll", {"x":sa[0],"y":sa[1],"direction":"down","session":"d1"})
        elif VISION: D("scroll", {"pid":WP,"window_id":WD,"x":sl[0],"y":sl[1],"direction":"down","session":"d1"})
        elif use_ax: D("scroll", {"pid":WP,"window_id":WD,"element_index":eidx,"direction":"down","session":"d1"})
        else: D("scroll", {"pid":WP,"window_id":WD,"x":sl[0],"y":sl[1],"direction":"down","session":"d1"})
    elif t == "setval":
        if eidx is not None: D("set_value", {"pid":WP,"window_id":WD,"element_index":eidx,"value":"set-by-cua","session":"d1"})
    elif t == "type":
        if eidx is not None and not VISION:
            D("click", {"pid":WP,"window_id":WD,"element_index":eidx,"session":"d1"}); time.sleep(0.35)
        elif c:
            D("click", {"pid":WP,"window_id":WD,"x":wl[0],"y":wl[1],"dispatch":("foreground" if m["fg"] else "background"),"session":"d1"}); time.sleep(0.35)
        D("type_text", {"pid":WP,"text":"typed-by-cua","session":"d1"})
    elif t == "key":
        D("press_key", {"pid":WP,"key":"tab","session":"d1"})

# ---------- run ----------
for i,(t,sel,label) in enumerate(PLAN):
    steps[i]["state"]="active"; flush()
    before = hstate()
    do(t, sel)
    stole=False; end=time.time()+1.5
    while time.time()<end:
        flush()
        if WIN_TITLE in active_name(): stole=True
        time.sleep(0.11)
    steps[i]["verified"] = verify(t, before, hstate())
    state["actions"]+=1
    if m["fg"]: steps[i]["result"]="front"
    elif stole: state["steals"]+=1; steps[i]["result"]="stole"
    else: steps[i]["result"]="held"
    steps[i]["state"]="done"; flush()
    sh(["wmctrl","-r","cua-driver-panel","-b","add,above"])

pulse(2.5)
if DESKTOP: D("set_config", {"key":"capture_scope","value":"window"})
D("stop_recording", {}); time.sleep(3)
subprocess.run("pkill -x electron; pkill -f lin-dash.py; pkill -f 'http.server 8146'; pkill cua-driver; pkill openbox", shell=True)
mp4 = (glob.glob(f"{REC}/**/*.mp4", recursive=True) or [None])[0]
size = os.path.getsize(mp4) if mp4 else 0
worked = sum(1 for s in steps if s.get("verified")=="ok"); ver = sum(1 for s in steps if s.get("verified") in ("ok","fail"))
verdict = ("foreground-mode, %d actions"%state["actions"]) if m["fg"] else ("%d/%d stole focus"%(state["steals"],state["actions"]))
line = "MODE=%s MEASURE=%s EFFECTS=%d/%d_landed MP4=%s SIZE=%d" % (MODE, verdict, worked, ver, mp4, size)
open(f"{WORK}/metric.log","w").write(line); print(line)
