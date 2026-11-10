// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The EC2 provider against a fake EC2 and STS Query endpoint: the calls a
//! create makes (tags in the create call, IMDSv2 with a hop limit of 1,
//! terminate on shutdown, no key pair or instance profile, one outbound-only
//! security group reused, a VPC of Cua's own and never the default one), the
//! dry-run test that creates nothing, the tag guards, and a sweep that leaves
//! everything the account already had untouched.
#![cfg(feature = "aws")]

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::aws::Aws;
use cua_byoc::model::{self, ProvisionSpec, Resource};
use cua_byoc::{CloudApi, Connection};

#[path = "support/fake_aws.rs"]
mod fake_aws;
use fake_aws::{S, fake, filters, spec_tags};

fn conn() -> Connection {
    Connection {
        provider: "aws".into(),
        profile: "test".into(),
        region: "us-west-2".into(),
        ..Default::default()
    }
}

fn actions(s: &S) -> Vec<String> {
    s.lock()
        .unwrap()
        .calls
        .iter()
        .map(|c| c["Action"].clone())
        .collect()
}

#[tokio::test]
async fn test_is_a_dry_run_that_creates_nothing() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let t = aws.test(&conn()).await;
    assert!(t.ok(), "{:?}", t.checks);
    assert_eq!(t.account, "111122223333");
    let names: Vec<_> = t.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["credentials", "region", "image", "network", "permissions"]
    );
    // Only dry runs of what creates; nothing exists afterwards.
    for c in &s.lock().unwrap().calls {
        if ["RunInstances", "CreateVpc"].contains(&c["Action"].as_str()) {
            assert_eq!(c.get("DryRun").map(String::as_str), Some("true"), "{c:?}");
        }
    }
    assert!(s.lock().unwrap().instances.is_empty() && s.lock().unwrap().groups.is_empty());
    assert_eq!(
        s.lock().unwrap().vpcs.len(),
        1,
        "only the default VPC the account had"
    );

    // A dry run AWS refuses fails `permissions`, saying which call.
    s.lock().unwrap().deny = true;
    let t = aws.test(&conn()).await;
    assert!(!t.ok());
    let p = t.checks.iter().find(|c| c.name == "permissions").unwrap();
    assert!(
        p.detail.contains("CreateVpc: UnauthorizedOperation"),
        "{}",
        p.detail
    );
    // ... and says which policy to attach.
    assert!(
        p.detail.contains(cua_byoc::policy::DOCS_URL),
        "{}",
        p.detail
    );
}

#[tokio::test]
async fn the_dry_run_vpc_carries_the_managed_tag() {
    // The least-privilege policy only allows a VPC created with the
    // cua-managed tag, so the test's dry run must send it.
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    assert!(aws.test(&conn()).await.ok());
    let calls = s.lock().unwrap().calls.clone();
    let vpc = calls
        .iter()
        .find(|c| c["Action"] == "CreateVpc")
        .expect("a dry run");
    assert_eq!(vpc["DryRun"], "true");
    assert_eq!(spec_tags(vpc, "vpc")["cua-managed"], "true");
    // No subnet of Cua's yet: RunInstances is not dry-run into one.
    assert!(calls.iter().all(|c| c["Action"] != "RunInstances"));
}

#[tokio::test]
async fn once_cua_has_a_subnet_the_dry_run_checks_run_instances_into_it() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    aws.provision(&conn(), &spec("owner1")).await.unwrap();
    s.lock().unwrap().calls.clear();
    assert!(aws.test(&conn()).await.ok());
    let run = calls_to(&s, "RunInstances");
    assert_eq!(run.len(), 1);
    assert_eq!(run[0]["DryRun"], "true");
    let subnet = s.lock().unwrap().subnets.keys().next().unwrap().clone();
    assert_eq!(run[0]["NetworkInterface.1.SubnetId"], subnet);
    // Refused, it names the call.
    s.lock().unwrap().deny = true;
    let t = aws.test(&conn()).await;
    let p = t.checks.iter().find(|c| c.name == "permissions").unwrap();
    assert!(
        p.detail.contains("RunInstances: UnauthorizedOperation"),
        "{}",
        p.detail
    );
}

