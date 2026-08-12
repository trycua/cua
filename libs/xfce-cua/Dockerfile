# Vanilla Cua XFCE desktop.
#
# Keep this image application-neutral. Application-specific images should build
# on top of it instead of making the desktop runtime inherit their architecture
# constraints.
FROM python:3.13-slim-bookworm

LABEL org.opencontainers.image.title="Cua XFCE"
LABEL org.opencontainers.image.description="Multi-architecture XFCE desktop with Cua Driver"
LABEL org.opencontainers.image.source="https://github.com/trycua/cua"

ENV DEBIAN_FRONTEND=noninteractive \
    HOME=/home/cua \
    DISPLAY=:1 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
    VNC_PORT=5901 \
    NOVNC_PORT=6901 \
    VNC_RESOLUTION=1280x900 \
    VNC_COL_DEPTH=24 \
    PATH=/opt/venv/bin:$PATH

USER root

RUN apt-get update && apt-get install -y --no-install-recommends \
    at-spi2-core \
    ca-certificates \
    curl \
    dbus-x11 \
    ffmpeg \
    firefox-esr \
    fonts-dejavu-core \
    libglib2.0-bin \
    netcat-openbsd \
    novnc \
    procps \
    sudo \
    supervisor \
    tigervnc-standalone-server \
    tigervnc-tools \
    websockify \
    x11-xserver-utils \
    xdg-utils \
    xfce4 \
    xfce4-terminal \
    && apt-get clean \
    && find /var/lib/apt/lists -mindepth 1 -delete

RUN useradd --create-home --shell /bin/bash --groups sudo cua \
    && echo "cua ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/cua \
    && chmod 0440 /etc/sudoers.d/cua

COPY requirements.txt /tmp/requirements.txt

RUN python -m venv /opt/venv \
    && /opt/venv/bin/python -m pip install --no-cache-dir --upgrade pip \
    && /opt/venv/bin/python -m pip install --no-cache-dir \
        -r /tmp/requirements.txt

RUN /opt/venv/bin/python -c \
        "from pathlib import Path; from cua_driver import CuaDriver, get_binary_path; assert Path(get_binary_path()).is_file()" \
    && chown cua:cua "$(/opt/venv/bin/python -c 'from cua_driver import get_binary_path; print(get_binary_path())')" \
    && su -s /bin/bash -c \
        "HOME=/home/cua /opt/venv/bin/cua-driver --version" cua

RUN printf '%s\n' \
      'pref("datareporting.policy.firstRunURL", "");' \
      'pref("datareporting.policy.dataSubmissionEnabled", false);' \
      'pref("datareporting.healthreport.uploadEnabled", false);' \
      'pref("browser.aboutwelcome.enabled", false);' \
      > /etc/firefox-esr/cua.js

COPY src/supervisor/supervisord.conf /etc/supervisor/supervisord.conf
COPY src/scripts/ /usr/local/bin/
COPY --chown=cua:cua src/xfce/ /home/cua/.config/xfce4/

RUN chmod +x /usr/local/bin/*.sh \
    && mkdir -p \
        /home/cua/.cache/dconf \
        /home/cua/.config/autostart \
        /home/cua/.mozilla/firefox \
        /home/cua/.vnc \
        /home/cua/shared \
        /home/cua/storage \
        /run/user/1000 \
    && printf '[Desktop Entry]\nHidden=true\n' > /home/cua/.config/autostart/xfce4-power-manager.desktop \
    && printf '[Desktop Entry]\nHidden=true\n' > /home/cua/.config/autostart/xfce4-screensaver.desktop \
    && chown -R cua:cua /home/cua /run/user/1000 \
    && chmod 0700 /run/user/1000

WORKDIR /home/cua

EXPOSE 5901 6901

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
  CMD curl --fail --silent "http://127.0.0.1:${NOVNC_PORT}/vnc.html" >/dev/null || exit 1

CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/supervisord.conf"]
