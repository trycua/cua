#!/bin/bash
# Wrapper for agent-device
# System images are downloaded on-demand via "agent-device install"

export ANDROID_SDK_ROOT=/home/user/android-sdk
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
export PATH="${ANDROID_SDK_ROOT}/cmdline-tools/latest/bin:${ANDROID_SDK_ROOT}/platform-tools:${ANDROID_SDK_ROOT}/emulator:${PATH}"

# Check if nested virtualization is enabled
check_kvm() {
    if [ ! -e /dev/kvm ]; then
        echo "=============================================="
        echo "WARNING: Nested virtualization is not enabled!"
        echo "=============================================="
        echo ""
        echo "agent-device requires KVM for Android emulation."
        echo "Please restart cuabot with nested virtualization enabled:"
        echo ""
        echo "    CUABOT_NESTED_VIRT=1 cuabot"
        echo ""
        echo "=============================================="
        exit 1
    fi
}

# Download and install the system image
install_system_image() {
    check_kvm

    echo "[agent-device] Downloading Android system image..."
    echo "[agent-device] This may take up to 15 minutes depending on your connection."
    echo ""

    # Accept licenses
    yes | sdkmanager --licenses 2>/dev/null || true

    # Download system image
    echo "[agent-device] Downloading system-images;android-35;google_apis_playstore;x86_64..."
    sdkmanager "system-images;android-35;google_apis_playstore;x86_64"

    if [ $? -ne 0 ]; then
        echo ""
        echo "ERROR: Failed to download system image"
        exit 1
    fi

    # Create AVD
    echo ""
    echo "[agent-device] Creating Android Virtual Device..."
    echo "no" | avdmanager create avd -n device \
        -k "system-images;android-35;google_apis_playstore;x86_64" -d "pixel_6" 2>/dev/null || true

    # Configure AVD for container use
    mkdir -p ~/.android/avd/device.avd
    cat > ~/.android/avd/device.avd/config.ini << 'AVDEOF'
avd.ini.encoding=UTF-8
AvdId=device
PlayStore.enabled=true
abi.type=x86_64
disk.dataPartition.size=4G
hw.accelerator.isAccelerated=yes
hw.cpu.arch=x86_64
hw.cpu.ncore=4
hw.gpu.enabled=yes
hw.gpu.mode=swiftshader_indirect
hw.keyboard=yes
hw.lcd.density=420
hw.lcd.height=2400
hw.lcd.width=1080
hw.ramSize=4096
image.sysdir.1=system-images/android-35/google_apis_playstore/x86_64/
tag.display=Google Play
tag.id=google_apis_playstore
vm.heapSize=576
AVDEOF

    echo ""
    echo "[agent-device] Installation complete!"
    echo "[agent-device] You can now use agent-device commands."
    exit 0
}

# Handle install command
if [ "$1" = "install" ]; then
    install_system_image
fi

# Check if nested virtualization is enabled
check_kvm

# Check if Android system images exist
if ! ls /home/user/android-sdk/system-images/android-* 1>/dev/null 2>&1; then
    echo "=============================================="
    echo "ERROR: Android system images not found!"
    echo "=============================================="
    echo ""
    echo "Run the following command to download the system image:"
    echo ""
    echo "    agent-device install"
    echo ""
    echo "(This may take up to 15 minutes)"
    echo "=============================================="
    exit 1
fi

# Create AVD if it doesn't exist
if [ ! -d ~/.android/avd/device.avd ]; then
    echo "[agent-device] Creating Android Virtual Device..."

    # Find the installed system image
    SYSTEM_IMAGE=$(ls -d ${ANDROID_SDK_ROOT}/system-images/android-*/google_apis_playstore/x86_64 2>/dev/null | head -1)
    if [ -z "$SYSTEM_IMAGE" ]; then
        SYSTEM_IMAGE=$(ls -d ${ANDROID_SDK_ROOT}/system-images/android-*/google_apis/x86_64 2>/dev/null | head -1)
    fi
    if [ -z "$SYSTEM_IMAGE" ]; then
        SYSTEM_IMAGE=$(ls -d ${ANDROID_SDK_ROOT}/system-images/android-*/default/x86_64 2>/dev/null | head -1)
    fi

    if [ -z "$SYSTEM_IMAGE" ]; then
        echo "ERROR: Could not find a valid system image in ${ANDROID_SDK_ROOT}/system-images/"
        exit 1
    fi

    # Extract the image identifier
    ANDROID_VER=$(echo "$SYSTEM_IMAGE" | grep -oP 'android-\d+' | head -1)
    IMAGE_TYPE=$(basename $(dirname "$SYSTEM_IMAGE"))
    IMAGE_ID="system-images;${ANDROID_VER};${IMAGE_TYPE};x86_64"

    echo "[agent-device] Using system image: $IMAGE_ID"
    echo "no" | avdmanager create avd -n device -k "$IMAGE_ID" -d "pixel_6" 2>/dev/null || true

    # Configure AVD for container use
    mkdir -p ~/.android/avd/device.avd
    IMAGE_SYSDIR=$(echo "$SYSTEM_IMAGE" | sed "s|${ANDROID_SDK_ROOT}/||")

    cat > ~/.android/avd/device.avd/config.ini << AVDEOF
avd.ini.encoding=UTF-8
AvdId=device
PlayStore.enabled=true
abi.type=x86_64
disk.dataPartition.size=4G
hw.accelerator.isAccelerated=yes
hw.cpu.arch=x86_64
hw.cpu.ncore=4
hw.gpu.enabled=yes
hw.gpu.mode=swiftshader_indirect
hw.keyboard=yes
hw.lcd.density=420
hw.lcd.height=2400
hw.lcd.width=1080
hw.ramSize=4096
image.sysdir.1=${IMAGE_SYSDIR}/
tag.display=Google Play
tag.id=${IMAGE_TYPE}
vm.heapSize=576
AVDEOF

    echo "[agent-device] AVD created!"
fi

# Fix KVM permissions if needed
if [ -e /dev/kvm ] && [ ! -w /dev/kvm ]; then
    sudo chmod 666 /dev/kvm 2>/dev/null || true
fi

# Auto-start emulator if no Android devices are connected
if ! ${ANDROID_SDK_ROOT}/platform-tools/adb devices 2>/dev/null | grep -q "emulator\|device$"; then
    echo "[agent-device] Starting Android emulator..."
    nohup ${ANDROID_SDK_ROOT}/emulator/emulator -avd device -no-audio -gpu swiftshader_indirect -no-snapshot -no-boot-anim &>/tmp/emulator.log &
    ${ANDROID_SDK_ROOT}/platform-tools/adb wait-for-device
    echo "[agent-device] Waiting for boot..."
    while [ "$(${ANDROID_SDK_ROOT}/platform-tools/adb shell getprop sys.boot_completed 2>/dev/null)" != "1" ]; do
        sleep 2
    done
    echo "[agent-device] Emulator ready!"
fi

# Run the real agent-device binary
exec /usr/lib/node_modules/agent-device/bin/agent-device.mjs "$@"