#[tokio::test]
async fn the_boot_log_is_the_decoded_console_output() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let r = Resource {
        provider: "aws".into(),
        id: "i-1".into(),
        resource_type: "instance".into(),
        ..Default::default()
    };
    // Nothing on the console yet.
    assert_eq!(aws.boot_log(&conn(), &r).await.unwrap(), None);
    s.lock().unwrap().console = Some("[ 1.0] cloud-init: boom\nsecond line\n".into());
    let log = aws.boot_log(&conn(), &r).await.unwrap().unwrap();
    assert_eq!(log, "[ 1.0] cloud-init: boom\nsecond line\n");
    let calls = s.lock().unwrap().calls.clone();
    let got = calls
        .iter()
        .find(|c| c["Action"] == "GetConsoleOutput")
        .unwrap();
    assert_eq!(got["InstanceId"], "i-1");
    assert_eq!(got["Latest"], "true");
}

fn spec(owner: &str) -> ProvisionSpec {
    ProvisionSpec {
        name: "cua-aws-0123456789ab".into(),
        family: "linux".into(),
        machine_type: "t4g.medium".into(),
        disk_gb: 30,
        user_data: "#cloud-config\n".into(),
        tags: model::tags(owner, "research", "cloud-0123", 1_900_000_000),
        arch: "arm64".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_create_is_tagged_locked_down_and_reuses_one_outbound_only_group() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    let types: Vec<_> = made.iter().map(|r| r.resource_type.as_str()).collect();
    assert_eq!(
        types,
        [
            "instance",
            "security_group",
            "vpc",
            "internet_gateway",
            "route_table",
            "subnet"
        ],
        "the instance, its group and the VPC parts made now, all recorded"
    );
    for r in &made[1..] {
        assert!(model::is_ours(&r.tags, "owner1"), "{r:?}");
        assert!(!r.id.is_empty());
    }
    let run = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .find(|c| c["Action"] == "RunInstances")
        .cloned()
        .unwrap();
    assert_eq!(run["ImageId"], "ami-new", "the newest Ubuntu image");
    assert_eq!(run["InstanceType"], "t4g.medium");
    assert_eq!(run["InstanceInitiatedShutdownBehavior"], "terminate");
    assert_eq!(run["MetadataOptions.HttpTokens"], "required");
    assert_eq!(run["MetadataOptions.HttpPutResponseHopLimit"], "1");
    assert_eq!(run["MetadataOptions.InstanceMetadataTags"], "disabled");
    assert_eq!(run["ClientToken"], "cua-aws-0123456789ab");
    assert_eq!(run["BlockDeviceMapping.1.Ebs.VolumeType"], "gp3");
    assert_eq!(run["NetworkInterface.1.SecurityGroupId.1"], made[1].id);
    assert_eq!(run["NetworkInterface.1.SubnetId"], made[5].id);
    assert_eq!(run["NetworkInterface.1.AssociatePublicIpAddress"], "true");
    assert!(
        !run.contains_key("KeyName") && !run.keys().any(|k| k.starts_with("IamInstanceProfile"))
    );
    // Tags ride in the create call, for the instance and its volume.
    let inst = spec_tags(&run, "instance");
    let vol = spec_tags(&run, "volume");
    assert_eq!(inst["cua-managed"], "true");
    assert_eq!(inst["cua-owner"], "owner1");
    assert_eq!(inst["cua-expires"], "1900000000");
    assert_eq!(inst, vol);
    // The group: tagged in its create call, in Cua's VPC (never the default
    // one), no ingress rule was ever added.
    let a = actions(&s);
    assert!(
        !a.iter()
            .any(|x| x.starts_with("AuthorizeSecurityGroup") || x.starts_with("Revoke"))
    );
    let (_, vpc, gtags) = s.lock().unwrap().groups[&made[1].id].clone();
    assert_eq!(vpc, made[2].id);
    assert_ne!(vpc, "vpc-default");
    assert!(model::is_ours(&gtags, "owner1"));

    // The network: a /16 of ours, a gateway attached to it, one route table
    // with the way out, and a /20 subnet routed through it. No peering, no
    // endpoint, nothing else.
    {
        let a = s.lock().unwrap();
        let (cidr, default, vtags) = a.vpcs[&made[2].id].clone();
        assert_eq!((cidr.as_str(), default), ("10.77.0.0/16", false));
        assert!(model::is_ours(&vtags, "owner1"));
        assert_eq!(vtags["Name"], "cua-sb-owner1");
        let (attached, gtags) = a.igws[&made[3].id].clone();
        assert_eq!(attached.as_deref(), Some(made[2].id.as_str()));
        assert!(model::is_ours(&gtags, "owner1"));
        let rt = &a.rts[&made[4].id];
        assert_eq!(rt.vpc, made[2].id);
        assert_eq!(rt.routes, [("0.0.0.0/0".to_string(), made[3].id.clone())]);
        assert_eq!(rt.assocs.len(), 1);
        assert_eq!(rt.assocs[0].1, made[5].id);
        let (svpc, zone, scidr, stags) = a.subnets[&made[5].id].clone();
        assert_eq!(
            (svpc.as_str(), zone.as_str()),
            (made[2].id.as_str(), "us-west-2a")
        );
        assert_eq!(scidr, "10.77.0.0/20");
        assert!(model::is_ours(&stags, "owner1"));
        // The account's default VPC was never touched.
        assert!(a.subnets.values().all(|s| s.0 != "vpc-default"));
        assert!(a.groups.values().all(|g| g.1 != "vpc-default"));
        assert_eq!(a.vpcs.len(), 2);
    }
    for banned in [
        "CreateVpcPeeringConnection",
        "CreateVpcEndpoint",
        "CreateNatGateway",
        "ModifyVpcAttribute",
        "CreateDefaultVpc",
    ] {
        assert!(!a.iter().any(|x| x == banned), "{banned}");
    }

    // A second create reuses all of it.
    let again = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert_eq!(again.len(), 1);
    for once in [
        "CreateSecurityGroup",
        "CreateVpc",
        "CreateInternetGateway",
        "CreateRouteTable",
        "CreateSubnet",
    ] {
        assert_eq!(
            actions(&s).iter().filter(|x| *x == once).count(),
            1,
            "{once}"
        );
    }
    // Another owner gets a VPC of their own.
    let other = aws.provision(&conn(), &spec("owner2")).await.unwrap();
    assert!(other.iter().any(|r| r.resource_type == "vpc"));
    assert_eq!(s.lock().unwrap().vpcs.len(), 3);
}

/// An interrupted create (a VPC with no gateway, no route table) is
/// finished by the next one, and what was made is not made again.
#[tokio::test]
async fn an_interrupted_vpc_create_is_finished_not_duplicated() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let tags = {
        let mut t = model::tags("owner1", "", "", 0);
        t.insert("cua-group".into(), "sandbox-vpc".into());
        t
    };
    s.lock()
        .unwrap()
        .vpcs
        .insert("vpc-half".into(), ("10.77.0.0/16".into(), false, tags));
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    let types: Vec<_> = made.iter().map(|r| r.resource_type.as_str()).collect();
    assert!(!types.contains(&"vpc"), "{types:?}");
    assert!(types.contains(&"internet_gateway") && types.contains(&"route_table"));
    assert_eq!(calls_to(&s, "CreateVpc").len(), 0);
    let a = s.lock().unwrap();
    assert_eq!(a.vpcs.len(), 2);
    assert!(a.subnets.values().all(|s| s.0 == "vpc-half"));
}

