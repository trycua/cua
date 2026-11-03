// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The EC2 provider against a fake EC2 and STS Query endpoint: the calls a
//! create makes (tags in the create call, IMDSv2 with a hop limit of 1,
//! terminate on shutdown, no key pair or instance profile, one outbound-only
//! security group reused), the dry-run test that creates nothing, the tag
//! guards, and a sweep that leaves everything the account already had
//! untouched.
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
        if ["RunInstances", "CreateSecurityGroup"].contains(&c["Action"].as_str()) {
            assert_eq!(c.get("DryRun").map(String::as_str), Some("true"), "{c:?}");
        }
    }
    assert!(s.lock().unwrap().instances.is_empty() && s.lock().unwrap().groups.is_empty());

    // A dry run AWS refuses fails `permissions`, saying which call.
    s.lock().unwrap().deny = true;
    let t = aws.test(&conn()).await;
    assert!(!t.ok());
    let p = t.checks.iter().find(|c| c.name == "permissions").unwrap();
    assert!(
        p.detail.contains("RunInstances: UnauthorizedOperation"),
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
async fn the_dry_run_security_group_carries_the_managed_tag() {
    // The least-privilege policy only allows a security group created with
    // the cua-managed tag, so the test's dry run must send it.
    let (url, s) = fake().await;
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    assert!(aws.test(&conn()).await.ok());
    let calls = s.lock().unwrap().calls.clone();
    let sg = calls
        .iter()
        .find(|c| c["Action"] == "CreateSecurityGroup")
        .expect("a dry run");
    assert_eq!(sg["DryRun"], "true");
    assert_eq!(spec_tags(sg, "security-group")["cua-managed"], "true");
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
    assert_eq!(made.len(), 2, "the instance and the new security group");
    assert_eq!(made[0].resource_type, "instance");
    assert_eq!(made[1].resource_type, "security_group");
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
    assert_eq!(run["ClientToken"], "cua-aws-0123456789ab");
    assert_eq!(run["BlockDeviceMapping.1.Ebs.VolumeType"], "gp3");
    assert_eq!(run["SecurityGroupId.1"], made[1].id);
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
    // The group: tagged in its create call, in the default VPC, no ingress
    // rule was ever added.
    let a = actions(&s);
    assert!(
        !a.iter()
            .any(|x| x.starts_with("AuthorizeSecurityGroup") || x.starts_with("Revoke"))
    );
    let (_, vpc, gtags) = s.lock().unwrap().groups[&made[1].id].clone();
    assert_eq!(vpc, "vpc-default");
    assert!(model::is_ours(&gtags, "owner1"));

    // A second create reuses the group.
    let again = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(
        actions(&s)
            .iter()
            .filter(|x| *x == "CreateSecurityGroup")
            .count(),
        1
    );
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
    assert_eq!(run["SecurityGroupId.1"], pool_group);
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
    assert_eq!(last["SecurityGroupId.1"], sandbox_group);
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

/// No capacity where EC2 puts an instance: the create tries each zone of the
/// default VPC in order, with its own client token, until one has room.
#[tokio::test]
async fn a_create_without_capacity_tries_the_other_zones() {
    let (url, s) = fake().await;
    s.lock().unwrap().full = vec!["".into(), "subnet-a".into()];
    let aws = Aws::with_endpoint(&url, "AKIDTEST", "secret");
    let made = aws.provision(&conn(), &spec("owner1")).await.unwrap();
    assert_eq!(made[0].resource_type, "instance");
    let runs: Vec<(String, String)> = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c["Action"] == "RunInstances")
        .map(|c| {
            (
                c.get("SubnetId").cloned().unwrap_or_default(),
                c["ClientToken"].clone(),
            )
        })
        .collect();
    assert_eq!(
        runs,
        vec![
            ("".into(), "cua-aws-0123456789ab".into()),
            ("subnet-a".into(), "cua-aws-0123456789ab-z0".into()),
            ("subnet-b".into(), "cua-aws-0123456789ab-z1".into()),
        ]
    );
    // Nowhere with room (or the type is not offered in a zone): the error
    // names every placement tried and why each failed, and nothing is left.
    {
        let mut a = s.lock().unwrap();
        a.full = vec!["".into(), "subnet-a".into()];
        a.unsupported = vec!["subnet-b".into()];
    }
    let before = s.lock().unwrap().instances.len();
    let e = aws
        .provision(&conn(), &spec("owner1"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("in us-west-2: tried 3 placements"), "{e}");
    assert!(
        e.contains("EC2's pick of zone: InsufficientInstanceCapacity"),
        "{e}"
    );
    assert!(
        e.contains("us-west-2a (subnet-a): InsufficientInstanceCapacity"),
        "{e}"
    );
    assert!(e.contains("us-west-2b (subnet-b): Unsupported"), "{e}");
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
    assert_eq!(
        actions(&s)
            .iter()
            .filter(|x| *x == "CreateSecurityGroup")
            .count(),
        1
    );
}

/// Another process (a second daemon, an older run) makes the group between
/// the look and the create: `InvalidGroup.Duplicate` means it is there.
#[tokio::test]
async fn a_duplicate_security_group_is_the_one_to_use() {
    let (url, s) = fake().await;
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
    assert_eq!(s.lock().unwrap().groups.len(), 1);
}
