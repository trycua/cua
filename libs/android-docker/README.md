# Android Docker

Docker image that runs an Android emulator with CUA Computer Server integration, enabling programmatic control of Android devices via HTTP API.

## Features

- **Android 11 Emulator** - Based on budtmo/docker-android
- **CUA Computer Server** - HTTP API for automation, file operations, window management
- **Custom Wallpaper Manager APK** - Programmatically set wallpapers without user interaction
- **VNC Access** - View and interact with the Android screen via web browser

## What's Inside

- **wallpaper-manager/** - Custom Android APK that uses WallpaperManager API to set wallpapers
- **entry.sh** - Container startup script that launches emulator and server
- **Dockerfile** - Production build (installs cua-computer-server from PyPI)
- **dev.Dockerfile** - Development build (uses local source code)

## Quick Start

### Production Build

```bash
cd android-docker
docker build -t cua-android .
docker run -d -p 6080:6080 -p 8000:8000 \
  -e EMULATOR_DEVICE="Samsung Galaxy S10" \
  -e WEB_VNC=true \
  --device /dev/kvm \
  --name android-container \
  cua-android
```

### Development Build

```bash
cd ..  # Go to libs/ directory
docker build -f android-docker/dev.Dockerfile -t cua-android:dev .
docker run -d -p 6080:6080 -p 8000:8000 \
  -e EMULATOR_DEVICE="Samsung Galaxy S10" \
  -e WEB_VNC=true \
  --device /dev/kvm \
  --name android-container \
  cua-android:dev
```

## Access Points

- **VNC Web UI**: http://localhost:6080
- **Computer Server API**: http://localhost:8000
- **API Documentation**: http://localhost:8000/docs

## API Examples

```bash
# Get screen size
curl -X POST http://localhost:8000/cmd \
  -H "Content-Type: application/json" \
  -d '{"command": "get_screen_size", "params": {}}'

# Set wallpaper (automatically handles permissions)
curl -X POST http://localhost:8000/cmd \
  -H "Content-Type: application/json" \
  -d '{"command": "set_wallpaper", "params": {"path": "/sdcard/image.jpg", "target": "home"}}'

# Launch app
curl -X POST http://localhost:8000/cmd \
  -H "Content-Type: application/json" \
  -d '{"command": "launch", "params": {"app": "com.android.settings"}}'
```

## Custom Wallpaper Solution

Android doesn't provide native ADB commands for setting wallpapers. We solved this by:

1. **Building a custom APK** (`wallpaper-manager`) that uses Android's WallpaperManager API
2. **Multi-stage Docker build** - APK is compiled during image build
3. **Auto-installation** - APK installs automatically on container startup
4. **Permission handling** - Files are copied to `/data/local/tmp` where all apps have read access
5. **Seamless integration** - `set_wallpaper()` API handles everything automatically