/// Our VPC parts go in the order AWS needs (the group, subnets, route
/// table, gateway, then the VPC), the guard refuses what is not ours, and
/// the default VPC is never deleted.
#[tokio::test]
async fn the_vpc_goes_away_part_by_part_and_only_ours() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    // Instance terminated first, as a sweep does.
    aws.delete(&conn(), &made[0], "owner1").await.unwrap();
    // The listing carries every part.
    let listed = aws.list_owned(&conn(), "owner1").await.unwrap();
    for t in cua_byoc::awsnet::DELETE_ORDER {
        assert!(listed.iter().any(|r| r.resource_type == *t), "{t}");
    }
    // Deleted in the order EC2 needs: the group, subnets, route table,
    // gateway, VPC.
    let mut parts: Vec<&Resource> = made[1..].iter().collect();
    parts.sort_by_key(|r| {
        cua_byoc::awsnet::DELETE_ORDER
            .iter()
            .position(|t| *t == r.resource_type)
    });
    for r in parts {
        aws.delete(&conn(), r, "owner1")
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", r.resource_type));
    }
    let a = s.lock().unwrap();
    assert!(a.subnets.is_empty() && a.igws.is_empty() && a.rts.is_empty() && a.groups.is_empty());
    assert_eq!(a.vpcs.keys().collect::<Vec<_>>(), ["vpc-default"]);
}

