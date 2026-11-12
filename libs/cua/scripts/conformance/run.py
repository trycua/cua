#!/usr/bin/env python3
"""Runtime conformance, live: the same cases against every runtime this computer has.

The roster and what each entry must do come from `entries.json`, which is generated from
`cua_vmm::conformance` (a test keeps the file in step). A Space behaves the same on every runtime:
the secure default network holds, names resolve, the guest downloads packages, a private image is
pulled with the host-held credential, a home makes and deletes child Spaces, a coding agent session
starts, and delete leaves nothing. A runtime that cannot keep a promise refuses it at once with the
reason the registry holds; anything else is a failure.

Opt in: nothing here runs by itself. Everything is made in an isolated Cua home (never ~/.cua) with
the daemon this script starts from the binary under test (never `cua daemon start`, which hands
over to an installed Cua Spaces app), and everything it makes is deleted, also on failure.

    CUA=/path/to/cua-spaces-cli ./run.py --entry containerization-microvm --entry vz-microvm \
        --image ghcr.io/trycua/linux:24.04-slim --private --out results/

The registry credential comes from the environment (CUA_REGISTRY_USERNAME and CUA_REGISTRY_PASSWORD)
or, with --private, from `gh auth token`. It is passed to the daemon's environment only and never
printed or written.
"""
import argparse, json, os, platform, re, shutil, subprocess, sys, tempfile, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENTRIES = json.loads((HERE / "entries.json").read_text())
HOST_OS = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system(), platform.system().lower())
SECRET = re.compile(r"(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_\w+|sk-ant-\S+)")
# Words in an error that mean "this computer lacks what the runtime needs" (a skip, not a failure).
MISSING = ("not installed", "is not set up", "cua-vz", "cua-microvm-init", "/dev/kvm", "needs macOS", "not available", "Missing")


def redact(s):
    return SECRET.sub("<redacted>", s or "")


class Run:
    """One isolated home with its own daemon."""

    def __init__(self, cua, home, mode, env_extra, log):
        self.cua, self.home, self.mode, self.log = cua, Path(home), mode, log
        self.env = dict(os.environ)
        self.env.update({
            "CUA_HOME": str(self.home), "CUA_TELEMETRY": "0", "DO_NOT_TRACK": "1",
            "CUA_CREDENTIAL_STORE": "test-keychain",
        })
        # The daemon forwards it to the agent (`env_from_host`); the scripted model ignores the value.
        self.env.setdefault("ANTHROPIC_API_KEY", "mock-key")
        if mode == "open":
            # No protection, for the cases that need a network where the runtime cannot enforce one.
            self.env["CUA_SECURE_DEFAULT"] = "0"
        self.env.update(env_extra)
        self.daemon = None

    def sh(self, *args, timeout=300, check=False):
        t = time.monotonic()
        try:
            p = subprocess.run([self.cua, *args], capture_output=True, text=True, env=self.env, timeout=timeout)
            rc, out, err = p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as e:
            rc, out, err = 124, (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or ""), "timed out"
        ms = (time.monotonic() - t) * 1000
        out, err = redact(out), redact(err)
        self.log(f"$ cua {' '.join(args[:6])}{' ...' if len(args) > 6 else ''} -> rc={rc} {ms:.0f}ms")
        return rc, out, err, ms

    def start(self):
        self.home.mkdir(parents=True, exist_ok=True)
        self.daemon = subprocess.Popen(
            [self.cua, "daemon", "start", "--foreground", "--loopback", "127.0.0.1:0"],
            stdout=open(self.home / "daemon.log", "w"), stderr=subprocess.STDOUT, env=self.env)
        for _ in range(150):
            rc, out, _, _ = self.sh("daemon", "status", timeout=30)
            if rc == 0 and "pid" in out:
                return True
            if self.daemon.poll() is not None:
                return False
            time.sleep(0.2)
        return False

    def stop(self):
        self.sh("daemon", "stop", timeout=60)
        if self.daemon and self.daemon.poll() is None:
            try:
                self.daemon.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.daemon.terminate()  # our own child, by handle

    def exec(self, name, script, timeout=120):
        # `sandbox exec` stops a command after 120 s by itself: give it the caller's limit.
        return self.sh("sandbox", "exec", name, "--timeout", str(int(timeout)), script, timeout=timeout + 30)


def create_args(entry, image, name, headless=True, extra=()):
    a = ["sandbox", "create", image, "--kind", entry["kind"], "--name", name, "--cpu", "1", "--memory", "1GB", "--json"]
    if entry["runtime"]:
        a += ["--runtime", entry["runtime"]]
    if headless:
        a += ["--headless"]
    return a + list(extra)


def cases_of(entry, host):
    for h in entry["hosts"]:
        if h["os"] == host:
            return {c["case"]: c for c in h["cases"]}
    return {}


