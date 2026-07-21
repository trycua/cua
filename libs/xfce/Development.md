# Development

## Building the Development Docker Image

To build the XFCE container with local computer-server changes:

```bash
cd libs/xfce
docker build -f Dockerfile.dev -t cua-xfce:dev ..
```

The build context is set to the parent directory to allow copying the local `computer-server` source.

## Tagging the Image

To tag the dev image as latest:

```bash
docker tag cua-xfce:dev cua-xfce:latest
```

## Running the Development Container

```bash
docker run -p 6901:6901 -p 8000:8000 cua-xfce:dev
```

## Direct Cua Driver proof of concept

The release Dockerfile can build an intentionally incompatible variant that
does not install or run `cua-computer-server`:

```bash
docker build \
  --build-arg CUA_GUI_RUNTIME=cua-driver \
  -t cua-xfce:driver-poc \
  libs/xfce
docker run --name cua-xfce-driver -p 6901:6901 cua-xfce:driver-poc
docker exec cua-xfce-driver /usr/local/bin/verify-cua-driver-sdk.py
```

The verification script is a co-located client of the Python `cua-driver` SDK.
It declares a desktop-scope session, reads the screen size, and captures the
whole desktop through the local Rust daemon. It also sends a harmless input
round-trip by moving the cursor to its current position. The daemon runs in its
default `standard` authorization mode; Linux has no macOS permissions gate.

This variant has no port 8000 service. Existing `cua-computer`, sandbox, bench,
CLI, and remote MCP consumers therefore cannot connect to it. That is the
architectural question this POC is intended to make concrete: a local SDK can
replace the GUI implementation but not a remotely reachable sandbox control
plane.

Access noVNC at: http://localhost:6901
