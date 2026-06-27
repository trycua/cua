#!/usr/bin/env python3
# Linux modality recorder — parity with the Windows wpf-v7 golden recorder.
# Runs under: xvfb-run -a --server-args="-screen 0 1024x768x24" dbus-run-session -- python3 lin-rec.py MODE
# Produces one single-modality recording: GTK3 harness LEFT, WebKit dashboard RIGHT
# (per-action held/STOLE measurement + live fg/bg indicator).
import json, os, subprocess, sys, time, signal, glob

MODE = sys.argv[1] if len(sys.argv) > 1 else "ax-bg"
HOME = os.path.expanduser("~")
DRV = f"{HOME}/cua/libs/cua-driver/rust/target/release/cua-driver"
HARNESS = "/tmp/lin-harness.py"
DASH = "/tmp/lin-dash.py"
DASH_HTML = "/tmp/lin-dashboard.html"
PORT = 8146
WORK = f"/tmp/cua-lin-{MODE}"
REC = f"{WORK}/rec"
HARW, HARH = 536, 740
PANX, PANW, PANH = 544, 480, 740

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
    except Exception as e:
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

def hstate():
    try: return json.load(open("/tmp/cua-lin-state.json"))
    except Exception: return {}

def verify(t, before, after):
    # did the action actually change the harness's state?
    if t == "click":  return "ok" if after.get("agreed") != before.get("agreed") else "fail"
    if t == "double": return "ok" if after.get("last_action") == "double_click" else "fail"
    if t == "right":  return "ok" if after.get("ctx", "none") != "none" else "fail"
    if t == "drag":   return "ok" if after.get("slider", 0) > before.get("slider", 0) else "fail"
    if t == "scroll": return "ok" if after.get("scroll", 0) > before.get("scroll", 0) else "fail"
    if t == "setval": return "ok" if after.get("mirror", "") == "set-by-cua" else "fail"
    if t == "type":   return "ok" if "typed-by-cua" in str(after.get("mirror", "")) else "fail"
    return "na"  # press-key (Tab): no observable harness-state effect to assert

def flush():
    fg = active_name()
    af = "CuaTestHarness" in fg
    st = {"run":m["title"],"expect":m["expect"],"fgmode":m["fg"],"foreground":fg,"appFront":af,
          "steals":state["steals"],"actions":state["actions"],"steps":steps}
    with open(f"{WORK}/status.json","w") as f: json.dump(st, f)

def pulse(sec):
    end = time.time()+sec
    while time.time() < end: flush(); time.sleep(0.15)

# ---------- setup ----------
os.makedirs(REC, exist_ok=True)
# NB: do NOT pkill cua-driver here — the wrapper owns the daemon. Port is freed by the wrapper.
subprocess.run("pkill -f lin-harness.py; pkill -f lin-dash.py", shell=True)
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