def host_addresses():
    """The host's own IPv4 addresses and its default gateway: none may be reachable from a guest."""
    out = set()
    try:
        if HOST_OS == "macos":
            for ifc in ("en0", "en1", "bridge100", "bridge101"):
                r = subprocess.run(["ipconfig", "getifaddr", ifc], capture_output=True, text=True)
                if r.stdout.strip():
                    out.add(r.stdout.strip())
            r = subprocess.run(["route", "-n", "get", "default"], capture_output=True, text=True)
            m = re.search(r"gateway: (\S+)", r.stdout)
            if m:
                out.add(m.group(1))
        else:
            r = subprocess.run(["ip", "-4", "-o", "addr"], capture_output=True, text=True)
            out |= set(re.findall(r"inet (\d+\.\d+\.\d+\.\d+)", r.stdout)) - {"127.0.0.1"}
            r = subprocess.run(["ip", "route", "show", "default"], capture_output=True, text=True)
            m = re.search(r"via (\S+)", r.stdout)
            if m:
                out.add(m.group(1))
    except OSError:
        pass
    # Where a guest sees the host through each runtime's own gateway.
    out |= {"192.168.64.1", "192.168.5.2", "172.17.0.1", "10.0.2.2", "172.16.0.1", "host.docker.internal", "host.lima.internal"}
    return sorted(out)


class Listener:
    """A TCP listener on every interface of the host: what 'no host' is tested against."""

    def __init__(self):
        import socket, threading
        self.s = socket.socket()
        self.s.bind(("0.0.0.0", 0))
        self.s.listen(8)
        self.port = self.s.getsockname()[1]
        self.hits = []
        self.alive = True

        def loop():
            while self.alive:
                try:
                    c, a = self.s.accept()
                except OSError:
                    return
                self.hits.append(a[0])
                try:
                    c.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi")
                finally:
                    c.close()
        threading.Thread(target=loop, daemon=True).start()

    def close(self):
        self.alive = False
        self.s.close()


