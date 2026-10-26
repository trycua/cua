//! The bulk lane's layout is stated in three places that cannot depend on
//! each other (the VM side in `cua-vmm`, the wire contract in `cua-proto`,
//! the Linux image's udev rule). This holds them together.

#[test]
fn the_vm_the_wire_contract_and_the_image_agree_on_the_lane() {
    assert_eq!(cua_vmm::bulk::SERIAL, cua_proto::bulk::SERIAL);
    assert_eq!(cua_vmm::bulk::DISK_BYTES, cua_proto::bulk::DISK_BYTES);
    let rule = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../images/linux/files/udev/71-cua-bulk-lane.rules"
    ))
    .expect("the image ships a udev rule for the bulk disk");
    assert!(
        rule.lines().any(|l| !l.trim_start().starts_with('#')
            && l.contains(&format!("ATTRS{{serial}}==\"{}\"", cua_proto::bulk::SERIAL))
            && l.contains("MODE=\"0600\"")),
        "the udev rule must give the spacesd user the disk with this serial"
    );
}
