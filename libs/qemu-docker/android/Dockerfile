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
    python3 \
    python3-pip \
    python3-venv \
    python3-dev \
    python3-tk \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

RUN /opt/venv/bin/pip install --no-cache-dir --upgrade pip && \
    /opt/venv/bin/pip install --no-cache-dir cua-computer-server

COPY src/entry.sh /usr/local/bin/entry.sh
RUN chmod +x /usr/local/bin/entry.sh

# Make venv accessible to androidusr
RUN chown -R 1300:1301 /opt/venv

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=10s --start-period=120s --retries=3 \
    CMD adb devices | grep -q "emulator" && curl -f http://localhost:8000/status || exit 1

# Switch back to androidusr user (as per base image)
USER 1300:1301

WORKDIR /home/androidusr

ENTRYPOINT ["/usr/local/bin/entry.sh"]
