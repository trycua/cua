# Development

## Building the Development Docker Image

To build the XFCE container with a locally built cua-spacesd:

```bash
cd libs/xfce
docker build -f Dockerfile.dev -t cua-xfce:dev ..
```

The build context is set to the parent directory (`libs/`) so the image can copy
`cua-spacesd/target/<target>/release/cua-spacesd`; build it first with
`cargo build --release -p cua-spacesd --target x86_64-unknown-linux-gnu` in
`libs/cua-spacesd`.

## Tagging the Image

To tag the dev image as latest:

```bash
docker tag cua-xfce:dev cua-xfce:latest
```

## Running the Development Container

```bash
docker run -p 6901:6901 -p 3211:3211 cua-xfce:dev
```

Verify that the image contains both the Python SDK and bundled executable:

```bash
docker run --rm cua-xfce:dev cua-driver --version
docker run --rm cua-xfce:dev \
  python -c "from cua_driver import CuaDriver, get_binary_path; print(get_binary_path())"
```

Installing Cua Driver does not start another GUI service. The image's remote API
is cua-spacesd on port 3211 (token in `/run/cua/env-token`).

Access noVNC at: http://localhost:6901
