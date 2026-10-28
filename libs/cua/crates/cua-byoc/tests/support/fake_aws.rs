// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A fake EC2 and STS Query endpoint, shared by the EC2 provider tests and
//! the upgrade tests (which also point released `cua` binaries at it with
//! `AWS_ENDPOINT_URL`).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

#[derive(Default)]
pub struct Acct {
    /// id -> (state, tags)
    pub instances: BTreeMap<String, (String, BTreeMap<String, String>)>,
    /// id -> (name, vpc, tags)
    pub groups: BTreeMap<String, (String, String, BTreeMap<String, String>)>,
    /// Every call: its Action and parameters.
    pub calls: Vec<BTreeMap<String, String>>,
    pub next: u32,
    /// Refuse dry runs as unauthorized.
    pub deny: bool,
    /// Where RunInstances has no capacity: "" (EC2's own pick) and subnet ids.
    pub full: Vec<String>,
    /// Where the type is not offered (`Unsupported`): subnet ids.
    pub unsupported: Vec<String>,
    /// What the console shows (base64 on the wire), per instance.
    pub console: Option<String>,
    /// Ingress rules authorized: `group protocol from-to source-group`.
    pub rules: std::collections::BTreeSet<String>,
    /// Make each security group lookup slow (ms), so concurrent callers
    /// all see no group before any create lands.
    pub describe_delay_ms: u64,
}

pub type S = Arc<Mutex<Acct>>;

fn xml(body: String) -> axum::response::Response {
    (StatusCode::OK, [("content-type", "text/xml")], body).into_response()
}

fn ec2(action: &str, inner: &str) -> axum::response::Response {
    xml(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r-1</requestId>{inner}</{action}Response>"
    ))
}

fn error(status: StatusCode, code: &str) -> axum::response::Response {
    (
        status,
        [("content-type", "text/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Errors><Error><Code>{code}</Code><Message>{code}</Message></Error></Errors><RequestID>r-1</RequestID></Response>"),
    )
        .into_response()
}

fn tag_set(tags: &BTreeMap<String, String>) -> String {
    let items: String = tags
        .iter()
        .map(|(k, v)| format!("<item><key>{k}</key><value>{v}</value></item>"))
        .collect();
    format!("<tagSet>{items}</tagSet>")
}

/// `Filter.N.Name` / `Filter.N.Value.M` as (name, values).
pub fn filters(p: &BTreeMap<String, String>) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for n in 1.. {
        let Some(name) = p.get(&format!("Filter.{n}.Name")) else {
            break;
        };
        let values = (1..)
            .map_while(|m| p.get(&format!("Filter.{n}.Value.{m}")).cloned())
            .collect();
        out.push((name.clone(), values));
    }
    out
}

fn matches(f: &[(String, Vec<String>)], state: &str, tags: &BTreeMap<String, String>) -> bool {
    f.iter().all(|(name, values)| {
        if let Some(k) = name.strip_prefix("tag:") {
            tags.get(k).is_some_and(|v| values.contains(v))
        } else if name == "instance-state-name" {
            values.iter().any(|v| v == state)
        } else {
            true
        }
    })
}

/// `TagSpecification.N.*` of `resource` as tags.
pub fn spec_tags(p: &BTreeMap<String, String>, resource: &str) -> BTreeMap<String, String> {
    for n in 1.. {
        let Some(ty) = p.get(&format!("TagSpecification.{n}.ResourceType")) else {
            break;
        };
        if ty == resource {
            return (1..)
                .map_while(|m| {
                    let k = p.get(&format!("TagSpecification.{n}.Tag.{m}.Key"))?;
                    Some((
                        k.clone(),
                        p.get(&format!("TagSpecification.{n}.Tag.{m}.Value"))
                            .cloned()
                            .unwrap_or_default(),
                    ))
                })
                .collect();
        }
    }
    BTreeMap::new()
}

fn instance_xml(id: &str, state: &str, tags: &BTreeMap<String, String>) -> String {
    format!(
        "<item><instanceId>{id}</instanceId><instanceState><code>16</code><name>{state}</name></instanceState><launchTime>2026-09-30T00:00:00.000Z</launchTime>{}</item>",
        tag_set(tags)
    )
}

