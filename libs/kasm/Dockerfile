FROM kasmweb/core-ubuntu-jammy:1.17.0
USER root

ENV HOME=/home/kasm-default-profile
ENV STARTUPDIR=/dockerstartup
ENV INST_SCRIPTS=$STARTUPDIR/install
WORKDIR $HOME

######### Customize Container Here ###########

# Installing python, pip, and libraries (but don't change default yet)
RUN apt-get update && \
    apt install -y wget build-essential libncursesw5-dev libssl-dev \
    libsqlite3-dev tk-dev libgdbm-dev libc6-dev libbz2-dev libffi-dev zlib1g-dev && \
    add-apt-repository ppa:deadsnakes/ppa && \
    apt-get update && \
    apt install -y python3.12 python3.12-dev python3.12-tk \
    gnome-screenshot wmctrl ffmpeg socat xclip curl && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Firefox (do this BEFORE changing default Python, as add-apt-repository needs system Python)
ENV DEBIAN_FRONTEND=noninteractive \
    INST_DIR=$STARTUPDIR/install
COPY ./src/ $INST_DIR
RUN bash ${INST_DIR}/ubuntu/install/firefox/install_firefox.sh

# Now set Python 3.12 as the default python3
RUN update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.12 1 && \
    update-alternatives --set python3 /usr/bin/python3.12

# Install pip for Python 3.12
RUN curl -sS https://bootstrap.pypa.io/get-pip.py | python3.12

# Disable SSL requirement
RUN sed -i 's/require_ssl: true/require_ssl: false/g' /usr/share/kasmvnc/kasmvnc_defaults.yaml
RUN sed -i 's/-sslOnly//g' /dockerstartup/vnc_startup.sh

# Enable sudo support for kasm-user
RUN echo "kasm-user:password" | chpasswd
RUN usermod -aG sudo kasm-user
RUN echo "kasm-user ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

######### End Customizations ###########

RUN chown 1000:0 $HOME
RUN $STARTUPDIR/set_user_permission.sh $HOME
ENV HOME=/home/kasm-user
WORKDIR $HOME
RUN mkdir -p $HOME && chown -R 1000:0 $HOME

# Install UV package manager
RUN python3.12 -m pip install uv

# Install cua-spacesd, the in-sandbox daemon (gRPC + gRPC-Web on :3211).
# Same artifact as libs/cua-spacesd/packaging/install.sh installs.
ARG CUA_SPACESD_VERSION=0.1.0
# release (default) installs the published cua-spacesd; none leaves it out
# (pull-request builds before that version is published).
ARG CUA_SPACESD_SOURCE=release
ARG TARGETARCH
RUN set -eux; \
    if [ "$CUA_SPACESD_SOURCE" = none ]; then \
      echo "CUA_SPACESD_SOURCE=none: cua-spacesd not installed"; exit 0; \
    fi; \
    [ "$CUA_SPACESD_SOURCE" = release ] || { echo "CUA_SPACESD_SOURCE must be release or none" >&2; exit 1; }; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64|x86_64) arch=x86_64 ;; \
      arm64|aarch64) arch=aarch64 ;; \
      *) echo "unsupported arch" >&2; exit 1 ;; \
    esac; \
    url="https://github.com/trycua/cua/releases/download/cua-spacesd-v${CUA_SPACESD_VERSION}/cua-spacesd-${arch}-unknown-linux-gnu.tar.gz"; \
    curl -fsSL "$url" | tar -xz -C /usr/local/bin cua-spacesd; \
    chmod 0755 /usr/local/bin/cua-spacesd; \
    ln -s cua-spacesd /usr/local/bin/cua-guestd; \
    ln -s cua-spacesd /usr/local/bin/cua-env-driver  # older names, one release

# Install playwright and Firefox dependencies
RUN uv pip install --system playwright && \
    python3.12 -m playwright install --with-deps firefox

# Clean up uv cache to avoid issues and reduce size
RUN rm -rf /home/kasm-user/.cache/uv /root/.cache/uv

# Fix ownership of remaining cache files
RUN chown -R 1000:0 /home/kasm-user/.cache 2>/dev/null || true

# Start cua-spacesd on 0.0.0.0:3211 at session start. The token comes from
# CUA_ENV_TOKEN or /run/cua/env-token; one is generated there when absent.
COPY ./start-spacesd.sh $STARTUPDIR/custom_startup.sh
RUN chmod +x $STARTUPDIR/custom_startup.sh

EXPOSE 3211

USER 1000