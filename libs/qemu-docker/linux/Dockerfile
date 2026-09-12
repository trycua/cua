FROM trycua/qemu-local:latest

RUN apt-get update && apt-get install -y --no-install-recommends python3-websockify dos2unix aria2 sshpass openssh-client && rm -rf /var/lib/apt/lists/*

COPY src/vm/setup/. /oem/
RUN find /oem -type f -name "*.sh" -exec dos2unix {} \;

COPY --chmod=755 src/entry.sh /entry.sh

ENV RAM_SIZE="8G"
ENV CPU_CORES="8"
ENV DISK_SIZE="64G"
ENV DISK_IO="threads"
ENV DISK_CACHE="writeback"
ENV ARGUMENTS="-qmp tcp:0.0.0.0:7200,server,nowait"

EXPOSE 22 5000 5900 8006

ENTRYPOINT ["/entry.sh"]