//! `cua-microvm-init`: PID 1 of a Cua microVM Space.
//!
//! Booted from a tiny initramfs (this binary as `/init`), it:
//!
//! 1. mounts the image (`/dev/vda`, read-only squashfs) under a writable
//!    overlay whose upper layer is the VM's own scratch disk (`/dev/vdb`),
//!    and switches root into it;
//! 2. mounts `/proc`, `/sys`, `/dev`, `/dev/pts`, `/dev/shm` and `/run`,
//!    brings up `lo` (the kernel's `ip=` parameter configures `eth0`), and
//!    writes `/etc/hosts` and `/etc/resolv.conf`;
//! 3. listens on vsock ([`cua_microvm_init::PORT`]) for the host's
//!    requests (the workload, the identity, exec, port checks), and
//! 4. reaps every orphan and powers the VM off when the workload exits.
//!
//! The protocol is in the library half of this crate.
//!
//! It must be a static executable (the initramfs holds nothing else; a
//! dynamic one panics the guest kernel with "No working init found"):
//! `cargo build --release -p cua-microvm-init --target
//! x86_64-unknown-linux-musl`, or a glibc target with
//! `RUSTFLAGS="-C target-feature=+crt-static"`. The daemon refuses a
//! dynamic one and `cua runtime doctor` reports it.

#[cfg(target_os = "linux")]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("cua-microvm-init runs only as PID 1 of a Linux microVM");
        std::process::exit(2);
    }
}
