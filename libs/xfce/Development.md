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

Access noVNC at: http://localhost:6901
