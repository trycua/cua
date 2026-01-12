# Cua Windows Container
#
# A Windows 11 VM with cua-computer-server for Computer-Use Agents.
#
# Build:
#   docker build -t trycua/cua-qemu-windows:latest .
#
# With Windows Arena benchmark apps (Chrome, LibreOffice, VLC, etc.):
#   docker build --build-arg INSTALL_WINARENA_APPS=true -t trycua/cua-qemu-windows:latest .
#
# Environment variables:
#   INSTALL_WINARENA_APPS - "true" to install benchmark apps (Chrome, LibreOffice, VLC, etc.)

FROM trycua/windows-local:latest

# Build argument for Windows Arena benchmark apps
ARG INSTALL_WINARENA_APPS=false

COPY src/vm/setup/. /oem/

COPY --chmod=755 src/entry.sh /entry.sh

# VM configuration
ENV RAM_SIZE="8G"
ENV CPU_CORES="8"
ENV VERSION="win11x64-enterprise-eval"
ENV DISK_SIZE="30G"

# Enable QEMU Machine Protocol for VM control
ENV ARGUMENTS="-qmp tcp:0.0.0.0:7200,server,nowait"

# Pass build arg as env var for setup.ps1 to read
ENV INSTALL_WINARENA_APPS=${INSTALL_WINARENA_APPS}

# Expose ports:
#   5000 - cua-computer-server API
#   8006 - noVNC web interface
EXPOSE 5000 8006

ENTRYPOINT ["/entry.sh"]