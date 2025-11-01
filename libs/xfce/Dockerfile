# CUA Docker XFCE Container
# Vanilla XFCE desktop with noVNC and computer-server

FROM ubuntu:22.04

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

# Install system dependencies first (including sudo)
RUN apt-get update && apt-get install -y \
    # System utilities
    sudo \
    unzip \
    zip \
    xdg-utils \
    # Desktop environment
    xfce4 \
    xfce4-terminal \
    dbus-x11 \
    # VNC server
    tigervnc-standalone-server \
    tigervnc-common \
    # noVNC dependencies
    # python will be installed via deadsnakes as 3.12 \
    git \
    net-tools \
    netcat \
    supervisor \
    # Computer-server dependencies
    # python-tk/dev for 3.12 will be installed later \
    gnome-screenshot \
    wmctrl \
    ffmpeg \
    socat \
    xclip \
    # Browser
    wget \
    software-properties-common \
    # Build tools
    build-essential \
    libncursesw5-dev \
    libssl-dev \
    libsqlite3-dev \
    tk-dev \
    libgdbm-dev \
    libc6-dev \
    libbz2-dev \
    libffi-dev \
    zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Python 3.12 from deadsnakes (keep system python3 for apt)
RUN add-apt-repository -y ppa:deadsnakes/ppa && \
    apt-get update && apt-get install -y \
    python3.12 python3.12-venv python3.12-dev python3.12-tk && \
    python3.12 -m ensurepip --upgrade && \
    python3.12 -m pip install --upgrade pip setuptools wheel && \
    rm -rf /var/lib/apt/lists/*

# Ensure 'python' points to Python 3.12
RUN update-alternatives --install /usr/bin/python python /usr/bin/python3.12 2

# Remove screensavers and power manager to avoid popups and lock screens
RUN apt-get remove -y \
    xfce4-power-manager \
    xfce4-power-manager-data \
    xfce4-power-manager-plugins \
    xfce4-screensaver \
    light-locker \
    xscreensaver \
    xscreensaver-data || true

# Create user after sudo is installed
RUN useradd -m -s /bin/bash -G sudo cua && \
    echo "cua:cua" | chpasswd && \
    echo "cua ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

# Install Firefox from Mozilla PPA (snap-free) - inline to avoid script issues
RUN apt-get update && \
    add-apt-repository -y ppa:mozillateam/ppa && \
    echo 'Package: *\nPin: release o=LP-PPA-mozillateam\nPin-Priority: 1001' > /etc/apt/preferences.d/mozilla-firefox && \
    apt-get update && \
    apt-get install -y firefox && \
    echo 'pref("datareporting.policy.firstRunURL", "");\npref("datareporting.policy.dataSubmissionEnabled", false);\npref("datareporting.healthreport.service.enabled", false);\npref("datareporting.healthreport.uploadEnabled", false);\npref("trailhead.firstrun.branches", "nofirstrun-empty");\npref("browser.aboutwelcome.enabled", false);' > /usr/lib/firefox/browser/defaults/preferences/firefox.js && \
    update-alternatives --install /usr/bin/x-www-browser x-www-browser /usr/bin/firefox 100 && \
    update-alternatives --install /usr/bin/gnome-www-browser gnome-www-browser /usr/bin/firefox 100 && \
    rm -rf /var/lib/apt/lists/*

# Install noVNC
RUN git clone https://github.com/novnc/noVNC.git /opt/noVNC && \
    git clone https://github.com/novnc/websockify /opt/noVNC/utils/websockify && \
    ln -s /opt/noVNC/vnc.html /opt/noVNC/index.html

# Pre-create cache directory with correct ownership before pip install
RUN mkdir -p /home/cua/.cache && \
    chown -R cua:cua /home/cua/.cache

# Install computer-server using Python 3.12 pip
RUN python3.12 -m pip install cua-computer-server

# Fix any cache files created by pip
RUN chown -R cua:cua /home/cua/.cache

# Copy startup scripts
COPY src/supervisor/ /etc/supervisor/conf.d/
COPY src/scripts/ /usr/local/bin/

# Make scripts executable
RUN chmod +x /usr/local/bin/*.sh

# Setup VNC
RUN chown -R cua:cua /home/cua
USER cua
WORKDIR /home/cua

# Create VNC directory (no password needed with SecurityTypes None)
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
    echo "[Desktop Entry]\nHidden=true" > $HOME/.config/autostart/xfce4-power-manager.desktop && \
    chown -R cua:cua $HOME/.config

# Create storage and shared directories, and Firefox cache directory
RUN mkdir -p $HOME/storage $HOME/shared $HOME/.cache/dconf $HOME/.mozilla/firefox && \
    chown -R cua:cua $HOME/storage $HOME/shared $HOME/.cache $HOME/.mozilla $HOME/.vnc

USER root

# Expose ports
EXPOSE $VNC_PORT $NOVNC_PORT $API_PORT

# Start services via supervisor
CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/supervisord.conf"]
