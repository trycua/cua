# Building the XFCE Docker Image

## Required Files for Build

The Dockerfile requires these files to be present in the `libs/xfce/` directory:

- `browser.py` - Copy from `libs/python/computer-server/computer_server/browser.py`
- `main.py` - Copy from `libs/python/computer-server/computer_server/main.py`

These files are copied into the container to include the browser tool functionality
that isn't yet in the published PyPI package.

## Before Building

```bash
# Copy the latest browser tool files
cp libs/python/computer-server/computer_server/browser.py libs/xfce/
cp libs/python/computer-server/computer_server/main.py libs/xfce/
```

## Build Command

```bash
cd libs/xfce
docker build -t cua-xfce .
```

## Note

Once the browser tool is included in the published `cua-computer-server` package,
these temporary file copies can be removed and the Dockerfile can be simplified.

