# KubeVirt containerDisk: FROM scratch + the qcow2 at /disk/disk.img, owned by
# the qemu user (107) per KubeVirt's containerDisk contract. Nothing else.
# Build context: a directory holding disk.img for the target platform.
FROM scratch
COPY --chown=107:107 disk.img /disk/disk.img