# openbox + `cua-driver serve` are launched by the bash wrapper (so the daemon
# reliably inherits DISPLAY/XAUTHORITY — a python-launched daemon saw 0 windows).
open(f"{WORK}/setup.log","a").write("DISPLAY=%s XAUTH=%s\n" % (os.environ.get("DISPLAY"), os.environ.get("XAUTHORITY")))
dbgcnt("at start")
D("set_agent_cursor_enabled", {"enabled":True,"session":"d1"})
D("set_agent_cursor_motion", {"session":"d1","cursor_color":"#FF2D2D","cursor_label":"cua-driver","glide_duration_ms":600,"dwell_after_click_ms":700,"idle_hide_ms":120000})
dbgcnt("after cursor")
subprocess.Popen(["python3","-m","http.server",str(PORT),"--directory",WORK], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
subprocess.Popen(["python3", HARNESS], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); time.sleep(3)
dbgcnt("after harness")
subprocess.Popen(["python3", DASH, f"http://127.0.0.1:{PORT}/dashboard.html", str(PANX),"0",str(PANW),str(PANH)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); time.sleep(4)
dbgcnt("after dashboard")
# position windows: harness LEFT, dashboard RIGHT + above
sh(["wmctrl","-r","CuaTestHarness GTK3","-e","0,0,0,%d,%d"%(HARW,HARH)])
sh(["wmctrl","-r","cua-driver-panel","-e","0,%d,0,%d,%d"%(PANX,PANW,PANH)])
sh(["wmctrl","-r","cua-driver-panel","-b","add,above"])
time.sleep(1)

# resolve harness window
w = None
for _ in range(20):
    wins = DJ("list_windows", {}).get("windows", [])
    w = next((x for x in wins if "CuaTestHarness" in (x.get("title","") or "")), None)
    if w: break
    time.sleep(0.5)
if not w:
    dbg = "DISPLAY=%s\n" % os.environ.get("DISPLAY")
    try: xalive = subprocess.run(["xdpyinfo"], capture_output=True, timeout=8).returncode == 0
    except Exception: xalive = False
    dbg += "X_alive=%s\n" % xalive
    dbg += "raw_list_windows=%s\n" % (D("list_windows", {})[:1800])
    dbg += "xwininfo=%s\n" % sh(["bash","-c","xwininfo -root -tree 2>/dev/null | grep -iE 'Cua|panel|GTK'"])
    open(f"{WORK}/debug.log","w").write(dbg)
    open(f"{WORK}/metric.log","w").write("FATAL: no harness window"); sys.exit(1)
WP, WD = w["pid"], w["window_id"]
WB = w.get("bounds") or w.get("frame") or {"x":0,"y":0,"w":HARW,"h":HARH}

def els():
    return DJ("get_window_state", {"pid":WP,"window_id":WD,"capture_mode":"ax"}).get("elements", [])

def find(E, **kw):
    for e in E:
        nm = str(e.get("label", e.get("name","")))
        role = str(e.get("role",""))
        if "name" in kw and kw["name"] not in nm: continue
        if "role" in kw and kw["role"].lower() not in role.lower(): continue
        return e
    return None

E = []
resolve = {}
for _ in range(14):
    E = els()
    resolve = {
        "chk": find(E, name="chk-agree") or find(E, role="check"),
        "btn": find(E, name="btn-clicktarget"),
        "ctx": find(E, name="btn-context"),
        "sld": find(E, name="sld-value") or find(E, role="slider"),
        "scr": find(E, name="scroll-tall"),
        "txt": find(E, name="txt-input") or find(E, role="text"),
    }
    if resolve["chk"] or resolve["txt"]: break
    time.sleep(0.7)
dump = "\n".join("[%s] role=%r name=%r" % (e.get("element_index"), e.get("role"), e.get("label", e.get("name",""))) for e in E)
open(f"{WORK}/resolve.log","w").write("RESOLVE "+" ".join("%s=%s"%(k,bool(v)) for k,v in resolve.items())+" count=%d\n--- elements ---\n%s"%(len(E),dump))

try: GEOM = json.load(open("/tmp/cua-lin-geom.json"))
except Exception: GEOM = {}
open(f"{WORK}/resolve.log","a").write("\n--- geom ---\n"+json.dumps(GEOM))
def gcenter(sel):
    g = GEOM.get(sel)
    return (g["x"]+g["w"]//2, g["y"]+g["h"]//2) if g else None
def gwinlocal(sel):
    g = GEOM.get(sel)
    return (g["x"]-int(WB["x"])+g["w"]//2, g["y"]-int(WB["y"])+g["h"]//2) if g else None

# seed agent cursor overlay BEFORE recording
D("move_cursor", {"x":HARW-30,"y":HARH-30,"session":"d1"})
time.sleep(0.4)
if DESKTOP: D("set_config", {"key":"capture_scope","value":"desktop"})
D("start_recording", {"output_dir":REC,"record_video":True})
pulse(2)

def do(t, sel):
    el = resolve.get(sel)
    g = GEOM.get(sel)
    eidx = el.get("element_index") if el else None
    use_ax = (eidx is not None) and (not VISION) and (not DESKTOP)
    c = gcenter(sel); wl = gwinlocal(sel) or (0,0)
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
        if g: fx=g["x"]-int(WB["x"])+8; fy=g["y"]-int(WB["y"])+g["h"]//2; D("drag", {"pid":WP,"from_x":fx,"from_y":fy,"to_x":fx+150,"to_y":fy,"session":"d1"})
    elif t == "scroll":
        if DESKTOP and c: D("scroll", {"x":c[0],"y":c[1],"direction":"down","session":"d1"})
        elif use_ax: D("scroll", {"pid":WP,"window_id":WD,"element_index":eidx,"direction":"down","session":"d1"})
        elif c: D("scroll", {"pid":WP,"window_id":WD,"x":wl[0],"y":wl[1],"direction":"down","session":"d1"})
    elif t == "setval":
        if eidx is not None: D("set_value", {"pid":WP,"window_id":WD,"element_index":eidx,"value":"set-by-cua","session":"d1"})
    elif t == "type":
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
        if "CuaTestHarness" in active_name(): stole=True
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
subprocess.run("pkill -f lin-harness.py; pkill -f lin-dash.py; pkill -f 'http.server 8146'; pkill cua-driver; pkill openbox", shell=True)
mp4 = (glob.glob(f"{REC}/**/*.mp4", recursive=True) or [None])[0]
size = os.path.getsize(mp4) if mp4 else 0
worked = sum(1 for s in steps if s.get("verified")=="ok"); ver = sum(1 for s in steps if s.get("verified") in ("ok","fail")); verdict = ("foreground-mode, %d actions"%state["actions"]) if m["fg"] else ("%d/%d stole focus"%(state["steals"],state["actions"]))
line = "MODE=%s MEASURE=%s EFFECTS=%d/%d_landed MP4=%s SIZE=%d" % (MODE, verdict, worked, ver, mp4, size)
open(f"{WORK}/metric.log","w").write(line); print(line)