def run_entry(entry, args, out_dir, log):
    """Returns the entry's row: {case: {status, detail, ms}}."""
    rows = {}
    row = lambda case, status, detail="", ms=None: rows.__setitem__(case, {"status": status, "detail": detail[:600], "ms": None if ms is None else round(ms)})
    if entry["not_runnable"]:
        for c in ("ready",):
            row(c, "n/a", entry["not_runnable"])
        return rows
    if HOST_OS not in [h["os"] for h in entry["hosts"]]:
        row("ready", "n/a", f"does not run on {HOST_OS}")
        return rows
    want = cases_of(entry, HOST_OS)
    selected = [c for c in want if want[c]["expect"] != "refused" or c == "policy"]
    cases = [c for c in (args.cases.split(",") if args.cases else [x["case"] for x in ENTRIES["cases"]])]
    # The registry decides which cases apply to the entry's guest (a macOS VM has no coding agent).
    cases = [c for c in cases if c not in want or want[c]["why"] != "not applicable to this guest"]
    image = args.image
    private_entry = bool(args.private)
    override = dict(x.split("=", 1) for x in (args.entry_image or []))
    if entry["id"] in override:
        image, private_entry = override[entry["id"]], False  # a public image of that runtime's kind
    # Short: the built-in runtime keeps Unix sockets under the home (103 bytes at most).
    idx = [e["id"] for e in ENTRIES["entries"]].index(entry["id"])
    home = Path(args.home_root) / f"c{idx}"
    if len(str(home)) > 42:
        raise SystemExit(f"the home {home} is too long for Unix socket paths: use a shorter --home-root")
    shutil.rmtree(home, ignore_errors=True)
    env_extra = {}
    private = private_entry
    if private:
        if "CUA_REGISTRY_PASSWORD" not in os.environ:
            tok = subprocess.run(["gh", "auth", "token"], capture_output=True, text=True).stdout.strip()
            if not tok:
                row("registry", "skip", "no registry credential in the environment and `gh auth token` gave none")
                private = False
            else:
                env_extra["CUA_REGISTRY_USERNAME"] = args.registry_user
                env_extra["CUA_REGISTRY_PASSWORD"] = tok
        if args.alias:
            (home).mkdir(parents=True, exist_ok=True)
            (home / "image-aliases.json").write_text(json.dumps({image: args.alias}))
            env_extra["CUA_IMAGE_ALIASES"] = str(home / "image-aliases.json")
    for var in ("CUA_VZ", "CUA_MICROVM_INIT"):
        if args.tools and (Path(args.tools) / {"CUA_VZ": "cua-vz", "CUA_MICROVM_INIT": "cua-microvm-init"}[var]).exists():
            env_extra[var] = str(Path(args.tools) / {"CUA_VZ": "cua-vz", "CUA_MICROVM_INIT": "cua-microvm-init"}[var])
    name = "conf"
    listener = Listener()
    run = None
    mode = "secure"
    try:
        policy = want.get("policy", {"expect": "pass"})
        # --- the secure default: held, or refused at once with the registry's reason
        if "policy" in cases and "policy" in want:
            run = Run(args.cua, home, "secure", env_extra, log)
            if not run.start():
                row("ready", "fail", "the daemon did not start: " + (home / "daemon.log").read_text()[-300:])
                return rows
            t = time.monotonic()
            rc, o, e, ms = run.sh(*create_args(entry, image, name), timeout=args.timeout)
            if policy["expect"] == "refused":
                reason = (policy["why"] or "")[:50]
                said = (e + o)
                norm = lambda s: re.sub(r"\s+", " ", s)
                if rc != 0 and norm(reason) in norm(said):
                    row("policy", "pass", f"refused at once ({ms:.0f} ms) with the registry's reason")
                elif rc == 0:
                    row("policy", "fail", "created under the secure default although the runtime declares it cannot protect the network")
                    run.sh("sandbox", "rm", name, "-f")
                else:
                    row("policy", "fail", "refused, but not with the declared reason: " + norm(said)[:300])
                run.stop()
                mode = "open"
                run = Run(args.cua, home, "open", env_extra, log)
                if not run.start():
                    row("ready", "fail", "the daemon did not start (open)")
                    return rows
                rc, o, e, ms = run.sh(*create_args(entry, image, name), timeout=args.timeout)
            if rc != 0:
                missing = any(k in (e + o) for k in MISSING)
                row("ready", "skip" if missing else "fail", "create failed: " + (e or o).strip()[:400])
                return rows
        else:
            run = Run(args.cua, home, "secure", env_extra, log)
            if not run.start():
                row("ready", "fail", "the daemon did not start")
                return rows
            rc, o, e, ms = run.sh(*create_args(entry, image, name), timeout=args.timeout)
            if rc != 0:
                row("ready", "fail", "create failed: " + (e or o).strip()[:400])
                return rows
        ready_first = ms
        row("ready", "pass", f"first create {ms / 1000:.1f}s", ms)
        if private and "registry" in cases:
            row("registry", "pass", "the private image was pulled with the host-held credential", ms)
        elif "registry" in cases and "registry" not in rows:
            row("registry", "skip", "the image is public (pass --private with a private image)")
        run.sh("spaces", "add", f"local:{name}")
        # --- network, only where the secure default holds
        if "policy" in cases and policy["expect"] == "pass" and mode == "secure":
            fails = []
            rc, o, e, _ = run.exec(name, 'curl -sS -m 15 -o /dev/null -w "%{http_code}" https://registry.npmjs.org/')
            if "200" not in o:
                fails.append(f"internet not reachable: {o.strip()} {e.strip()[:120]}")
            rc, o, _, _ = run.exec(name, 'curl -sS -m 4 -o /dev/null -w "%{http_code}" http://169.254.169.254/ || true')
            if o.strip().endswith("200") or o.strip().startswith("2"):
                fails.append("cloud metadata reachable")
            for addr in host_addresses():
                rc, o, _, _ = run.exec(name, f'curl -sS -m 3 -o /dev/null -w "%{{http_code}}" http://{addr}:{listener.port}/ || true', timeout=60)
                if o.strip().endswith("200") or listener.hits:
                    fails.append(f"the host's listener answered at {addr}")
                    break
            # The refused attempts are in `cua network log` with their destination and time, and
            # the destination that was reached is not among them.
            rc, o, e, _ = run.sh("network", "log", name, "--json")
            try:
                events = json.loads(o)
            except ValueError:
                events = []
                fails.append("cua network log did not answer: " + (o + e).strip()[:120])
            meta = [x for x in events if "169.254" in x.get("dst", "") or x.get("reason", "").startswith("platform:metadata")]
            if not meta:
                fails.append("the refused metadata attempt is not in the network log")
            elif not meta[0].get("last_unix") or not meta[0].get("count"):
                fails.append("the logged metadata attempt has no time or count")
            rc, ips, _, _ = run.exec(name, "getent ahostsv4 registry.npmjs.org | awk '{print $1}' | sort -u")
            reached = {i.strip() for i in ips.split() if i.strip()}
            if any(x.get("dst", "").split(":")[0].split("/")[0] in reached for x in events):
                fails.append("an allowed destination is logged as blocked")
            # A denied destination of the Space's own list is logged the same way (when the runtime
            # applies it to the running Space).
            rc, o, e, _ = run.sh("network", "set", name, "--network", "internet", "--deny", "1.1.1.1")
            set_said = (o + e).strip().replace("\n", " ")[:160]
            if rc == 0 and "next start" not in (o + e).lower():
                run.exec(name, 'curl -sS -m 3 -o /dev/null http://1.1.1.1/ || true', timeout=60)
                denied = []
                for _ in range(8):  # the gateway writes its log a moment after the refusal
                    rc, o, e, _ = run.sh("network", "log", name, "--json")
                    try:
                        denied = [x for x in json.loads(o) if "1.1.1.1" in x.get("dst", "")]
                    except ValueError:
                        denied = []
                    if denied:
                        break
                    time.sleep(2)
                if not denied or not denied[0].get("last_unix"):
                    fails.append(f"a destination of the Space's own deny list is not in the network log with a time (set said: {set_said}; destinations in the log: {sorted({x.get('dst', '') for x in json.loads(o)}) if o.strip().startswith('[') else o.strip()[:80]})")
            row("policy", "fail" if fails else "pass", "; ".join(fails) or "internet yes; metadata, host and its addresses no; refusals are logged with destination and time")
        elif "policy" in cases and policy["expect"] == "pass" and mode != "secure":
            row("policy", "fail", "ran open although the runtime declares it protects the network")
        # --- dns: names resolve and an install-style download works on the secure network; with
        #     no network the lookup fails at once (the default network follows, after the others)
        dns_fails = []
        dns_ms = None
        if "dns" in cases:
            dns_fails, dns_notes, dns_ms = dns_checks(run, name, entry, policy, mode)
            row("dns", "fail" if dns_fails else "pass", "; ".join(dns_fails) or "; ".join(dns_notes), dns_ms)
        # --- download
        if "download" in cases:
            script = ('case "$(uname -m)" in aarch64|arm64) p=linux-arm64;; *) p=linux-x64;; esac; '
                      'u=$(curl -sS -m 30 "https://registry.npmjs.org/@anthropic-ai%2fclaude-code-$p/latest" | jq -r .dist.tarball); '
                      'curl -sS -L -m 900 -o /dev/null -w "%{size_download} bytes in %{time_total}s from $u\\n" "$u"')
            rc, o, e, ms = run.exec(name, script, timeout=700)
            m = re.search(r"(\d+) bytes in ([\d.]+)s", o)
            ok = bool(m) and int(m.group(1)) > 10_000_000
            row("download", "pass" if ok else "fail", (o + e).strip()[:300], ms)
        # --- children
        if "children" in cases:
            row_children(run, entry, name, args, row, mode)
        # --- agent
        if "agent" in cases:
            row_agent(run, entry, name, args, row, image)
        # --- lifecycle
        if "lifecycle" in cases:
            row_lifecycle(run, entry, name, row)
        # --- warm claim
        if "warm" in cases:
            row_warm(run, entry, image, args, row)
        # --- cached ready times
        if args.n > 0 and "ready" in cases:
            times = []
            for i in range(args.n):
                run.sh("sandbox", "rm", name, "-f", timeout=300)
                rc, o, e, ms = run.sh(*create_args(entry, image, name), timeout=args.timeout)
                if rc != 0:
                    times.append(None)
                    break
                times.append(round(ms))
            run.sh("sandbox", "rm", name, "-f", timeout=300)
            # A daemon stopped in the middle of a warm-pool burst outlives its own 30 s grace and
            # leaves the built-in Linux VM running (a known issue): let the pool settle first.
            time.sleep(args.settle)
            good = [t for t in times if t]
            if good:
                rows["ready"]["detail"] += f"; cached x{len(good)}: " + ", ".join(f"{t / 1000:.1f}s" for t in good) + f" (median {sorted(good)[len(good) // 2] / 1000:.1f}s)"
                rows["ready"]["cached_ms"] = good
        # --- dns on the default (open) network: a second daemon without the secure default
        if "dns" in cases and "dns" in rows and rows["dns"]["status"] == "pass" and mode == "secure" \
                and policy["expect"] == "pass" and not args.no_open_dns:
            run.sh("sandbox", "rm", name, "-f", timeout=300)
            run.stop()
            run = Run(args.cua, home, "open", env_extra, log)
            if not run.start():
                row("dns", "fail", rows["dns"]["detail"] + "; default network: the daemon did not start")
            else:
                open_name = "conf-open"
                rc, o, e, ms = run.sh(*create_args(entry, image, open_name), timeout=args.timeout)
                if rc != 0:
                    row("dns", "fail", rows["dns"]["detail"] + "; default network: create failed: " + (e or o).strip()[:200])
                else:
                    run.sh("spaces", "add", f"local:{open_name}")
                    fails, notes, _ = dns_checks(run, open_name, entry, policy, "open")
                    row("dns", "fail" if fails else "pass",
                        rows["dns"]["detail"] + "; default network: " + ("; ".join(fails) or "; ".join(notes)), rows["dns"]["ms"])
                run.sh("sandbox", "rm", open_name, "-f", timeout=300)
    finally:
        listener.close()
        # --- teardown, then prove nothing is left
        if run is not None:
            for n in (name, "conf-open", "conf-fork", "conf-child", "conf-big", "conf-home"):
                run.sh("sandbox", "rm", n, "-f", timeout=300)
            run.sh("sandbox", "checkpoints", "prune", timeout=120)
            run.stop()
            # A warm-pool member the daemon kept ready holds the built-in Linux VM up past the
            # daemon's exit; `daemon reap` (no daemon needed) ends what a gone daemon left.
            run.sh("daemon", "reap", timeout=300)
            stop_apple_services(run.home)
            ok, detail = teardown(run, entry, name)
            if "teardown" in cases:
                row("teardown", "pass" if ok else "fail", detail)
        shutil.rmtree(home, ignore_errors=True) if not args.keep_home else None
    return rows