/// Parts that are not ours (the default VPC, a VPC without our tags, another
/// home's) are never listed, described for write or deleted.
#[tokio::test]
async fn a_vpc_part_that_is_not_ours_is_never_touched() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    {
        let mut a = s.lock().unwrap();
        a.vpcs.insert(
            "vpc-theirs".into(),
            ("10.0.0.0/16".into(), false, BTreeMap::new()),
        );
        a.vpcs.insert(
            "vpc-other-home".into(),
            ("10.77.0.0/16".into(), false, {
                let mut t = model::tags("someone-else", "", "", 0);
                t.insert("cua-group".into(), "sandbox-vpc".into());
                t
            }),
        );
        a.subnets.insert(
            "subnet-theirs".into(),
            (
                "vpc-theirs".into(),
                "us-west-2a".into(),
                "10.0.0.0/20".into(),
                BTreeMap::new(),
            ),
        );
        a.igws.insert(
            "igw-theirs".into(),
            (Some("vpc-theirs".into()), BTreeMap::new()),
        );
        a.rts.insert("rtb-theirs".into(), Default::default());
    }
    assert!(aws.list_owned(&conn(), "me").await.unwrap().is_empty());
    for (ty, id) in [
        ("vpc", "vpc-default"),
        ("vpc", "vpc-theirs"),
        ("vpc", "vpc-other-home"),
        ("subnet", "subnet-theirs"),
        ("internet_gateway", "igw-theirs"),
        ("route_table", "rtb-theirs"),
    ] {
        let r = Resource {
            provider: "aws".into(),
            id: id.into(),
            resource_type: ty.into(),
            ..Default::default()
        };
        let e = aws.delete(&conn(), &r, "me").await.unwrap_err().to_string();
        assert!(e.contains("refusing"), "{id}: {e}");
    }
    let a = actions(&s);
    for verb in [
        "DeleteVpc",
        "DeleteSubnet",
        "DeleteInternetGateway",
        "DeleteRouteTable",
        "DetachInternetGateway",
        "DisassociateRouteTable",
    ] {
        assert!(!a.iter().any(|x| x == verb), "{verb} was called");
    }
}

/// Our machines placed before Cua had a VPC of its own are in the default
/// VPC: the listing marks them so the sweep says so.
#[tokio::test]
async fn our_older_machines_in_the_default_vpc_are_marked() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    put(&s, "i-old", model::tags("me", "s", "c", 0));
    let new = aws.provision(&conn(), &spec("me")).await.unwrap();
    let listed = aws.list_owned(&conn(), "me").await.unwrap();
    let old = listed.iter().find(|r| r.id == "i-old").unwrap();
    assert_eq!(
        old.extra.get(model::DEFAULT_VPC).map(String::as_str),
        Some("true")
    );
    let fresh = listed.iter().find(|r| r.id == new[0].id).unwrap();
    assert!(!fresh.extra.contains_key(model::DEFAULT_VPC));
    assert_eq!(fresh.extra["vpc"], new[2].id);
}

