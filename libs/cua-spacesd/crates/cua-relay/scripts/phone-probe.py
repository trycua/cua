#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""A paired phone, as a script, against the local harness relay (never production).

It does what the phone app does, with the standard library and the openssl
binary only, so its bytes are a reference for the app:

  enroll                 make a P-256 key, enroll it as a device of account ADA
  machines               the account's machines (needs the device session)
  call TOOL [JSON]       HostThreadService.CallThreadTool over gRPC-Web through
                         the relay, on --machine (default: the harness machine)
  push-register [TOKEN]  PUT /v1/push/registrations/<device id>
  push-status            GET /v1/push
  alerts                 the alerts the relay's log provider wrote

State lives under $CUA_HARNESS_DIR (the key, 0600). Environment: source
`mobile-harness.sh env` first.
"""
import base64, hashlib, json, os, struct, subprocess, sys, tempfile, time, urllib.request, urllib.error

D = os.environ.get("CUA_HARNESS_DIR") or sys.exit("source `mobile-harness.sh env` first")
RELAY = os.environ["CUA_HARNESS_RELAY_URL"]
MINT = os.environ["CUA_HARNESS_MINT_URL"]
SUB = os.environ.get("PHONE_ACCOUNT", "ada")
KEY = os.path.join(D, f"phone-{SUB}.pem")


def b64u(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def http(method, url, headers=None, body=None):
    req = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def mint():
    s, _, b = http("POST", MINT, {"content-type": "application/json"}, json.dumps({"sub": SUB}).encode())
    return json.loads(b)["access_token"]


def openssl(*args, stdin=None):
    return subprocess.run(["openssl", *args], input=stdin, capture_output=True, check=True).stdout


def key_pair():
    if not os.path.exists(KEY):
        fd = os.open(KEY, os.O_WRONLY | os.O_CREAT, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(openssl("ecparam", "-genkey", "-name", "prime256v1", "-noout"))
    der = openssl("ec", "-in", KEY, "-pubout", "-outform", "DER")
    pub = der[-65:]
    assert pub[0] == 4
    return pub


def device_id(pub): return "dev_" + hashlib.sha256(pub).hexdigest()[:24]


def sign(message):
    der = openssl("dgst", "-sha256", "-sign", KEY, stdin=message.encode())
    # DER (r, s) -> fixed 64 bytes
    assert der[0] == 0x30
    i = 2 if der[1] < 0x80 else 2 + (der[1] & 0x7F)
    parts = []
    for _ in range(2):
        assert der[i] == 2
        n = der[i + 1]
        parts.append(int.from_bytes(der[i + 2:i + 2 + n], "big").to_bytes(32, "big"))
        i += 2 + n
    return b64u(b"".join(parts))


def account_headers(extra=None):
    h = {"authorization": "Bearer " + mint(), "content-type": "application/json"}
    h.update(extra or {})
    return h


def enroll():
    pub = key_pair()
    did = device_id(pub)
    ts = int(time.time())
    body = {"public_key": b64u(pub), "name": "harness phone", "platform": "ios", "ts": ts,
            "sig": sign(f"cua-device-register/v1\n{did}\n{ts}"), "bootstrap": True}
    s, _, b = http("POST", RELAY + "/v1/devices", account_headers(), json.dumps(body).encode())
    print(s, b.decode()[:300])
    return did


def session():
    pub = key_pair()
    did = device_id(pub)
    ts = int(time.time())
    body = {"device_id": did, "ts": ts, "sig": sign(f"cua-device-session/v1\n{did}\n{ts}")}
    s, _, b = http("POST", RELAY + "/v1/devices/session", account_headers(), json.dumps(body).encode())
    if s != 200:
        sys.exit(f"session: {s} {b.decode()}")
    return json.loads(b)["session"]


def with_session():
    return account_headers({"x-cua-device-session": session()})


def machines():
    s, _, b = http("GET", RELAY + "/v1/machines", with_session())
    if s != 200:
        sys.exit(f"machines: {s} {b.decode()}")
    return json.loads(b)["machines"]


def default_machine():
    name = os.environ.get("CUA_HARNESS_MACHINE", "harness-machine")
    for m in machines():
        if m["name"] == name:
            return m["id"]
    sys.exit("no machine named " + name)


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def pb_string(field, s):
    raw = s.encode()
    return varint(field << 3 | 2) + varint(len(raw)) + raw


def pb_fields(buf):
    i, out = 0, {}
    while i < len(buf):
        tag = 0
        shift = 0
        while True:
            b = buf[i]; i += 1
            tag |= (b & 0x7F) << shift; shift += 7
            if not b & 0x80: break
        wire = tag & 7
        if wire == 2:
            n = 0; shift = 0
            while True:
                b = buf[i]; i += 1
                n |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80: break
            out[tag >> 3] = buf[i:i + n]; i += n
        elif wire == 0:
            v = 0; shift = 0
            while True:
                b = buf[i]; i += 1
                v |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80: break
            out[tag >> 3] = v
        else:
            raise SystemExit("unexpected wire type")
    return out


def call_tool(machine, space, tool, args):
    msg = pb_string(1, space) + pb_string(2, tool) + pb_string(3, json.dumps(args))
    body = b"\x00" + struct.pack(">I", len(msg)) + msg
    h = with_session()
    h["content-type"] = "application/grpc-web+proto"
    h["x-grpc-web"] = "1"
    s, hd, raw = http("POST", f"{RELAY}/m/{machine}/cua.env.v1.HostThreadService/CallThreadTool", h, body)
    if s != 200:
        sys.exit(f"relay: {s} {raw.decode(errors='replace')}")
    if hd.get("grpc-status", "0") != "0":
        sys.exit(f"grpc {hd['grpc-status']}: {hd.get('grpc-message', '')}")
    # frames: data (0x00) then trailers (0x80)
    i, data, trailers = 0, b"", b""
    while i < len(raw):
        flag, n = raw[i], struct.unpack(">I", raw[i + 1:i + 5])[0]
        frame = raw[i + 5:i + 5 + n]
        if flag & 0x80: trailers = frame
        else: data = frame
        i += 5 + n
    import re
    m = re.search(rb"grpc-status:\s*(\d+)", trailers)
    if m and m.group(1) != b"0":
        sys.exit("grpc: " + trailers.decode(errors="replace"))
    f = pb_fields(data)
    text = json.loads(f.get(2, b"").decode() or "null") if f.get(2) else None
    content = json.loads((f.get(1) or b"[]").decode())
    first = content[0]["text"] if content and content[0].get("type") == "text" else None
    return {"is_error": bool(f.get(3, 0)), "structured": text,
            "result": json.loads(first) if first and first[:1] in "{[" else first}


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    flags = {}
    rest = []
    it = iter(sys.argv[2:])
    for a in it:
        if a in ("--machine", "--space"): flags[a[2:]] = next(it)
        else: rest.append(a)
    if cmd == "enroll":
        print(enroll())
    elif cmd == "machines":
        print(json.dumps(machines(), indent=2))
    elif cmd == "call":
        machine = flags.get("machine") or default_machine()
        args = json.loads(rest[1]) if len(rest) > 1 else {}
        space = flags.get("space") or "relay:" + machine
        args.setdefault("space", space)
        print(json.dumps(call_tool(machine, space, rest[0], args), indent=2))
    elif cmd == "push-register":
        did = device_id(key_pair())
        token = rest[0] if rest else hashlib.sha256(did.encode()).hexdigest()
        body = {"platform": "ios", "provider": "apns", "token": token, "app_id": "ai.cua.spaces.mobile",
                "env": "sandbox", "categories": ["needs_you", "done"], "locale": "en"}
        s, _, b = http("PUT", f"{RELAY}/v1/push/registrations/{did}", with_session(), json.dumps(body).encode())
        print(s, b.decode())
    elif cmd == "push-status":
        s, _, b = http("GET", RELAY + "/v1/push", with_session())
        print(s, b.decode())
    elif cmd == "alerts":
        p = os.environ.get("CUA_HARNESS_PUSH_ALERTS", "")
        print(open(p).read() if os.path.exists(p) else "(none)")
    else:
        print(__doc__)
        sys.exit(2)


main()