def home_image_for(entry, args):
    """The image a home Space is made from: the slim Linux image (it carries the `cua` CLI),
    its VM disk for a VM runtime, or what --entry-home-image names for this entry."""
    override = dict(x.split("=", 1) for x in (args.entry_home_image or []))
    if entry["id"] in override:
        return override[entry["id"]]
    return args.home_image + "-disk" if entry["kind"] == "vm" and not args.home_image.endswith("-disk") else args.home_image


RESOLVE = ('getent ahosts nodejs.org | head -2; '
           'python3 -c "import socket;print(len(socket.getaddrinfo(\'nodejs.org\',443)))" 2>&1; '
           # an install-style download: redirects followed, failures are errors, a file comes back
           'curl -fsSL -m 60 -o /tmp/dns-probe.json -w "curl %{http_code} %{size_download}" https://nodejs.org/dist/index.json')
NO_NETWORK = ('getent hosts nodejs.org >/dev/null 2>&1; echo "getent=$?"; '
              'curl -sS -m 20 -o /dev/null https://nodejs.org/ >/dev/null 2>&1; echo "curl=$?"')


def dns_checks(run, name, entry, policy, mode):
    """The 'a Space resolves names' case on one Space. Returns (failures, notes, ms)."""
    fails, notes = [], []
    rc, o, e, ms = run.exec(name, RESOLVE, timeout=150)
    m = re.search(r"curl 200 (\d+)", o)
    if rc == 0 and m and int(m.group(1)) > 1000 and "STREAM" in o:
        notes.append(f"{mode}: nodejs.org resolves and downloads {m.group(1)} bytes")
    else:
        fails.append(f"{mode}: " + (o + e).strip().replace("\n", " | ")[:260])
    if mode == "secure" and policy["expect"] == "pass":
        # No network: the lookup must fail at once, not hang, and the network comes back after.
        rc, o, e, _ = run.sh("network", "set", name, "--network", "none", timeout=120)
        said = (e or o).strip()
        if rc != 0 and re.search(r"not supported|cannot|can't|does not|refus|unsupported", said, re.I):
            # The runtime says, in words, that it cannot run a Space without network: gated, with its reason.
            notes.append("none: gated by the runtime: " + said[:120])
        elif rc != 0:
            fails.append("none: could not set the network: " + said[:160])
        else:
            t = time.monotonic()
            rc, o, e, _ = run.exec(name, NO_NETWORK, timeout=90)
            took = time.monotonic() - t
            g, c = re.search(r"getent=(\d+)", o), re.search(r"curl=(\d+)", o)
            if not (g and c and g.group(1) != "0" and c.group(1) == "6"):
                fails.append("none: a name resolved or the failure was not a resolution error: " + (o + e).strip().replace("\n", " | ")[:160])
            elif took > 20:
                fails.append(f"none: the lookup failed only after {took:.0f} s")
            else:
                notes.append(f"none: lookup fails at once ({took:.1f} s)")
            rc, o, e, _ = run.sh("network", "set", name, "--network", "internet", timeout=120)
            rc, o, e, _ = run.exec(name, RESOLVE, timeout=150)
            if not (rc == 0 and "curl 200" in o):
                fails.append("back on the internet preset names do not resolve: " + (o + e).strip().replace("\n", " | ")[:160])
    return fails, notes, ms