#[tokio::test]
async fn a_pool_host_boots_the_image_its_ssm_parameter_names() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let param = cua_byoc::aws::host_image_parameter(true, "amd64");
    let host = ProvisionSpec {
        name: "cua-host-aws-1-0123ab".into(),
        machine_type: "g6.xlarge".into(),
        disk_gb: 100,
        user_data: "#!/bin/bash\n".into(),
        tags: model::tags("owner1", "host:aws-1", "", 1_900_000_000),
        arch: "amd64".into(),
        image_parameter: param.clone(),
        ..Default::default()
    };
    let made = aws.provision(&conn(), &host).await.unwrap();
    assert_eq!(made[0].resource_type, "instance");
    let a = actions(&s);
    assert!(
        !a.iter().any(|x| x == "DescribeImages"),
        "EC2 resolves the parameter: {a:?}"
    );
    let run = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .find(|c| c["Action"] == "RunInstances")
        .cloned()
        .unwrap();
    assert_eq!(run["ImageId"], format!("resolve:ssm:{param}"));
    assert_eq!(run["InstanceType"], "g6.xlarge");
    assert_eq!(run["BlockDeviceMapping.1.DeviceName"], "/dev/sda1");
    assert_eq!(run["BlockDeviceMapping.1.Ebs.VolumeSize"], "100");
    assert_eq!(run["InstanceInitiatedShutdownBehavior"], "terminate");
    assert_eq!(spec_tags(&run, "instance")["cua-space"], "host-aws-1");
    // Without peer ports a host uses the outbound-only group.
    assert!(!a.iter().any(|x| x == "AuthorizeSecurityGroupIngress"));
    // Anything but a public parameter is refused before a call.
    let bad = ProvisionSpec {
        image_parameter: "/my/own/param".into(),
        name: "cua-host-aws-2-0123ab".into(),
        ..host
    };
    assert!(aws.provision(&conn(), &bad).await.is_err());
}

