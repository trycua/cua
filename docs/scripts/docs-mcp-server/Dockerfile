# syntax=docker/dockerfile:1
FROM python:3.12-slim AS builder

# Install uv
# Pinned: `uv:latest` would let the build tool itself drift between rebuilds.
COPY --from=ghcr.io/astral-sh/uv:0.6.17 /uv /uvx /bin/

# Set working directory
WORKDIR /app

# Copy project files
COPY pyproject.toml uv.lock ./

ENV VIRTUAL_ENV=/app/.venv
ENV PATH="/app/.venv/bin:$PATH"

# Install dependencies from uv.lock.
#
# --locked is the point of this file: it installs exactly what uv.lock
# records and fails if the lock is out of date with pyproject.toml, instead
# of silently re-resolving. Without it, two builds of the same commit months
# apart produce different dependency trees, which is how the reader's lancedb
# drifted away from the indexer's and broke vector search.
#
# --no-install-project: application modules are copied in, not packaged.
RUN uv sync --locked --no-dev --no-install-project

# Copy application code
COPY main.py docs_index.py ./

# Production image
FROM python:3.12-slim AS runtime

# Create non-root user for security
RUN useradd --create-home --shell /bin/bash appuser

WORKDIR /app

# Copy virtual environment from builder
COPY --from=builder /app/.venv /app/.venv
COPY --from=builder /app/main.py /app/docs_index.py /app/

# Set environment variables
ENV VIRTUAL_ENV=/app/.venv
ENV PATH="/app/.venv/bin:$PATH"
ENV PYTHONUNBUFFERED=1
ENV PORT=8000
ENV HOST=0.0.0.0

# Database paths (mount volumes to these paths)
ENV DOCS_DB_PATH=/data/docs_db
ENV CODE_DB_PATH=/data/code_db

# OpenTelemetry configuration
ENV OTEL_ENDPOINT=https://otel.cua.ai
ENV OTEL_SERVICE_NAME=cua-docs-mcp

# Create data directory
RUN mkdir -p /data && chown appuser:appuser /data

# Switch to non-root user
USER appuser

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8000/health')" || exit 1

# Run the server
CMD ["python", "main.py"]
