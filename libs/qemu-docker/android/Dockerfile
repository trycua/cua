# ============================================================================
# Stage 1: Build wallpaper-manager APK
# ============================================================================
FROM eclipse-temurin:17-jdk AS builder

RUN apt-get update && apt-get install -y wget unzip dos2unix && \
    mkdir -p /opt/android-sdk/cmdline-tools && \
    cd /opt/android-sdk/cmdline-tools && \
    wget -q https://dl.google.com/android/repository/commandlinetools-linux-9477386_latest.zip && \
    unzip commandlinetools-linux-9477386_latest.zip && \
    rm commandlinetools-linux-9477386_latest.zip && \
    mv cmdline-tools latest

ENV ANDROID_HOME=/opt/android-sdk
ENV PATH="${ANDROID_HOME}/cmdline-tools/latest/bin:${ANDROID_HOME}/platform-tools:${PATH}"

RUN yes | sdkmanager --licenses && \
    sdkmanager "platforms;android-30" "build-tools;30.0.3"

COPY src/wallpaper-manager /build/wallpaper-manager
WORKDIR /build/wallpaper-manager

RUN curl -fsSL -o gradle/wrapper/gradle-wrapper.jar \
    https://raw.githubusercontent.com/gradle/gradle/v7.6.0/gradle/wrapper/gradle-wrapper.jar && \
    dos2unix gradlew && \
    chmod +x gradlew

RUN ./gradlew assembleDebug --no-daemon

# ============================================================================
# Stage 2: Runtime image with Android emulator & Computer server
# ============================================================================
FROM budtmo/docker-android:emulator_11.0

USER root

# Set environment variable to identify this as Cua Android container
ENV IS_CUA_ANDROID=true

# Copy wallpaper-manager APK from builder stage
COPY --from=builder /build/wallpaper-manager/app/build/outputs/apk/debug/app-debug.apk /opt/apks/wallpaper-manager.apk

RUN apt-get update && \
    apt-get install -y \
    software-properties-common \
    build-essential \
    curl \
    gcc \
    && add-apt-repository -y ppa:deadsnakes/ppa && \
    apt-get update && \
    apt-get install -y \
    python3.12 \
    python3.12-venv \
    python3.12-dev \
    python3.12-tk \
    && rm -rf /var/lib/apt/lists/*

# Install UV package manager
RUN curl -LsSf https://astral.sh/uv/install.sh | sh && \
    chmod 755 /root /root/.local /root/.local/bin /root/.local/bin/uv && \
    ln -sf /root/.local/bin/uv /usr/local/bin/uv && \
    uv --version

# Set Python 3.12 as default python3
RUN update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.12 1 && \
    update-alternatives --set python3 /usr/bin/python3.12

# Create UV project for cua-computer-server and cua-agent
RUN mkdir -p /opt/cua-server && \
    uv init --vcs none --no-readme --no-workspace --no-pin-python /opt/cua-server && \
    uv add --directory /opt/cua-server cua-computer-server "cua-agent[all]"

COPY src/entry.sh /usr/local/bin/entry.sh
RUN chmod +x /usr/local/bin/entry.sh

# Make UV project accessible to androidusr
RUN chown -R 1300:1301 /opt/cua-server

# Ensure UV is accessible in PATH for androidusr
ENV PATH="/usr/local/bin:$PATH"

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=10s --start-period=120s --retries=3 \
    CMD adb devices | grep -q "emulator" && curl -f http://localhost:8000/status || exit 1

# Switch back to androidusr user (as per base image)
USER 1300:1301

WORKDIR /home/androidusr

ENTRYPOINT ["/usr/local/bin/entry.sh"]