def row_children(run, entry, name, args, row, mode):
    child = "conf-child"
    img = args.child_image
    flags = ["--kind", entry["kind"]] + (["--runtime", entry["runtime"]] if entry["runtime"] else [])
    # The case drives the `cua` CLI inside the Space: the Space under test when its image ships it,
    # else a home made from the slim Linux image on the same runtime.
    rc, o, e, _ = run.exec(name, "command -v cua >/dev/null 2>&1 && echo has-cua || echo no-cua")
    if "has-cua" not in o:
        home_img = home_image_for(entry, args)
        name = "conf-home"
        rc, o, e, ms = run.sh(*create_args(entry, home_img, name), timeout=args.timeout)
        if rc != 0:
            said = (e or o).strip()
            gone = any(k in said for k in ("not published", "not found", "manifest unknown", "MANIFEST_UNKNOWN", "denied", "unauthorized"))
            row("children", "skip" if gone or any(k in said for k in MISSING) else "fail",
                f"the home image {home_img} could not be made: {said[:300]} (build libs/images/linux --target slim and pass --home-image, or --entry-home-image ID=IMAGE)")
            return
        run.sh("spaces", "add", f"local:{name}")
        for _ in range(60):  # a Space that was just made answers a moment later
            rc, o, e, _ = run.exec(name, "command -v cua >/dev/null 2>&1 && echo has-cua || echo no-cua")
            if rc == 0:
                break
            time.sleep(2)
        if "has-cua" not in o:
            row("children", "fail", f"the home image {home_img} has no `cua` CLI in the guest: {(o + e).strip()[-200:]}")
            return
    rc, o, e, ms = run.sh("spaces", "children", f"local:{name}", "on")
    if rc != 0:
        row("children", "fail", "children on: " + (e or o).strip()[:200])
        return
    # The daemon opens the Space's spawn channel once the guest's cua-spacesd answers: the files
    # the guest's `cua` looks for appear when it is up.
    up = ""
    for _ in range(60):
        rc, up, _, _ = run.exec(name, "ls /run/cua/spawn-address /dev/shm/cua-spawn-*/spawn-address /tmp/cua-spawn-*/spawn-address \"$XDG_RUNTIME_DIR\"/cua-spawn/spawn-address 2>/dev/null | head -1")
        if up.strip():
            break
        time.sleep(2)
    if not up.strip():
        row("children", "fail", "the Space's spawn channel did not come up within 120 s (its cua-spacesd needs the spawn service: cua-spacesd 0.10.1 or later)")
        return
    inner = f'cua spaces create {img} {" ".join(flags)} --name {child} 2>&1 | tail -6'
    t = time.monotonic()
    rc, o, e, _ = run.exec(name, inner, timeout=args.timeout)
    made = "Created" in o
    steps = [("create", made, o.strip()[-600:])]
    if made:
        rc, o, e, _ = run.exec(name, f"cua spaces ls | grep -c {child}")
        steps.append(("list", o.strip().endswith("1"), o.strip()))
        rc, o, e, _ = run.exec(name, f'cua sandbox exec {child} -- echo child-ok', timeout=120)
        steps.append(("use", "child-ok" in o, (o + e).strip()[-100:]))
        # the budget: a create that cannot fit is refused, in plain words
        rc, o, e, _ = run.exec(name, f'cua spaces create {img} {" ".join(flags)} --name conf-big --cpus 99 2>&1 | tail -2')
        steps.append(("budget", ("budget" in o.lower() or "too many" in o.lower() or "exceed" in o.lower() or "left" in o.lower()) and "Created" not in o, o.strip()[-160:]))
        rc, o, e, _ = run.exec(name, f"cua spaces delete {child} -f 2>&1 | tail -1; cua spaces ls | grep -c {child}")
        steps.append(("delete", o.strip().endswith("0") and "Deleted" in o, o.strip()[-100:]))
    audit = Path(run.home) / "children-audit.jsonl"
    text = audit.read_text() if audit.exists() else ""
    steps.append(("audit", '"spawn"' in text, f"{len(text.splitlines())} audit lines"))
    bad = [f"{n}: {d}" for n, ok, d in steps if not ok]
    row("children", "fail" if bad else "pass", "; ".join(bad) or "create, list, use, budget refusal, delete, audited", (time.monotonic() - t) * 1000)


