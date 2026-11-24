//! Debug helper: boot a disk under the QEMU backend and leave it running.
//! `cargo run --example qemu_boot -- <name> <disk> [aarch64|x86_64]`; stop with
//! `cargo run --example qemu_boot -- --delete <name>`.

use std::path::PathBuf;
use std::time::Duration;

use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Runtime, SshAccess, StartSpec};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cache = PathBuf::from(std::env::var("HOME").unwrap()).join(".cua/e2e-cache");
    let rt = QemuRuntime::new(QemuConfig {
        root: cache.join("vmm-qemu"),
        ..Default::default()
    });
    if args[0] == "--delete" {
        rt.delete(&args[1]).await.unwrap();
        return;
    }
    let key = cua_vmm::cloudinit::ensure_ssh_key(&cache.join("id_ed25519"))
        .await
        .unwrap();
    let arch = args
        .get(2)
        .map(|a| a.parse().unwrap())
        .unwrap_or(cua_vmm::Arch::host());
    let spec = StartSpec::new(&args[0], ImageSource::disk(&args[1]))
        .arch(arch)
        .ssh(SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        })
        .ready_timeout(Duration::from_secs(60));
    let inst = rt.start(&spec).await.unwrap();
    println!("{}", serde_json::to_string_pretty(&inst).unwrap());
}
