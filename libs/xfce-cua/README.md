# xfce-cua

`trycua/xfce-cua` is Cua's application-neutral Linux desktop image. It is
built directly on Debian and published for both `linux/amd64` and
`linux/arm64`.

It includes:

- XFCE on an X11 VNC session
- browser access through noVNC
- Firefox ESR
- Cua Driver and its Python SDK
- ffmpeg for Cua Driver trajectory video recording

It intentionally does not include KiCad or another application suite.
Application-specific images can build on top of this image without constraining
the base desktop to one CPU architecture.

## Run

```bash
docker run --rm \
  -p 6901:6901 \
  -e VNC_PW=cua \
  trycua/xfce-cua:latest
```

Open <http://localhost:6901/vnc.html> and use the password supplied through
`VNC_PW`.

## Use Cua Driver inside the desktop

The installed executable and Python SDK use the container's native CPU
architecture:

```bash
docker exec -u cua <container> cua-driver --version
docker exec -u cua <container> \
  python -c "from cua_driver import CuaDriver; print(CuaDriver)"
```

Cua Driver is the only computer-use runtime included. It does not auto-start
because its lifecycle belongs to the client running inside the sandbox. Start
it against the XFCE session when needed:

```bash
docker exec -d -u cua \
  -e HOME=/home/cua \
  -e DISPLAY=:1 \
  <container> \
  cua-driver serve --dangerously-bypass-approvals
```

## Build

```bash
docker buildx build --platform linux/amd64,linux/arm64 libs/xfce-cua
```
