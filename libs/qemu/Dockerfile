# Define build argument for deployment mode (default is dev, can also be azure)
ARG DEPLOY_MODE="dev"

# Conditional copy of files based on build argument
FROM windowsarena/windows-local:latest AS build_dev
ONBUILD COPY src/vm/setup/. /shared/
ONBUILD COPY src/vm/unattend-files/dev_win11x64-enterprise-eval.xml /run/assets/win11x64-enterprise-eval.xml
ONBUILD ENV FOLDER_NAME=shared

FROM windowsarena/windows-local:latest AS build_azure
ONBUILD COPY src/vm/setup/. /oem/
ONBUILD COPY src/vm/unattend-files/azure_win11x64-enterprise-eval.xml /run/assets/win11x64-enterprise-eval.xml
ONBUILD ENV FOLDER_NAME=oem

FROM build_${DEPLOY_MODE}

ARG DEPLOY_MODE="dev"
ENV DEPLOY_MODE=${DEPLOY_MODE}

RUN echo "FOLDER_NAME: ${FOLDER_NAME}"
RUN echo "DEPLOY_MODE: ${DEPLOY_MODE}"

# If azure, replace windows data folder with oem folder
RUN if [ "${DEPLOY_MODE}" = "azure" ]; then \
    WINDOWS_DATA_FOLDER='\\\\host.lan\\Data'; \
    WINDOWS_OEM_FOLDER='C:\\oem'; \
    OEM_FOLDER='oem'; \
    sed -i "s|${WINDOWS_DATA_FOLDER}|${WINDOWS_OEM_FOLDER}|g" "/${OEM_FOLDER}/on-logon.ps1"; \
    sed -i "s|${WINDOWS_DATA_FOLDER}|${WINDOWS_OEM_FOLDER}|g" "/${OEM_FOLDER}/setup.ps1"; \
    fi

# Install fuse and socat (for port forwarding)
RUN apt-get update && apt-get install -y fuse socat

# Copy entry script
COPY src/entry.sh /entry.sh
RUN chmod +x /entry.sh

ENV YRES="900"
ENV XRES="1440"
ENV RAM_SIZE="8G"
ENV CPU_CORES="8"
ENV VERSION="win11x64-enterprise-eval"
ENV DISK_SIZE="30G"

# Enable QEMU's JSON-based QEMU Machine Protocol (QMP)
ENV ARGUMENTS="-qmp tcp:0.0.0.0:7200,server,nowait"

# Expose ports
EXPOSE 5000 8006

ENTRYPOINT ["/entry.sh"]