async fn handle(State(s): State<S>, body: Bytes) -> axum::response::Response {
    let p: BTreeMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let action = p.get("Action").cloned().unwrap_or_default();
    let delay = s.lock().unwrap().describe_delay_ms;
    if action == "DescribeSecurityGroups" && delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    let mut a = s.lock().unwrap();
    a.calls.push(p.clone());
    if p.get("DryRun").map(String::as_str) == Some("true") {
        return if a.deny {
            error(StatusCode::FORBIDDEN, "UnauthorizedOperation")
        } else {
            error(StatusCode::PRECONDITION_FAILED, "DryRunOperation")
        };
    }
    match action.as_str() {
        "GetCallerIdentity" => xml("<GetCallerIdentityResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><GetCallerIdentityResult><Arn>arn:aws:sts::111122223333:assumed-role/Admin/ada</Arn><UserId>X</UserId><Account>111122223333</Account></GetCallerIdentityResult><ResponseMetadata><RequestId>r</RequestId></ResponseMetadata></GetCallerIdentityResponse>".into()),
        "DescribeInstanceTypeOfferings" => ec2(&action, "<instanceTypeOfferingSet><item><instanceType>t4g.medium</instanceType><locationType>region</locationType><location>us-west-2</location></item></instanceTypeOfferingSet>"),
        "DescribeImages" => ec2(&action, "<imagesSet><item><imageId>ami-old</imageId><name>ubuntu-noble-old</name><creationDate>2026-01-01T00:00:00.000Z</creationDate><rootDeviceName>/dev/sda1</rootDeviceName></item><item><imageId>ami-new</imageId><name>ubuntu-noble-new</name><creationDate>2026-09-01T00:00:00.000Z</creationDate><rootDeviceName>/dev/sda1</rootDeviceName></item></imagesSet>"),
        "DescribeVpcs" => ec2(&action, "<vpcSet><item><vpcId>vpc-default</vpcId><isDefault>true</isDefault></item></vpcSet>"),
        "DescribeSecurityGroups" => {
            let f = filters(&p);
            let ids: Vec<String> = (1..).map_while(|n| p.get(&format!("GroupId.{n}")).cloned()).collect();
            if !ids.is_empty() && !ids.iter().all(|i| a.groups.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidGroup.NotFound");
            }
            let items: String = a
                .groups
                .iter()
                .filter(|(id, (_, _, tags))| (ids.is_empty() || ids.contains(id)) && matches(&f, "", tags))
                .map(|(id, (name, vpc, tags))| format!("<item><groupId>{id}</groupId><groupName>{name}</groupName><vpcId>{vpc}</vpcId>{}</item>", tag_set(tags)))
                .collect();
            ec2(&action, &format!("<securityGroupInfo>{items}</securityGroupInfo>"))
        }
        "CreateSecurityGroup" => {
            // EC2 refuses a second group of the same name in a VPC.
            if a.groups
                .values()
                .any(|(n, v, _)| *n == p["GroupName"] && *v == p["VpcId"])
            {
                return error(StatusCode::BAD_REQUEST, "InvalidGroup.Duplicate");
            }
            a.next += 1;
            let id = format!("sg-{}", a.next);
            let tags = spec_tags(&p, "security-group");
            a.groups.insert(id.clone(), (p["GroupName"].clone(), p["VpcId"].clone(), tags));
            ec2(&action, &format!("<return>true</return><groupId>{id}</groupId>"))
        }
        "AuthorizeSecurityGroupIngress" => {
            let get = |k: &str| p.get(&format!("IpPermissions.1.{k}")).cloned().unwrap_or_default();
            let rule = format!(
                "{} {} {}-{} {}",
                p["GroupId"],
                get("IpProtocol"),
                get("FromPort"),
                get("ToPort"),
                get("Groups.1.GroupId")
            );
            if !a.rules.insert(rule) {
                return error(StatusCode::BAD_REQUEST, "InvalidPermission.Duplicate");
            }
            ec2(&action, "<return>true</return>")
        }
        "DeleteSecurityGroup" => {
            a.groups.remove(&p["GroupId"]);
            ec2(&action, "<return>true</return>")
        }
        "GetConsoleOutput" => {
            use base64::Engine as _;
            let out = a
                .console
                .as_ref()
                .map(|c| base64::engine::general_purpose::STANDARD.encode(c))
                .unwrap_or_default();
            ec2(&action, &format!("<instanceId>{}</instanceId><output>{out}</output>", p["InstanceId"]))
        }
        "DescribeSubnets" => ec2(&action, "<subnetSet><item><subnetId>subnet-b</subnetId><availabilityZone>us-west-2b</availabilityZone></item><item><subnetId>subnet-a</subnetId><availabilityZone>us-west-2a</availabilityZone></item></subnetSet>"),
        "RunInstances" if a.full.contains(&p.get("SubnetId").cloned().unwrap_or_default()) => {
            error(StatusCode::BAD_REQUEST, "InsufficientInstanceCapacity")
        }
        "RunInstances" if a.unsupported.contains(&p.get("SubnetId").cloned().unwrap_or_default()) => {
            error(StatusCode::BAD_REQUEST, "Unsupported")
        }
        "RunInstances" => {
            a.next += 1;
            let id = format!("i-{}", a.next);
            let tags = spec_tags(&p, "instance");
            a.instances.insert(id.clone(), ("pending".into(), tags.clone()));
            ec2(&action, &format!("<reservationId>r-1</reservationId><instancesSet>{}</instancesSet>", instance_xml(&id, "pending", &tags)))
        }
        "DescribeInstances" => {
            let f = filters(&p);
            let ids: Vec<String> = (1..).map_while(|n| p.get(&format!("InstanceId.{n}")).cloned()).collect();
            if !ids.is_empty() && !ids.iter().all(|i| a.instances.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidInstanceID.NotFound");
            }
            let items: String = a
                .instances
                .iter()
                .filter(|(id, (state, tags))| (ids.is_empty() || ids.contains(id)) && matches(&f, state, tags))
                .map(|(id, (state, tags))| instance_xml(id, state, tags))
                .collect();
            ec2(&action, &format!("<reservationSet><item><reservationId>r-1</reservationId><instancesSet>{items}</instancesSet></item></reservationSet>"))
        }
        "StopInstances" | "StartInstances" | "TerminateInstances" => {
            let id = p["InstanceId.1"].clone();
            let to = match action.as_str() {
                "StopInstances" => "stopped",
                "StartInstances" => "running",
                _ => "terminated",
            };
            if let Some(i) = a.instances.get_mut(&id) {
                i.0 = to.into();
            }
            let set = match action.as_str() {
                "StopInstances" => "instancesSet",
                "StartInstances" => "instancesSet",
                _ => "instancesSet",
            };
            ec2(&action, &format!("<{set}><item><instanceId>{id}</instanceId><currentState><code>0</code><name>{to}</name></currentState></item></{set}>"))
        }
        _ => error(StatusCode::BAD_REQUEST, "InvalidAction"),
    }
}

pub async fn fake() -> (String, S) {
    let s: S = Arc::default();
    let app = axum::Router::new().fallback(handle).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, s)
}
