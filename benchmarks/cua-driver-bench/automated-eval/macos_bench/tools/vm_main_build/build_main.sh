#!/bin/zsh
# Kept for the record (Amendments 3 and 5): the scripts that build, assemble and grant the private main-build app inside the
# benchmark VM (SIP disabled). Run as the VM user lume, with sudo rights, in this order: build_main.sh, assemble_main.sh, tcc_grant.sh.
# build Cua Driver from the main snapshot (release config) inside the VM
source $HOME/.cargo/env
export CUA_DRIVER_SOURCE_SHA=$(cat ~/bench-work/cua-main/SOURCE_SHA)
export CUA_DRIVER_GIT_SHA=$CUA_DRIVER_SOURCE_SHA
export CUA_DRIVER_RS_TELEMETRY_ENABLED=false
cd ~/bench-work/cua-main/src/libs/cua-driver/rust
date -u
rustup show active-toolchain 2>&1 | tail -1
cargo build --locked --release -p cua-driver -p cursor-theme-cli > ~/bench-work/cua-main/build.log 2>&1
echo "build exit $?"
date -u

