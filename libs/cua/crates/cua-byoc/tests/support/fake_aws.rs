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
    /// Zones where RunInstances has no capacity.
    pub full: Vec<String>,
    /// Zones where the type is not offered (`Unsupported`).
    pub unsupported: Vec<String>,
    /// id -> (cidr, is default, tags). `vpc-default` is always there.
    pub vpcs: BTreeMap<String, (String, bool, BTreeMap<String, String>)>,
    /// id -> (vpc, zone, cidr, tags)
    pub subnets: BTreeMap<String, (String, String, String, BTreeMap<String, String>)>,
    /// id -> (attached vpc, tags)
    pub igws: BTreeMap<String, (Option<String>, BTreeMap<String, String>)>,
    /// id -> route table
    pub rts: BTreeMap<String, Rt>,
    /// Another process makes a VPC (id `vpc-0`) at the same moment as each
    /// CreateVpc.
    pub race_vpc: bool,
    /// The instance each RunInstances made: id -> (subnet, public ip, group).
    pub placed: BTreeMap<String, (String, bool, String)>,
    /// What the console shows (base64 on the wire), per instance.
    pub console: Option<String>,
    /// Ingress rules authorized: `group protocol from-to source-group`.
    pub rules: std::collections::BTreeSet<String>,
    /// Make each security group lookup slow (ms), so concurrent callers
    /// all see no group before any create lands.
    pub describe_delay_ms: u64,
}

/// A route table of the fake.
#[derive(Clone, Default)]
pub struct Rt {
    pub vpc: String,
    pub tags: BTreeMap<String, String>,
    /// (destination cidr, gateway id)
    pub routes: Vec<(String, String)>,
    /// (association id, subnet id)
    pub assocs: Vec<(String, String)>,
}

/// The zones the fake region has.
pub const ZONES: &[&str] = &["us-west-2a", "us-west-2b"];

pub type S = Arc<Mutex<Acct>>;

fn field_match(
    f: &[(String, Vec<String>)],
    attrs: &[(&str, &str)],
    tags: &BTreeMap<String, String>,
) -> bool {
    f.iter().all(|(name, values)| {
        if let Some(k) = name.strip_prefix("tag:") {
            tags.get(k).is_some_and(|v| values.contains(v))
        } else if let Some((_, have)) = attrs.iter().find(|(n, _)| n == name) {
            values.iter().any(|v| v == have)
        } else {
            true
        }
    })
}

fn ids(p: &BTreeMap<String, String>, key: &str) -> Vec<String> {
    (1..)
        .map_while(|n| p.get(&format!("{key}.{n}")).cloned())
        .collect()
}

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

