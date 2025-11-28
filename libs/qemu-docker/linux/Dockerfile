FROM qemu-local:latest

COPY src/vm/setup/. /oem/

COPY --chmod=755 src/entry.sh /entry.sh

ENV RAM_SIZE="8G"
ENV CPU_CORES="8"
ENV DISK_SIZE="64G"
ENV ARGUMENTS="-qmp tcp:0.0.0.0:7200,server,nowait"

EXPOSE 5000 8006

ENTRYPOINT ["/entry.sh"]