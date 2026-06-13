# Cua Docker XFCE Container
# Vanilla XFCE desktop with noVNC and computer-server
# Uses the official KiCad 9 image as base (Debian Bookworm + KiCad 9 pre-installed)

FROM kicad/kicad:9.0

# Avoid prompts from apt
ENV DEBIAN_FRONTEND=noninteractive

# Set environment variables
ENV HOME=/home/cua
ENV DISPLAY=:1
ENV VNC_PORT=5901
ENV NOVNC_PORT=6901
ENV API_PORT=8000
ENV VNC_RESOLUTION=1024x768
ENV VNC_COL_DEPTH=24

USER root

# Install system dependencies
RUN apt-get update && apt-get install -y \
    # System utilities
    sudo \
    unzip \
    zip \
    xdg-utils \
    # DHCP client for networking
    isc-dhcp-client \
    # Qt/XCB runtime deps for KiCad/XFCE
    libxcb-icccm4 \
    libxcb-image0 \
    libxcb-keysyms1 \
    libxcb-render-util0 \
    libxcb-xinerama0 \
    libxcb-shape0 \
    libxcb-randr0 \
    libxcb-xfixes0 \
    libxcb-sync1 \
    libxcb-util1 \
    libxcb-cursor0 \
    libxkbcommon-x11-0 \
    # Desktop environment
    xfce4 \
    xfce4-terminal \
    dbus-x11 \
    # VNC server
    tigervnc-standalone-server \
    tigervnc-tools \
    # noVNC dependencies
    git \
    net-tools \
    netcat-openbsd \
    supervisor \
    # computer-server dependencies (screenshot, mouse, clipboard)
    gnome-screenshot \
    wmctrl \
    ffmpeg \
    socat \
    xclip \
    # Browser (used by agent)
    wget \
    ca-certificates \
    firefox-esr \
    # GTK/WebKit for pywebview (cua-bench-ui task display)
    gir1.2-gtk-3.0 \
    gir1.2-webkit2-4.1 \
    libgirepository1.0-dev \
    libcairo2-dev \
    pkg-config \
    gobject-introspection \
    # Python pip to bootstrap uv
    python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Install uv, Python 3.13, and create venv at /opt/venv
# Python 3.13 satisfies cua-bench-ui's >=3.12 requirement (Bookworm only has 3.11)
RUN pip3 install --break-system-packages uv && \
    uv python install 3.13 && \
    uv venv /opt/venv --python 3.13 && \
    ln -sf /opt/venv/bin/python3 /usr/local/bin/python3 && \
    ln -sf /opt/venv/bin/python3 /usr/local/bin/python

# Remove screensavers and power manager to avoid popups and lock screens
RUN apt-get remove -y \
    xfce4-power-manager \
    xfce4-screensaver \
    light-locker \
    xscreensaver || true

# Create cua user
RUN useradd -m -s /bin/bash -G sudo cua && \
    echo "cua:cua" | chpasswd && \
    echo "cua ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

# Configure Firefox ESR
RUN echo 'pref("datareporting.policy.firstRunURL", "");\npref("datareporting.policy.dataSubmissionEnabled", false);\npref("datareporting.healthreport.service.enabled", false);\npref("datareporting.healthreport.uploadEnabled", false);\npref("trailhead.firstrun.branches", "nofirstrun-empty");\npref("browser.aboutwelcome.enabled", false);' > /usr/lib/firefox-esr/browser/defaults/preferences/firefox.js && \
    update-alternatives --install /usr/bin/x-www-browser x-www-browser /usr/bin/firefox-esr 100 && \
    update-alternatives --install /usr/bin/gnome-www-browser gnome-www-browser /usr/bin/firefox-esr 100

# Install noVNC
RUN git clone https://github.com/trycua/noVNC.git /opt/noVNC && \
    git clone https://github.com/novnc/websockify /opt/noVNC/utils/websockify && \
    ln -s /opt/noVNC/vnc.html /opt/noVNC/index.html

# Install Python packages into /opt/venv (Python 3.13)
RUN uv pip install --python /opt/venv/bin/python3 \
    cua-computer-server \
    "cua-agent[all]" \
    "pywebview[gtk]" \
    "cua-bench-ui>=0.7.0" \
    "aiohttp>=3.9.0" \
    "aiofiles>=23.0.0" \
    "websockets>=12.0" \
    "rich>=14.2.0" \
    "tqdm>=4.66.0" \
    "jinja2>=3.1.0" \
    "pyquery>=2.0.1" \
    "html5lib>=1.1" \
    "beautifulsoup4>=4.14.2" \
    "pillow>=11.0.0" \
    "numpy>=2.0.0" \
    "python-dotenv>=1.0.0" \
    "psutil>=5.9.0" \
    "datasets>=3.0.0" \
    "matplotlib>=3.8.0" \
    "anthropic>=0.26.0"

# Copy startup scripts
COPY src/supervisor/ /etc/supervisor/conf.d/
COPY src/scripts/ /usr/local/bin/
RUN chmod +x /usr/local/bin/*.sh

# Set ownership and switch to cua user
RUN chown -R cua:cua /home/cua
USER cua
WORKDIR /home/cua

# Create VNC directory
RUN mkdir -p $HOME/.vnc

# Configure XFCE for first start
RUN mkdir -p $HOME/.config/xfce4/xfconf/xfce-perchannel-xml $HOME/.config/xfce4 $HOME/.config/autostart

# Copy XFCE config to disable browser launching and welcome screens
COPY --chown=cua:cua src/xfce-config/helpers.rc $HOME/.config/xfce4/helpers.rc
COPY --chown=cua:cua src/xfce-config/xfce4-session.xml $HOME/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-session.xml
COPY --chown=cua:cua src/xfce-config/xfce4-power-manager.xml $HOME/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-power-manager.xml

# Disable autostart for screensaver, lock screen, and power manager
RUN echo "[Desktop Entry]\nHidden=true" > $HOME/.config/autostart/xfce4-tips-autostart.desktop && \
    echo "[Desktop Entry]\nHidden=true" > $HOME/.config/autostart/xfce4-screensaver.desktop && \
    echo "[Desktop Entry]\nHidden=true" > $HOME/.config/autostart/light-locker.desktop && \
    echo "[Desktop Entry]\nHidden=true" > $HOME/.config/autostart/xfce4-power-manager.desktop

# Create storage and shared directories
RUN mkdir -p $HOME/storage $HOME/shared $HOME/.cache/dconf $HOME/.mozilla/firefox

USER root

# Expose ports
EXPOSE $VNC_PORT $NOVNC_PORT $API_PORT

# Start services via supervisor
CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/supervisord.conf"]