/// Pool hosts get their own group: its ingress rules are TCP on the peer
/// ports and every UDP port, from members of that same group only (no
/// address range at all); the sandboxes' outbound-only group is never
/// reused for it or changed. A group that exists gets the rules it lacks
/// (an older one opened fewer ports), and rules it has are left alone.
#[tokio::test]
async fn pool_hosts_reach_each_other_and_nothing_else() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let sandbox = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    let sandbox_group = sandbox[1].id.clone();
    let host = |n: &str| ProvisionSpec {
        name: format!("cua-host-{n}"),
        machine_type: "g6.xlarge".into(),
        disk_gb: 100,
        user_data: "#!/bin/bash\n".into(),
        tags: model::tags("owner1", n, "", 1_900_000_000),
        arch: "amd64".into(),
        image_parameter: cua_byoc::aws::host_image_parameter(true, "amd64"),
        peer_ports: Some((0, 65535)),
        ..Default::default()
    };
    let made = aws.provision(&conn(), &host("host:aws-1")).await.unwrap();
    assert_eq!(made.len(), 2, "the instance and the pool hosts' group");
    let pool_group = made[1].id.clone();
    assert_ne!(pool_group, sandbox_group);
    let calls = s.lock().unwrap().calls.clone();
    let rules: Vec<_> = calls
        .iter()
        .filter(|c| c["Action"] == "AuthorizeSecurityGroupIngress")
        .collect();
    assert_eq!(rules.len(), 2);
    for (r, proto) in rules.iter().zip(["tcp", "udp"]) {
        assert_eq!(r["GroupId"], pool_group);
        assert_eq!(r["IpPermissions.1.IpProtocol"], proto);
        assert_eq!(r["IpPermissions.1.FromPort"], "0");
        assert_eq!(r["IpPermissions.1.ToPort"], "65535");
        assert_eq!(r["IpPermissions.1.Groups.1.GroupId"], pool_group);
        assert!(
            !r.keys()
                .any(|k| k.contains("IpRanges") || k.contains("Ipv6Ranges")),
            "{r:?}"
        );
    }
    let run = calls
        .iter()
        .rfind(|c| c["Action"] == "RunInstances")
        .unwrap();
    assert_eq!(run["NetworkInterface.1.SecurityGroupId.1"], pool_group);
    let (_, _, gtags) = s.lock().unwrap().groups[&pool_group].clone();
    assert_eq!(gtags["cua-group"], "pool-hosts");
    // A second host reuses it (its rules are there: asking again is
    // fine); a sandbox still gets the outbound-only one.
    let again = aws.provision(&conn(), &host("host:aws-2")).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(s.lock().unwrap().rules.len(), 2);
    let sb = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert_eq!(sb.len(), 1);
    let last = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .rfind(|c| c["Action"] == "RunInstances")
        .cloned()
        .unwrap();
    assert_eq!(last["NetworkInterface.1.SecurityGroupId.1"], sandbox_group);
    assert!(
        calls_to(&s, "AuthorizeSecurityGroupIngress")
            .iter()
            .all(|c| c["GroupId"] == pool_group),
        "the sandboxes' group was opened"
    );
}

fn calls_to(s: &S, action: &str) -> Vec<BTreeMap<String, String>> {
    s.lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c["Action"] == action)
        .cloned()
        .collect()
}

/// No capacity in a zone: the create tries each zone of Cua's VPC in order
/// (a subnet made for each as it is tried), with its own client token, until
/// one has room.
#[tokio::test]
async fn a_create_without_capacity_tries_the_other_zones() {
    let (url, s) = fake().await;
    s.lock().unwrap().full = vec!["us-west-2a".into()];
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert_eq!(made[0].resource_type, "instance");
    let subnet_in = |zone: &str| -> String {
        s.lock()
            .unwrap()
            .subnets
            .iter()
            .find(|(_, v)| v.1 == zone)
            .map(|(k, _)| k.clone())
            .unwrap()
    };
    let runs: Vec<(String, String)> = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c["Action"] == "RunInstances")
        .map(|c| {
            (
                c["NetworkInterface.1.SubnetId"].clone(),
                c["ClientToken"].clone(),
            )
        })
        .collect();
    assert_eq!(
        runs,
        vec![
            (subnet_in("us-west-2a"), "cua-aws-0123456789ab".into()),
            (subnet_in("us-west-2b"), "cua-aws-0123456789ab-z1".into()),
        ]
    );
    // Both zones have their own range of the /16.
    let cidrs: Vec<String> = s
        .lock()
        .unwrap()
        .subnets
        .values()
        .map(|v| v.2.clone())
        .collect();
    assert_eq!(cidrs.len(), 2);
    assert!(
        cidrs.contains(&"10.77.0.0/20".to_string()) && cidrs.contains(&"10.77.16.0/20".to_string())
    );
    // Nowhere with room (or the type is not offered in a zone): the error
    // names every placement tried and why each failed, and nothing is left.
    {
        let mut a = s.lock().unwrap();
        a.full = vec!["us-west-2a".into()];
        a.unsupported = vec!["us-west-2b".into()];
    }
    let before = s.lock().unwrap().instances.len();
    let e = aws
        .provision(&conn(), &spec("owner1"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("in us-west-2: tried 2 placements"), "{e}");
    assert!(
        e.contains(&format!(
            "us-west-2a ({}): InsufficientInstanceCapacity",
            subnet_in("us-west-2a")
        )),
        "{e}"
    );
    assert!(
        e.contains(&format!(
            "us-west-2b ({}): Unsupported",
            subnet_in("us-west-2b")
        )),
        "{e}"
    );
    assert_eq!(s.lock().unwrap().instances.len(), before);
}

