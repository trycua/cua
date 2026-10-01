# Base image using Debian for arm64 architecture (optimized for Apple Silicon)
FROM debian:bookworm-slim AS lumier-base

# Set environment variables for Lume API server configuration
ENV LUME_API_HOST="host.docker.internal"

# Default VM configuration (can be overridden at runtime)
ENV VERSION="ghcr.io/trycua/macos-sequoia-vanilla:latest"
ENV RAM_SIZE="8192"
ENV CPU_CORES="4"
ENV DISK_SIZE="100"
ENV DISPLAY="1024x768"
ENV VM_NAME="lumier"
ENV HOST_SHARED_PATH=""
ENV LUMIER_DEBUG="0"

# Install necessary tools and noVNC dependencies
RUN apt-get update && \
    apt-get install -y \
    netcat-traditional \
    curl \
    sshpass \
    wget \
    unzip \
    git \
    python3 \
    python3-pip \
    python3-numpy \
    procps && \
    rm -rf /var/lib/apt/lists/*

# Download and install noVNC without caching
RUN wget https://github.com/trycua/noVNC/archive/refs/heads/master.zip -O master1.zip && \
    unzip master1.zip && \
    mv noVNC-master /opt/noVNC && \
    rm master1.zip

# Set environment variables for noVNC
ENV NOVNC_PATH="/opt/noVNC"

# Create necessary directories
RUN mkdir -p /run/bin /run/lib /run/config /run/hooks /run/lifecycle

# Copy scripts to the container
COPY src/config/constants.sh /run/config/
COPY src/bin/entry.sh /run/bin/entry.sh

# Copy library files if they exist
COPY src/lib/ /run/lib/
COPY src/hooks/ /run/hooks/

# Copy on-logon script to lifecycle directory
COPY src/hooks/on-logon.sh /run/lifecycle/

# Make scripts executable
RUN chmod +x \
    /run/bin/* \
    /run/hooks/* \
    /run/lifecycle/* 2>/dev/null || true

# Expose ports for noVNC and Lume API
EXPOSE 8006

# VOLUME setup
VOLUME [ "/storage" ]
VOLUME [ "/data" ]

# Default entrypoint
ENTRYPOINT ["/run/bin/entry.sh"]