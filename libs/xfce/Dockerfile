# Multi-architecture XFCE desktop with the legacy computer-server API expected
# by cua-sandbox's DockerRuntime.
ARG XFCE_CUA_VERSION=0.1.0
FROM trycua/xfce-cua:${XFCE_CUA_VERSION}

LABEL org.opencontainers.image.title="Cua XFCE Computer Server"
LABEL org.opencontainers.image.description="Multi-architecture XFCE desktop with cua-computer-server"
LABEL org.opencontainers.image.source="https://github.com/trycua/cua"

ENV API_PORT=8000

USER root

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    gnome-screenshot \
    python3-dev \
    socat \
    wmctrl \
    xclip \
    && apt-get clean \
    && find /var/lib/apt/lists -mindepth 1 -delete

COPY requirements-computer-server.txt /tmp/requirements-computer-server.txt
RUN /opt/venv/bin/python -m pip install --no-cache-dir \
        -r /tmp/requirements-computer-server.txt \
    && /opt/venv/bin/python -c \
        "import computer_server; print(computer_server.__file__)" \
    && apt-get purge -y build-essential python3-dev \
    && apt-get autoremove -y \
    && apt-get clean \
    && find /var/lib/apt/lists -mindepth 1 -delete

COPY src/scripts/start-computer-server.sh /usr/local/bin/start-computer-server.sh
COPY src/supervisor/supervisord.conf /etc/supervisor/supervisord.conf
RUN chmod +x /usr/local/bin/start-computer-server.sh

EXPOSE 5901 6901 8000

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
  CMD curl --fail --silent "http://127.0.0.1:${API_PORT}/status" >/dev/null || exit 1

CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/supervisord.conf"]
