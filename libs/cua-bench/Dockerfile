# cua-bench container for local batch processing
FROM python:3.12-slim

# Install system dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    python3-dev \
    portaudio19-dev \
    libsndfile1 \
    bash \
    && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y nodejs \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; Debian has not shipped that yet, so disable both roots explicitly (a no-op once they are gone).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI
RUN npm install -g @anthropic-ai/claude-code

WORKDIR /app

# Copy the cua-bench package
COPY cua_bench /app/cua_bench
COPY pyproject.toml /app/pyproject.toml
COPY README.md /app/README.md

# Install Python dependencies as root
RUN python -m pip install --upgrade pip && \
    python -m pip install --no-cache-dir -e .[all]

# Install cua-agent and non torch optional deps
RUN python -m pip install --no-cache-dir soundfile cua-agent[qwen,gemini] cua-som && \
    python -m pip install --no-cache-dir opentelemetry-sdk opentelemetry-exporter-otlp-proto-http

# Create a non-root user and set ownership
RUN useradd -m -u 1000 cuauser && \
    chown -R cuauser:cuauser /app && \
    mkdir -p /home/cuauser/.cache && \
    chown -R cuauser:cuauser /home/cuauser/.cache

# Switch to non-root user
USER cuauser

# Set PATH for user installations
ENV PATH="/home/cuauser/.local/bin:$PATH"

# Create Claude settings directory and configure default permissions for the user
RUN mkdir -p ~/.claude && \
    echo '{\n  "permissions": {\n    "defaultMode": "bypassPermissions"\n  }\n}' > ~/.claude/settings.json

# CLAUDE_CODE_OAUTH_TOKEN should be passed at runtime via -e flag, not baked into image
ENV PYTHONUNBUFFERED="TRUE" 

# Default command (will be overridden by batch script)
CMD ["python", "-m", "cua_bench.batch.solver"]
