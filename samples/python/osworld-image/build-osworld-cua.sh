#!/usr/bin/env bash
# Patch the official OSWorld Ubuntu qcow2 with cua-driver + an MCP HTTP endpoint,
# then flatten it into disk.img ready to wrap as a KubeVirt containerDisk.
# Runs inside the libguestfs helper container; /work is the build directory and
# contains base.qcow2, the cua-driver release tarball, and this osworld-image/ folder.
# Nothing is downloaded inside the guest: Node and supergateway are fetched here
# and copied in, so the recipe works with libguestfs' network disabled.
set -euxo pipefail
cd /work
: "${DRIVER_TGZ:=cua-driver-rs-0.25.0-linux-x86_64-binary.tar.gz}"
: "${SUPERGATEWAY_VERSION:=3.4.3}"
: "${NODE_VERSION:=22.12.0}"

# 1. Inputs: cua-driver release binary, Node.js, supergateway (npm package).
rm -rf drv && mkdir drv && tar -xzf "$DRIVER_TGZ" -C drv cua-driver
if [ ! -x node/bin/node ]; then
  curl -fsSL -o node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz"
  rm -rf node && mkdir node && tar -xJf node.tar.xz -C node --strip-components=1 && rm node.tar.xz
fi
rm -rf supergateway && mkdir supergateway
PATH="/work/node/bin:$PATH" npm install --prefix /work/supergateway --omit=dev "supergateway@${SUPERGATEWAY_VERSION}"

# 2. Customize an overlay of the official disk (base.qcow2 is never modified).
rm -f work.qcow2 && qemu-img create -f qcow2 -b base.qcow2 -F qcow2 work.qcow2
virt-customize -a work.qcow2 --no-network --smp 4 --memsize 4096 \
  --mkdir /opt/cua \
  --copy-in drv/cua-driver:/usr/local/bin \
  --chmod 0755:/usr/local/bin/cua-driver \
  --copy-in node:/opt \
  --copy-in supergateway:/opt \
  --run-command 'ln -sf /opt/node/bin/node /usr/local/bin/node && ln -sf /opt/supergateway/node_modules/.bin/supergateway /usr/local/bin/supergateway' \
  --copy-in osworld-image/start-cua-mcp.sh:/opt/cua \
  --chmod 0755:/opt/cua/start-cua-mcp.sh \
  --copy-in osworld-image/cua-driver-mcp.service:/etc/systemd/system \
  --copy-in osworld-image/99-cua-dhcp.yaml:/etc/netplan \
  --chmod 0600:/etc/netplan/99-cua-dhcp.yaml \
  --run-command 'mkdir -p /etc/cloud/cloud.cfg.d && printf "network: {config: disabled}\n" > /etc/cloud/cloud.cfg.d/99-disable-network-config.cfg' \
  --run-command 'export PATH=/usr/local/bin:/opt/node/bin:$PATH; cua-driver --version && node --version && supergateway --help >/dev/null' \
  --run-command 'systemctl enable cua-driver-mcp.service'

# 3. Flatten the overlay onto its base and compress: this is the containerDisk payload.
qemu-img convert -O qcow2 -c -p -m 8 -W work.qcow2 disk.img
qemu-img info disk.img