def row_agent(run, entry, name, args, row, image):
    if not args.agent_python:
        row("agent", "skip", "no --agent-python (a python with the `mcp` package) to drive the agent")
        return
    script = HERE / "agent_case.py"
    t = time.monotonic()
    p = subprocess.run([args.agent_python, str(script), "--cua", run.cua, "--space", f"local:{name}", "--name", name],
                       capture_output=True, text=True, env=run.env, timeout=1500)
    out = redact(p.stdout + p.stderr)
    row("agent", "pass" if p.returncode == 0 else "fail", out.strip().splitlines()[-1][:300] if out.strip() else "", (time.monotonic() - t) * 1000)


def row_lifecycle(run, entry, name, row):
    caps = entry["capabilities"]
    fails, notes = [], []
    # stop and resume
    rc, o, e, ms = run.sh("sandbox", "suspend", name, timeout=300)
    if rc != 0:
        fails.append("suspend: " + (e or o).strip()[:120])
    else:
        rc, o, e, ms2 = run.sh("sandbox", "resume", name, timeout=600)
        if rc != 0:
            fails.append("resume: " + (e or o).strip()[:160])
        else:
            rc, o, e, _ = run.exec(name, "echo back")
            if "back" not in o:
                fails.append("after resume the guest does not answer")
            notes.append(f"suspend {ms:.0f} ms, resume {ms2:.0f} ms")
    if "restart" and not fails:
        rc, o, e, ms = run.sh("sandbox", "restart", name, timeout=600)
        if rc != 0:
            fails.append("restart: " + (e or o).strip()[:160])
        else:
            # `restart` returns once the runtime says so: how long until the guest answers?
            t = time.monotonic()
            answered = None
            while time.monotonic() - t < 180:
                rc, o, e, _ = run.exec(name, "echo back2", timeout=30)
                if "back2" in o:
                    answered = time.monotonic() - t
                    break
                time.sleep(2)
            if answered is None:
                fails.append("after restart the guest does not answer in 180 s")
            notes.append(f"restart {ms:.0f} ms, answering {answered * 1000 + ms:.0f} ms after the call" if answered is not None else f"restart {ms:.0f} ms")
    # what it declares: a checkpoint and a fork, or a plain refusal
    rc, o, e, _ = run.sh("sandbox", "checkpoint", name, "conf-ck", timeout=600)
    if caps["checkpoint"]:
        if rc != 0:
            fails.append("declares checkpoints but: " + (e or o).strip()[:140])
        else:
            rc, o, e, _ = run.sh("sandbox", "fork", "conf-ck", "conf-fork", timeout=600) if caps["fork"] else (0, "", "", 0)
            if caps["fork"] and rc != 0:
                fails.append("declares forks but: " + (e or o).strip()[:140])
            run.sh("sandbox", "rm", "conf-fork", "-f", timeout=300)
            run.sh("sandbox", "checkpoints", "rm", "conf-ck", timeout=120)
    else:
        text = (e + o).strip()
        if rc == 0:
            fails.append("does not declare checkpoints but one was made")
            run.sh("sandbox", "checkpoints", "rm", "conf-ck", timeout=120)
        elif len(text) < 15:
            fails.append("a checkpoint is refused without a plain reason")
        else:
            notes.append("checkpoint refused: " + text[:80])
    row("lifecycle", "fail" if fails else "pass", "; ".join(fails + notes))


