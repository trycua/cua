# cua-bench container for local batch processing
FROM python:3.12-slim

# Install system dependencies required by Playwright and other tools
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    python3-dev \
    portaudio19-dev \
    bash \
    # Playwright system dependencies
    libnss3 \
    libnspr4 \
    libatk-bridge2.0-0 \
    libdrm2 \
    libxkbcommon0 \
    libxcomposite1 \
    libxdamage1 \
    libxrandr2 \
    libgbm1 \
    libxss1 \
    libasound2 \
    libxfixes3 \
    && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y nodejs \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI
RUN npm install -g @anthropic-ai/claude-code

WORKDIR /app

# Copy the cua-bench package
COPY cua_bench /app/cua_bench
COPY pyproject.toml /app/pyproject.toml
COPY README.md /app/README.md

# Install Python dependencies and Playwright as root
RUN python -m pip install --upgrade pip && \
    python -m pip install --no-cache-dir -e .[all] && \
    playwright install --with-deps chromium

# Create a non-root user and set ownership
RUN useradd -m -u 1000 cuauser && \
    chown -R cuauser:cuauser /app && \
    mkdir -p /home/cuauser/.cache && \
    cp -r /root/.cache/ms-playwright /home/cuauser/.cache/ 2>/dev/null || true && \
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
