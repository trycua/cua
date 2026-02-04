# Minimal xpra container with agent-browser
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

# Create non-root user (but stay as root for now)
RUN useradd -m -d /home/user user && \
    chown -R user /home/user

# Install base dependencies (as root, no sudo needed)
RUN apt-get update && apt-get install -y \
    curl \
    ca-certificates \
    gnupg \
    wget \
    apt-transport-https \
    software-properties-common \
    python3 \
    python3-pip \
    lsb-release \
    netcat-openbsd \
    git

# Install xpra from official repo (must be v6+)
RUN apt-get update \
    && wget -O /usr/share/keyrings/xpra.asc https://xpra.org/xpra.asc \
    && cd /etc/apt/sources.list.d && wget https://raw.githubusercontent.com/Xpra-org/xpra/master/packaging/repos/jammy/xpra.sources \
    && apt-get update \
    && apt-get install -y xpra \
    && xpra --version \
    && XPRA_VERSION=$(xpra --version | grep -oP 'v\K[0-9]+' | head -1) \
    && if [ "$XPRA_VERSION" -lt 5 ]; then echo "ERROR: xpra version must be >= 5, got v$XPRA_VERSION"; exit 1; fi

# Install Node.js 20.x
RUN mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor | tee /etc/apt/keyrings/nodesource.gpg > /dev/null \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_20.x nodistro main" | tee /etc/apt/sources.list.d/nodesource.list \
    && apt-get update && apt-get install -y nodejs

# Install remaining packages
RUN apt-get install -y \
    x11-apps \
    dbus \
    dbus-x11 \
    feh \
    xdg-utils \
    && npm install -g agent-browser \
    && DEBIAN_FRONTEND=noninteractive npx playwright install-deps \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Chromium from xtradeb PPA (avoids snap requirement on Ubuntu 22.04)
RUN add-apt-repository ppa:xtradeb/apps -y \
    && apt-get update \
    && apt-get install -y chromium \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Create chromium wrapper that adds --no-sandbox (required in container)
RUN mv /usr/bin/chromium /usr/bin/chromium-bin \
    && printf '#!/bin/bash\nexec /usr/bin/chromium-bin --no-sandbox "$@"' > /usr/bin/chromium \
    && chmod +x /usr/bin/chromium

# Setup dbus and create runtime directories
RUN mkdir -p /run/dbus /tmp/runtime-user \
    && dbus-uuidgen | tee /var/lib/dbus/machine-id > /dev/null \
    && chmod 777 /run/dbus /tmp/runtime-user

# Install sudo for runtime use (after all root operations)
RUN apt-get update && apt-get install -y sudo \
    && adduser user sudo \
    && echo '%sudo ALL=(ALL) NOPASSWD:ALL' >> /etc/sudoers \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Now switch to non-root user for user-specific setup
USER user
WORKDIR /home/user

ENV PATH="/home/user/.local/bin:${PATH}"

# Install agent-browser as user
RUN agent-browser install

# Set Chromium as default browser (needed for some agent CLI onboardings)
RUN xdg-settings set default-web-browser chromium.desktop 2>/dev/null || true

## ==== preinstalled apps for the agent ====

# Python plotting libraries
RUN pip3 install --no-cache-dir matplotlib numpy pandas seaborn plotly

# Claude Code CLI
RUN curl -fsSL https://claude.ai/install.sh | bash

# Install uv (Python package manager for self-contained scripts)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh

# Add aliases so apt-get/apt work without explicit sudo
RUN echo "alias apt-get='sudo apt-get'" >> ~/.bashrc && \
    echo "alias apt='sudo apt'" >> ~/.bashrc

# Lazy-install wrapper scripts for agent CLIs
RUN mkdir -p ~/.local/bin && \
    printf '#!/bin/bash\nif ! command -v /usr/local/bin/codex &>/dev/null; then sudo npm install -g @openai/codex; fi\n/usr/local/bin/codex "$@"' > ~/.local/bin/codex && \
    printf '#!/bin/bash\nif ! command -v /usr/local/bin/aider &>/dev/null; then pip3 install --user aider-chat; fi\naider "$@"' > ~/.local/bin/aider && \
    printf '#!/bin/bash\nif ! command -v ~/.local/bin/openclaw-bin &>/dev/null; then curl -fsSL https://openclaw.ai/install.sh | bash && mv ~/.local/bin/openclaw ~/.local/bin/openclaw-bin; fi\n~/.local/bin/openclaw-bin "$@"' > ~/.local/bin/openclaw && \
    printf '#!/bin/bash\nif ! command -v ~/.local/bin/vibe-bin &>/dev/null; then curl -LsSf https://mistral.ai/vibe/install.sh | bash && mv ~/.local/bin/vibe ~/.local/bin/vibe-bin; fi\n~/.local/bin/vibe-bin "$@"' > ~/.local/bin/vibe && \
    printf '#!/bin/bash\nnpx @google/gemini-cli "$@"' > ~/.local/bin/gemini && \
    chmod +x ~/.local/bin/codex ~/.local/bin/aider ~/.local/bin/openclaw ~/.local/bin/vibe ~/.local/bin/gemini

ENV DISPLAY=:100
ENV XDG_RUNTIME_DIR=/tmp/runtime-user
ENV AGENT_BROWSER_HEADED=1

# Copy cuabot prompts
COPY --chown=user:user src/prompts/.mcp.json /home/user/.mcp.json
COPY --chown=user:user src/prompts/SYSTEM.md /home/user/CLAUDE.md

EXPOSE 10000

USER user

# Note: dbus-daemon runs as session bus (not system) to avoid needing root
CMD ["sh", "-c", "xpra --version && dbus-daemon --session --fork --address=unix:path=/tmp/dbus-session 2>/dev/null || true && xpra seamless :100 --sharing=yes --no-daemon --bind-tcp=0.0.0.0:10000 --html=on"]