fn put(s: &S, id: &str, tags: BTreeMap<String, String>) {
    s.lock()
        .unwrap()
        .instances
        .insert(id.into(), ("running".into(), tags));
}

#[tokio::test]
async fn nothing_untagged_or_foreign_is_listed_stopped_or_deleted() {
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    put(&s, "i-untagged", BTreeMap::new());
    put(&s, "i-foreign", model::tags("someone-else", "x", "c", 0));
    put(
        &s,
        "i-lookalike",
        BTreeMap::from([("Name".to_string(), "cua-aws-1".to_string())]),
    );
    s.lock().unwrap().groups.insert(
        "sg-default".into(),
        ("default".into(), "vpc-default".into(), BTreeMap::new()),
    );
    put(&s, "i-mine", model::tags("me", "s", "c", 0));

    let listed = aws.list_owned(&conn(), "me").await.unwrap();
    assert_eq!(
        listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["i-mine"]
    );
    // The owner filter goes to AWS, not just the client.
    let desc = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .find(|c| c["Action"] == "DescribeInstances")
        .cloned()
        .unwrap();
    assert!(
        filters(&desc)
            .iter()
            .any(|(n, v)| n == "tag:cua-owner" && v == &["me".to_string()])
    );

    for id in ["i-untagged", "i-foreign", "i-lookalike"] {
        let r = Resource {
            provider: "aws".into(),
            id: id.into(),
            resource_type: "instance".into(),
            ..Default::default()
        };
        for err in [
            aws.delete(&conn(), &r, "me").await.unwrap_err(),
            aws.stop(&conn(), &r, "me").await.unwrap_err(),
            aws.start(&conn(), &r, "me").await.unwrap_err(),
        ] {
            assert!(err.to_string().contains("refusing"), "{id}: {err}");
        }
    }
    let sg = Resource {
        provider: "aws".into(),
        id: "sg-default".into(),
        resource_type: "security_group".into(),
        ..Default::default()
    };
    assert!(aws.delete(&conn(), &sg, "me").await.is_err());
    let a = actions(&s);
    for verb in [
        "TerminateInstances",
        "StopInstances",
        "StartInstances",
        "DeleteSecurityGroup",
    ] {
        assert!(!a.iter().any(|x| x == verb), "{verb} was called");
    }
    // Ours: stop, start and delete go through.
    let mine = Resource {
        provider: "aws".into(),
        id: "i-mine".into(),
        resource_type: "instance".into(),
        ..Default::default()
    };
    aws.stop(&conn(), &mine, "me").await.unwrap();
    aws.start(&conn(), &mine, "me").await.unwrap();
    aws.delete(&conn(), &mine, "me").await.unwrap();
    assert_eq!(s.lock().unwrap().instances["i-mine"].0, "terminated");
    // Gone already: fine.
    let gone = Resource {
        id: "i-gone".into(),
        ..mine
    };
    aws.delete(&conn(), &gone, "me").await.unwrap();
    assert_eq!(s.lock().unwrap().instances["i-untagged"].0, "running");
}

/// The elastic pool's provider conformance suite against the fake EC2.
#[tokio::test]
async fn aws_conforms_to_the_elastic_contract() {
    use base64::Engine as _;
    use cua_byoc::conformance::{CreateCall, lifecycle_checks, static_checks};
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let c = conn();
    let bad = static_checks(&aws, &c).await;
    assert!(bad.is_empty(), "{bad:#?}");
    let s2 = s.clone();
    let seen = move || {
        s2.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|p| p["Action"] == "RunInstances")
            .map(|p| CreateCall {
                machine_type: p.get("InstanceType").cloned().unwrap_or_default(),
                // The fake answers for one region (the endpoint's).
                region: "us-west-2".into(),
                user_data: p
                    .get("UserData")
                    .and_then(|u| base64::engine::general_purpose::STANDARD.decode(u).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default(),
                tags: spec_tags(p, "instance"),
                ssh_key: p.contains_key("KeyName"),
            })
            .collect()
    };
    let bad = lifecycle_checks(&aws, &c, "owner1", &seen).await;
    assert!(bad.is_empty(), "{bad:#?}");
}