fn instance_xml_vpc(id: &str, state: &str, tags: &BTreeMap<String, String>, vpc: &str) -> String {
    format!(
        "<item><instanceId>{id}</instanceId><vpcId>{vpc}</vpcId><instanceState><code>16</code><name>{state}</name></instanceState><launchTime>2026-09-30T00:00:00.000Z</launchTime>{}</item>",
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
        "DescribeVpcs" => {
            let f = filters(&p);
            let want = ids(&p, "VpcId");
            let items: String = a
                .vpcs
                .iter()
                .filter(|(id, (_, def, tags))| {
                    (want.is_empty() || want.contains(id))
                        && field_match(&f, &[("is-default", if *def { "true" } else { "false" })], tags)
                })
                .map(|(id, (cidr, def, tags))| format!("<item><vpcId>{id}</vpcId><cidrBlock>{cidr}</cidrBlock><isDefault>{def}</isDefault>{}</item>", tag_set(tags)))
                .collect();
            if !want.is_empty() && !want.iter().all(|i| a.vpcs.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidVpcID.NotFound");
            }
            ec2(&action, &format!("<vpcSet>{items}</vpcSet>"))
        }
        "CreateVpc" => {
            a.next += 1;
            let id = format!("vpc-{}", a.next);
            let tags = spec_tags(&p, "vpc");
            if a.race_vpc {
                a.vpcs.insert("vpc-0".into(), (p["CidrBlock"].clone(), false, tags.clone()));
            }
            a.vpcs.insert(id.clone(), (p["CidrBlock"].clone(), false, tags));
            ec2(&action, &format!("<vpc><vpcId>{id}</vpcId><cidrBlock>{}</cidrBlock></vpc>", p["CidrBlock"]))
        }
        "DeleteVpc" => {
            let id = p["VpcId"].clone();
            if !a.vpcs.contains_key(&id) {
                return error(StatusCode::BAD_REQUEST, "InvalidVpcID.NotFound");
            }
            let busy = a.subnets.values().any(|s| s.0 == id)
                || a.igws.values().any(|g| g.0.as_deref() == Some(id.as_str()))
                || a.groups.values().any(|g| g.1 == id)
                || a.instances.iter().any(|(i, (st, _))| st != "terminated" && a.placed.get(i).is_some_and(|x| a.subnets.get(&x.0).is_some_and(|s| s.0 == id)));
            if busy {
                return error(StatusCode::BAD_REQUEST, "DependencyViolation");
            }
            a.vpcs.remove(&id);
            ec2(&action, "<return>true</return>")
        }
        "DescribeAvailabilityZones" => {
            let items: String = ZONES
                .iter()
                .map(|z| format!("<item><zoneName>{z}</zoneName><zoneState>available</zoneState><zoneType>availability-zone</zoneType><optInStatus>opt-in-not-required</optInStatus></item>"))
                .collect();
            ec2(&action, &format!("<availabilityZoneInfo>{items}</availabilityZoneInfo>"))
        }
        "DescribeInternetGateways" => {
            let f = filters(&p);
            let want = ids(&p, "InternetGatewayId");
            if !want.is_empty() && !want.iter().all(|i| a.igws.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidInternetGatewayID.NotFound");
            }
            let items: String = a
                .igws
                .iter()
                .filter(|(id, (vpc, tags))| {
                    (want.is_empty() || want.contains(id))
                        && field_match(&f, &[("attachment.vpc-id", vpc.as_deref().unwrap_or(""))], tags)
                })
                .map(|(id, (vpc, tags))| {
                    let att = vpc.as_ref().map(|v| format!("<attachmentSet><item><vpcId>{v}</vpcId><state>available</state></item></attachmentSet>")).unwrap_or_else(|| "<attachmentSet/>".into());
                    format!("<item><internetGatewayId>{id}</internetGatewayId>{att}{}</item>", tag_set(tags))
                })
                .collect();
            ec2(&action, &format!("<internetGatewaySet>{items}</internetGatewaySet>"))
        }
        "CreateInternetGateway" => {
            a.next += 1;
            let id = format!("igw-{}", a.next);
            let tags = spec_tags(&p, "internet-gateway");
            a.igws.insert(id.clone(), (None, tags));
            ec2(&action, &format!("<internetGateway><internetGatewayId>{id}</internetGatewayId></internetGateway>"))
        }
        "AttachInternetGateway" => {
            if !a.vpcs.contains_key(&p["VpcId"]) {
                return error(StatusCode::BAD_REQUEST, "InvalidVpcID.NotFound");
            }
            match a.igws.get_mut(&p["InternetGatewayId"]) {
                Some(g) => g.0 = Some(p["VpcId"].clone()),
                None => return error(StatusCode::BAD_REQUEST, "InvalidInternetGatewayID.NotFound"),
            }
            ec2(&action, "<return>true</return>")
        }
        "DetachInternetGateway" => {
            match a.igws.get_mut(&p["InternetGatewayId"]) {
                Some(g) if g.0.as_deref() == Some(p["VpcId"].as_str()) => g.0 = None,
                Some(_) => return error(StatusCode::BAD_REQUEST, "Gateway.NotAttached"),
                None => return error(StatusCode::BAD_REQUEST, "InvalidInternetGatewayID.NotFound"),
            }
            ec2(&action, "<return>true</return>")
        }
        "DeleteInternetGateway" => {
            match a.igws.get(&p["InternetGatewayId"]) {
                Some((Some(_), _)) => return error(StatusCode::BAD_REQUEST, "DependencyViolation"),
                Some(_) => {}
                None => return error(StatusCode::BAD_REQUEST, "InvalidInternetGatewayID.NotFound"),
            }
            a.igws.remove(&p["InternetGatewayId"]);
            ec2(&action, "<return>true</return>")
        }
        "DescribeRouteTables" => {
            let f = filters(&p);
            let want = ids(&p, "RouteTableId");
            if !want.is_empty() && !want.iter().all(|i| a.rts.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidRouteTableID.NotFound");
            }
            let items: String = a
                .rts
                .iter()
                .filter(|(id, rt)| (want.is_empty() || want.contains(id)) && field_match(&f, &[("vpc-id", rt.vpc.as_str())], &rt.tags))
                .map(|(id, rt)| {
                    let routes: String = rt.routes.iter().map(|(c, g)| format!("<item><destinationCidrBlock>{c}</destinationCidrBlock><gatewayId>{g}</gatewayId><state>active</state></item>")).collect();
                    let assocs: String = rt.assocs.iter().map(|(i, sn)| format!("<item><routeTableAssociationId>{i}</routeTableAssociationId><routeTableId>{id}</routeTableId><subnetId>{sn}</subnetId><main>false</main></item>")).collect();
                    format!("<item><routeTableId>{id}</routeTableId><vpcId>{}</vpcId><routeSet>{routes}</routeSet><associationSet>{assocs}</associationSet>{}</item>", rt.vpc, tag_set(&rt.tags))
                })
                .collect();
            ec2(&action, &format!("<routeTableSet>{items}</routeTableSet>"))
        }
        "CreateRouteTable" => {
            a.next += 1;
            let id = format!("rtb-{}", a.next);
            let rt = Rt { vpc: p["VpcId"].clone(), tags: spec_tags(&p, "route-table"), ..Default::default() };
            a.rts.insert(id.clone(), rt);
            ec2(&action, &format!("<routeTable><routeTableId>{id}</routeTableId></routeTable>"))
        }
        "CreateRoute" => {
            let Some(rt) = a.rts.get_mut(&p["RouteTableId"]) else {
                return error(StatusCode::BAD_REQUEST, "InvalidRouteTableID.NotFound");
            };
            let dst = p["DestinationCidrBlock"].clone();
            if rt.routes.iter().any(|(c, _)| *c == dst) {
                return error(StatusCode::BAD_REQUEST, "RouteAlreadyExists");
            }
            rt.routes.push((dst, p["GatewayId"].clone()));
            ec2(&action, "<return>true</return>")
        }
        "AssociateRouteTable" => {
            let sn = p["SubnetId"].clone();
            if !a.subnets.contains_key(&sn) {
                return error(StatusCode::BAD_REQUEST, "InvalidSubnetID.NotFound");
            }
            a.next += 1;
            let assoc = format!("rtbassoc-{}", a.next);
            let Some(rt) = a.rts.get_mut(&p["RouteTableId"]) else {
                return error(StatusCode::BAD_REQUEST, "InvalidRouteTableID.NotFound");
            };
            rt.assocs.push((assoc.clone(), sn));
            ec2(&action, &format!("<associationId>{assoc}</associationId>"))
        }
        "DisassociateRouteTable" => {
            for rt in a.rts.values_mut() {
                rt.assocs.retain(|(i, _)| *i != p["AssociationId"]);
            }
            ec2(&action, "<return>true</return>")
        }
        "DeleteRouteTable" => {
            match a.rts.get(&p["RouteTableId"]) {
                Some(rt) if !rt.assocs.is_empty() => return error(StatusCode::BAD_REQUEST, "DependencyViolation"),
                Some(_) => {}
                None => return error(StatusCode::BAD_REQUEST, "InvalidRouteTableID.NotFound"),
            }
            a.rts.remove(&p["RouteTableId"]);
            ec2(&action, "<return>true</return>")
        }
        "DescribeSubnets" => {
            let f = filters(&p);
            let want = ids(&p, "SubnetId");
            if !want.is_empty() && !want.iter().all(|i| a.subnets.contains_key(i)) {
                return error(StatusCode::BAD_REQUEST, "InvalidSubnetID.NotFound");
            }
            let items: String = a
                .subnets
                .iter()
                .filter(|(id, (vpc, zone, _, tags))| {
                    (want.is_empty() || want.contains(id))
                        && field_match(
                            &f,
                            &[
                                ("vpc-id", vpc.as_str()),
                                ("availability-zone", zone.as_str()),
                                ("default-for-az", if vpc == "vpc-default" { "true" } else { "false" }),
                            ],
                            tags,
                        )
                })
                .map(|(id, (vpc, zone, cidr, tags))| format!("<item><subnetId>{id}</subnetId><vpcId>{vpc}</vpcId><availabilityZone>{zone}</availabilityZone><cidrBlock>{cidr}</cidrBlock>{}</item>", tag_set(tags)))
                .collect();
            ec2(&action, &format!("<subnetSet>{items}</subnetSet>"))
        }
        "CreateSubnet" => {
            if !a.vpcs.contains_key(&p["VpcId"]) {
                return error(StatusCode::BAD_REQUEST, "InvalidVpcID.NotFound");
            }
            if a.subnets.values().any(|s| s.0 == p["VpcId"] && s.2 == p["CidrBlock"]) {
                return error(StatusCode::BAD_REQUEST, "InvalidSubnet.Conflict");
            }
            a.next += 1;
            let id = format!("subnet-{}", a.next);
            let tags = spec_tags(&p, "subnet");
            a.subnets.insert(id.clone(), (p["VpcId"].clone(), p["AvailabilityZone"].clone(), p["CidrBlock"].clone(), tags));
            ec2(&action, &format!("<subnet><subnetId>{id}</subnetId></subnet>"))
        }
        "DeleteSubnet" => {
            let id = p["SubnetId"].clone();
            if !a.subnets.contains_key(&id) {
                return error(StatusCode::BAD_REQUEST, "InvalidSubnetID.NotFound");
            }
            let busy = a.instances.iter().any(|(i, (st, _))| st != "terminated" && a.placed.get(i).is_some_and(|x| x.0 == id));
            if busy {
                return error(StatusCode::BAD_REQUEST, "DependencyViolation");
            }
            a.subnets.remove(&id);
            for rt in a.rts.values_mut() {
                rt.assocs.retain(|(_, sn)| *sn != id);
            }
            ec2(&action, "<return>true</return>")
        }
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
        "RunInstances" => {
            let subnet = p.get("NetworkInterface.1.SubnetId").or(p.get("SubnetId")).cloned().unwrap_or_default();
            let zone = a.subnets.get(&subnet).map(|s| s.1.clone()).unwrap_or_default();
            if a.full.contains(&zone) {
                return error(StatusCode::BAD_REQUEST, "InsufficientInstanceCapacity");
            }
            if a.unsupported.contains(&zone) {
                return error(StatusCode::BAD_REQUEST, "Unsupported");
            }
            a.next += 1;
            let id = format!("i-{}", a.next);
            let tags = spec_tags(&p, "instance");
            let public = p.get("NetworkInterface.1.AssociatePublicIpAddress").map(String::as_str) == Some("true");
            let group = p.get("NetworkInterface.1.SecurityGroupId.1").or(p.get("SecurityGroupId.1")).cloned().unwrap_or_default();
            a.placed.insert(id.clone(), (subnet.clone(), public, group));
            a.instances.insert(id.clone(), ("pending".into(), tags.clone()));
            let vpc = a.subnets.get(&subnet).map(|s| s.0.clone()).unwrap_or_else(|| "vpc-default".into());
            ec2(&action, &format!("<reservationId>r-1</reservationId><instancesSet>{}</instancesSet>", instance_xml_vpc(&id, "pending", &tags, &vpc)))
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
                .map(|(id, (state, tags))| {
                    let vpc = a.placed.get(id).and_then(|x| a.subnets.get(&x.0)).map(|s| s.0.clone()).unwrap_or_else(|| "vpc-default".into());
                    instance_xml_vpc(id, state, tags, &vpc)
                })
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
    s.lock().unwrap().vpcs.insert(
        "vpc-default".into(),
        ("172.31.0.0/16".into(), true, BTreeMap::new()),
    );
    let app = axum::Router::new().fallback(handle).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, s)
}

/// [`fake`] with the default VPC's default subnets, one per zone (what a
/// launch without Cua's own VPC lands in).
#[allow(dead_code)]
pub async fn fake_with_default_subnets() -> (String, S) {
    let (url, s) = fake().await;
    {
        let mut a = s.lock().unwrap();
        for (id, zone, cidr) in [
            ("subnet-b", "us-west-2b", "172.31.16.0/20"),
            ("subnet-a", "us-west-2a", "172.31.0.0/20"),
        ] {
            a.subnets.insert(
                id.into(),
                (
                    "vpc-default".into(),
                    zone.into(),
                    cidr.into(),
                    BTreeMap::new(),
                ),
            );
        }
    }
    (url, s)
}
