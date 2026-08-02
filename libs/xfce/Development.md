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

Verify that the image contains both the Python SDK and bundled executable:

```bash
docker run --rm cua-xfce:dev cua-driver --version
docker run --rm cua-xfce:dev \
  python -c "from cua_driver import CuaDriver, get_binary_path; print(get_binary_path())"
```

Installing Cua Driver does not replace or start another GUI service. The image
continues to expose computer-server on port 8000. A future transport adapter can
host the typed Python SDK without coupling the base image to HTTP, gRPC, or
another remote protocol.

Access noVNC at: http://localhost:6901