/// A cold burst of first launches on one account: every caller looks for
/// the shared group before any made it. One makes it; the rest use it.
#[tokio::test]
async fn sixteen_concurrent_first_launches_share_one_security_group() {
    let (url, s) = fake().await;
    s.lock().unwrap().describe_delay_ms = 40;
    let aws = Arc::new(Aws::with_endpoint(&url, "AKIDTEST", "secret"));
    let mut tasks = vec![];
    for i in 0..16 {
        let aws = aws.clone();
        tasks.push(tokio::spawn(async move {
            let mut sp = spec("owner1");
            sp.name = format!("cua-aws-{i:012}");
            aws.provision(&conn(), &sp).await
        }));
    }
    for t in tasks {
        t.await.unwrap().expect("a launch of the burst");
    }
    for once in ["CreateSecurityGroup", "CreateVpc", "CreateSubnet"] {
        assert_eq!(
            actions(&s).iter().filter(|x| *x == once).count(),
            1,
            "{once}"
        );
    }
}

/// Another process (a second daemon, an older run) makes the group between
/// the look and the create: `InvalidGroup.Duplicate` means it is there.
#[tokio::test]
async fn a_duplicate_security_group_is_the_one_to_use() {
    let (url, s) = fake().await;
    // The VPC and its subnet exist (a pool host made them with its own
    // group); only the outbound-only group is raced for.
    let host = ProvisionSpec {
        name: "cua-host-0".into(),
        machine_type: "t4g.medium".into(),
        user_data: "#!/bin/bash\n".into(),
        tags: model::tags("owner1", "host:aws-0", "", 1_900_000_000),
        arch: "arm64".into(),
        peer_ports: Some((0, 65535)),
        ..Default::default()
    };
    Aws::with_endpoint(&url, "AKIDTEST", "secret")
        .provision(&conn(), &host)
        .await
        .unwrap();
    s.lock().unwrap().describe_delay_ms = 40;
    let mut tasks = vec![];
    for i in 0..8 {
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            // Each with its own provider: no shared lock.
            let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
            let mut sp = spec("owner1");
            sp.name = format!("cua-aws-{i:012}");
            aws.provision(&conn(), &sp).await
        }));
    }
    for t in tasks {
        t.await.unwrap().expect("a launch of the burst");
    }
    // The pool group and one outbound-only group, in the one VPC.
    let a = s.lock().unwrap();
    let ours: Vec<_> = a
        .vpcs
        .iter()
        .filter(|(_, v)| !v.1)
        .map(|(k, _)| k)
        .collect();
    assert_eq!(ours.len(), 1);
    assert_eq!(a.groups.len(), 2);
}

/// Two processes make a VPC at the same moment: both keep the lowest id; the
/// other one (empty, ours) is deleted, and its parts are never recorded.
#[tokio::test]
async fn a_vpc_made_twice_at_once_keeps_the_lowest_and_deletes_the_other() {
    let (url, s) = fake().await;
    s.lock().unwrap().race_vpc = true;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert!(
        made.iter().all(|r| r.resource_type != "vpc"),
        "not ours to record"
    );
    let a = s.lock().unwrap();
    let mut ids: Vec<&String> = a.vpcs.keys().collect();
    ids.sort();
    assert_eq!(ids, ["vpc-0", "vpc-default"]);
    assert!(a.subnets.values().all(|v| v.0 == "vpc-0"));
    drop(a);
    assert_eq!(calls_to(&s, "DeleteVpc").len(), 1);
}