def row_warm(run, entry, image, args, row):
    """A pool of one: the member is claimed by a create, answers, is replaced, and the pool goes.

    The runtime says what it does: a runtime with no warm claims refuses the pool in plain words (its
    Spaces start cold); one that can claim hands out a Space that answers, and a second member is kept
    ready afterwards (the refill)."""
    caps = entry["capabilities"]
    pool, claimed = "conf-warm", "conf-warm-1"
    fails, notes = [], []
    t0 = time.monotonic()
    a = ["sandbox", "pool", "create", image, "--name", pool, "--min", "1", "--max", "1", "--kind", entry["kind"], "--cpu", "1", "--memory", "1GB", "--json"]
    if entry["runtime"]:
        a += ["--runtime", entry["runtime"]]
    rc, o, e, ms = run.sh(*a, timeout=300)
    if not caps["claim"]:
        said = (o + e).strip()
        if rc != 0 and len(said) >= 15:
            row("warm", "pass", "no warm claims here, said in plain words: " + said[:120], ms)
        else:
            row("warm", "fail", "this runtime declares no warm claims, but the pool was not refused in plain words")
        run.sh("sandbox", "pool", "rm", pool, timeout=120)
        return

    def ready():
        rc, o, e, _ = run.sh("sandbox", "pool", "ls", "--json", timeout=60)
        try:
            for p in json.loads(o):
                if p.get("name") == pool:
                    return int(p.get("ready", 0)), (p.get("status") or p.get("reason") or "")
        except (ValueError, TypeError, AttributeError):
            pass
        return 0, (o + e).strip()[:120]

    if rc != 0:
        row("warm", "fail", "pool create: " + (e or o).strip()[:200])
        return
    try:
        n, why = 0, ""
        deadline = time.monotonic() + 900
        while time.monotonic() < deadline:
            n, why = ready()
            if n >= 1:
                break
            time.sleep(3)
        if n < 1:
            fails.append(f"no member was ready within 15 minutes ({why})")
        else:
            notes.append(f"member ready after {time.monotonic() - t0:.0f}s")
            rc, o, e, ms = run.sh("sandbox", "create", "--pool", pool, "--name", claimed, "--json", timeout=args.timeout)
            if rc != 0:
                fails.append("create from the pool: " + (e or o).strip()[:200])
            else:
                notes.append(f"claim {ms:.0f} ms")
                rc, o, e, _ = run.exec(claimed, "echo claimed-ok && cat /etc/machine-id 2>/dev/null | head -c 8")
                if "claimed-ok" not in o:
                    fails.append("the claimed Space does not answer: " + (e or o).strip()[:120])
                # The pool keeps one member: a replacement comes.
                again = False
                for _ in range(200):
                    if ready()[0] >= 1:
                        again = True
                        break
                    time.sleep(3)
                if not again:
                    fails.append("no replacement member became ready in 10 minutes")
    finally:
        run.sh("sandbox", "rm", claimed, "-f", timeout=300)
        run.sh("sandbox", "pool", "rm", pool, timeout=300)
    row("warm", "fail" if fails else "pass", "; ".join(fails + notes), (time.monotonic() - t0) * 1000)


def stop_apple_services(home):
    """Apple's `container` services outlive a Space (cua never stops them, they are per user): the
    ones this run's home started are stopped before the home goes."""
    for b in Path(home).glob("vmm/containerization/install/*/bin/container"):
        env = dict(os.environ, CONTAINER_APP_ROOT=str(Path(home) / "vmm/containerization/data"), CONTAINER_INSTALL_ROOT=str(b.parent.parent))
        subprocess.run([str(b), "system", "stop"], capture_output=True, env=env, timeout=120)


def teardown(run, entry, name):
    rc, o, e, _ = run.sh("sandbox", "ls", "--embedded", timeout=60)
    left = [l for l in o.splitlines()[1:] if l.strip()]
    leftovers = []
    if left:
        leftovers.append(f"Spaces left: {len(left)}")
    home = str(run.home)
    # The runtimes exit with the daemon (the built-in Linux VM takes a few seconds): wait for
    # them before the home goes, never delete a home a VM still runs from.
    deadline = time.monotonic() + 90
    while True:
        ps = subprocess.run(["ps", "-axo", "pid,args"], capture_output=True, text=True).stdout
        mine = [l for l in ps.splitlines() if home in l and "daemon" not in l and ("cua-vz" in l or "container-runtime-linux" in l or "limactl" in l or "container-apiserver" in l or "qemu" in l or "firecracker" in l)]
        if not mine or time.monotonic() > deadline:
            break
        time.sleep(2)
    if mine:
        leftovers.append(f"{len(mine)} runtime process(es) still run after 90 s")
        # Never leave a VM running from a home that is about to be deleted: end this run's own
        # processes, by exact pid (they carry this run's isolated home in their command line).
        for l in mine:
            try:
                os.kill(int(l.split()[0]), 15)
            except (ValueError, OSError):
                pass
    return (not leftovers), "; ".join(leftovers) or "no Space, process or instance left"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--entry", action="append", help="an entry id (repeatable); default: every entry that runs on this OS")
    ap.add_argument("--cua", default=os.environ.get("CUA") or shutil.which("cua"))
    ap.add_argument("--image", default="ghcr.io/trycua/linux:24.04-slim")
    ap.add_argument("--alias", default=None, help="the package the image resolves to (a private mirror of the same tag)")
    ap.add_argument("--child-image", default="ghcr.io/trycua/linux:24.04-slim")
    ap.add_argument("--home-image", default="ghcr.io/trycua/linux:24.04-slim",
                    help="the image of the home Space the children case makes child Spaces from when the Space under test has no `cua` CLI "
                         "(a VM runtime adds -disk)")
    ap.add_argument("--entry-home-image", action="append", help="ID=IMAGE: the home image one entry uses (a local build, for example)")
    ap.add_argument("--private", action="store_true", help="the image is private: the credential comes from the environment or `gh auth token`")
    ap.add_argument("--registry-user", default="x")
    ap.add_argument("--entry-image", action="append", help="ID=IMAGE: the image one entry uses (public, e.g. qemu-vm=ghcr.io/trycua/linux:24.04)")
    ap.add_argument("--cases", default=None, help="comma list (default: all)")
    ap.add_argument("--n", type=int, default=0, help="extra creates after deleting the Space, for the cached ready time")
    ap.add_argument("--timeout", type=int, default=1500)
    ap.add_argument("--settle", type=int, default=45, help="seconds the daemon is left to settle (its warm pool) before it stops")
    ap.add_argument("--tools", default=None, help="a folder holding cua-vz and cua-microvm-init")
    ap.add_argument("--agent-python", default=None)
    ap.add_argument("--home-root", default="/tmp/cua-conf",
                    help="where the isolated Cua homes are made (short: Unix socket paths are limited)")
    ap.add_argument("--keep-home", action="store_true")
    ap.add_argument("--no-open-dns", action="store_true", help="skip the dns case on the default (open) network, which restarts the daemon")
    ap.add_argument("--out", default=str(HERE / "results"))
    args = ap.parse_args()
    if not args.cua:
        sys.exit("set CUA (or --cua) to the binary under test")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    wanted = args.entry or [e["id"] for e in ENTRIES["entries"] if not e["not_runnable"] and HOST_OS in [h["os"] for h in e["hosts"]]]
    by_id = {e["id"]: e for e in ENTRIES["entries"]}
    matrix = {}
    logf = open(out / "conformance.log", "a")

    def log(msg):
        logf.write(f"[{time.strftime('%H:%M:%S')}] {redact(msg)}\n")
        logf.flush()

    for eid in wanted:
        entry = by_id[eid]
        log(f"== {eid}")
        print(f"== {eid}", flush=True)
        load = subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip().split("load averages:")[-1].strip()
        try:
            rows = run_entry(entry, args, out, log)
        except Exception as e:  # a crash is a failure, with its words
            rows = {"ready": {"status": "fail", "detail": f"driver error: {redact(str(e))[:300]}", "ms": None}}
        matrix[eid] = {"host": HOST_OS, "load": load, "cases": rows}
        for c, r in rows.items():
            print(f"   {c:10} {r['status']:8} {r['detail'][:110]}", flush=True)
        (out / "matrix.json").write_text(json.dumps(matrix, indent=1))
    # a summary a person can read
    lines = ["| Runtime | " + " | ".join(c["case"] for c in ENTRIES["cases"]) + " |", "|---|" + "---|" * len(ENTRIES["cases"])]
    for eid, m in matrix.items():
        lines.append(f"| {eid} | " + " | ".join(m["cases"].get(c["case"], {}).get("status", "-") for c in ENTRIES["cases"]) + " |")
    (out / "matrix.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    failed = [eid for eid, m in matrix.items() if any(r["status"] == "fail" for r in m["cases"].values())]
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
