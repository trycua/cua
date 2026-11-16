// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The network Cua's AWS machines live in: a VPC of its own, never the
//! account's default one.
//!
//! One VPC per cua home and region, `cua-sb-<owner>`:
//!
//! - a /16 (`10.77.0.0/16`), one public /20 subnet per availability zone
//!   that a computer was placed in, an internet gateway and one route table
//!   (`0.0.0.0/0` to the gateway). Machines get a public address and dial
//!   out; the group has no inbound rule.
//! - no peering, no endpoints, no NAT, no other route: a computer reaches
//!   the internet and nothing else in your account.
//! - every part carries Cua's tags (`cua-managed`, `cua-owner`, ...) and is
//!   recorded in this home's state ([`Resource`]); only parts that
//!   [`model::is_ours`] says are ours are ever read for write or changed.
//!
//! An existing VPC of ours is reused (and repaired if an earlier create
//! stopped half way); the default VPC and every VPC without our tags is left
//! alone.

use std::collections::BTreeMap;

use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::types::{Filter, ResourceType, Tag, TagSpecification};
use cua_sandbox_core::Error;

use crate::api::Result;
use crate::aws::aws_err as err;
use crate::model::{self, Resource, tag};

/// The `cua-group` tag value of the VPC and the parts made with it.
pub const VPC_GROUP: &str = "sandbox-vpc";

/// The VPC's address range.
pub const VPC_CIDR: &str = "10.77.0.0/16";

/// Whether Cua's own VPC is in use: it is part of Secure Spaces. Without
/// the experiment sandboxes go where they always went, the account's
/// default VPC.
pub fn enabled() -> bool {
    cua_vmm::network::Mode::from_env() == cua_vmm::network::Mode::Secure
}

/// The most availability zones a region can have subnets in (a /16 holds
/// sixteen /20s; AWS regions have at most six zones).
pub const MAX_ZONES: usize = 16;

/// The name of `owner`'s VPC.
pub fn vpc_name(owner: &str) -> String {
    format!("cua-sb-{}", model::label_value(owner))
}

/// The subnet range of the zone at `index` in the region's sorted zone
/// list: `10.77.<16*index>.0/20`.
pub fn subnet_cidr(index: usize) -> Option<String> {
    (index < MAX_ZONES).then(|| format!("10.77.{}.0/20", index * 16))
}

/// The resource types of the VPC's parts, in the order they are deleted
/// (a computer and its group first; the VPC last).
pub const DELETE_ORDER: &[&str] = &[
    "security_group",
    "subnet",
    "route_table",
    "internet_gateway",
    "vpc",
];

/// Whether a record type is a part of the Cua VPC.
pub fn is_network_type(t: &str) -> bool {
    matches!(t, "vpc" | "subnet" | "route_table" | "internet_gateway")
}

fn filter(name: &str, values: &[&str]) -> Filter {
    Filter::builder()
        .name(name)
        .set_values(Some(values.iter().map(|v| v.to_string()).collect()))
        .build()
}

fn owner_filters(owner: &str) -> Vec<Filter> {
    vec![
        filter(&format!("tag:{}", tag::MANAGED), &["true"]),
        filter(
            &format!("tag:{}", tag::OWNER),
            &[&model::label_value(owner)],
        ),
        filter(&format!("tag:{}", tag::GROUP), &[VPC_GROUP]),
    ]
}

fn tags_of(tags: &[Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|t| {
            Some((
                t.key()?.to_string(),
                t.value().unwrap_or_default().to_string(),
            ))
        })
        .collect()
}

/// The tags every part carries, for `owner`, plus its name.
pub fn part_tags(owner: &str, name: &str) -> BTreeMap<String, String> {
    let mut t = model::tags(owner, "", "", 0);
    t.insert(tag::GROUP.into(), VPC_GROUP.into());
    t.insert("Name".into(), name.into());
    t
}

fn spec(kind: ResourceType, tags: &BTreeMap<String, String>) -> TagSpecification {
    let mut s = TagSpecification::builder().resource_type(kind);
    for (k, v) in tags {
        s = s.tags(Tag::builder().key(k).value(v).build());
    }
    s.build()
}

/// What [`ensure_vpc`] found or made.
#[derive(Clone, Debug, Default)]
pub struct Vpc {
    /// `vpc-...`.
    pub id: String,
    /// The route table with the way out.
    pub route_table: String,
    /// The internet gateway.
    pub gateway: String,
    /// The records of the parts made now (empty when all existed).
    pub made: Vec<Resource>,
}

fn record(
    conn_region: &str,
    kind: &str,
    id: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
    extra: &[(&str, &str)],
) -> Resource {
    Resource {
        provider: "aws".into(),
        id: id.into(),
        resource_type: kind.into(),
        name: name.into(),
        region: conn_region.into(),
        created: model::now(),
        tags: tags.clone(),
        state: "available".into(),
        extra: extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

/// The VPC of `owner` in the client's region, if one exists and is ours.
pub async fn find_vpc(ec2: &aws_sdk_ec2::Client, owner: &str) -> Result<Option<String>> {
    let mut req = ec2.describe_vpcs();
    for f in owner_filters(owner) {
        req = req.filters(f);
    }
    let out = req.send().await.map_err(|e| err("find the Cua VPC", e))?;
    // The lowest id when two processes raced to create one: all of them
    // settle on the same VPC.
    Ok(out
        .vpcs()
        .iter()
        .filter(|v| !v.is_default().unwrap_or(false) && model::is_ours(&tags_of(v.tags()), owner))
        .filter_map(|v| v.vpc_id())
        .min()
        .map(str::to_string))
}

/// Finds or creates the VPC, its gateway and its route table. Every step
/// looks for what is already there first, so an interrupted create is
/// finished by the next call. One caller at a time per cua home (the
/// provider holds a lock).
pub async fn ensure_vpc(ec2: &aws_sdk_ec2::Client, region: &str, owner: &str) -> Result<Vpc> {
    let name = vpc_name(owner);
    let tags = part_tags(owner, &name);
    let mut made: Vec<Resource> = Vec::new();
    let vpc_id = match find_vpc(ec2, owner).await? {
        Some(id) => id,
        None => {
            let created = ec2
                .create_vpc()
                .cidr_block(VPC_CIDR)
                .tag_specifications(spec(ResourceType::Vpc, &tags))
                .send()
                .await
                .map_err(|e| err("create the Cua VPC", e))?;
            let id = created
                .vpc()
                .and_then(|v| v.vpc_id())
                .ok_or_else(|| Error::Cloud("aws: CreateVpc returned no VPC".into()))?
                .to_string();
            // Two processes may have created one at the same moment (EC2
            // has no unique name): they all keep the lowest id, and the
            // other one, which is empty and ours, goes.
            match find_vpc(ec2, owner).await? {
                Some(winner) if winner != id => {
                    delete_part(ec2, "vpc", &id).await?;
                    winner
                }
                _ => {
                    made.push(record(
                        region,
                        "vpc",
                        &id,
                        &name,
                        &tags,
                        &[("cidr", VPC_CIDR)],
                    ));
                    id
                }
            }
        }
    };

    // The internet gateway attached to this VPC and tagged ours.
    let gws = ec2
        .describe_internet_gateways()
        .filters(filter("attachment.vpc-id", &[&vpc_id]))
        .send()
        .await
        .map_err(|e| err("list internet gateways", e))?;
    let gw = gws
        .internet_gateways()
        .iter()
        .find(|g| model::is_ours(&tags_of(g.tags()), owner))
        .and_then(|g| g.internet_gateway_id())
        .map(str::to_string);
    let gateway = match gw {
        Some(id) => id,
        None => {
            let gw_name = format!("{name}-igw");
            let gw_tags = part_tags(owner, &gw_name);
            let created = ec2
                .create_internet_gateway()
                .tag_specifications(spec(ResourceType::InternetGateway, &gw_tags))
                .send()
                .await
                .map_err(|e| err("create the internet gateway", e))?;
            let id = created
                .internet_gateway()
                .and_then(|g| g.internet_gateway_id())
                .ok_or_else(|| Error::Cloud("aws: CreateInternetGateway returned nothing".into()))?
                .to_string();
            made.push(record(
                region,
                "internet_gateway",
                &id,
                &gw_name,
                &gw_tags,
                &[("vpc", vpc_id.as_str())],
            ));
            ec2.attach_internet_gateway()
                .internet_gateway_id(&id)
                .vpc_id(&vpc_id)
                .send()
                .await
                .map_err(|e| err("attach the internet gateway", e))?;
            id
        }
    };

    // The route table: ours, in this VPC, with the default route to the
    // gateway.
    let rts = ec2
        .describe_route_tables()
        .filters(filter("vpc-id", &[&vpc_id]))
        .send()
        .await
        .map_err(|e| err("list route tables", e))?;
    let found = rts
        .route_tables()
        .iter()
        .find(|r| model::is_ours(&tags_of(r.tags()), owner));
    let (rt, has_route) = match found {
        Some(r) => (
            r.route_table_id().unwrap_or_default().to_string(),
            r.routes().iter().any(|x| {
                x.destination_cidr_block() == Some("0.0.0.0/0")
                    && x.gateway_id() == Some(gateway.as_str())
            }),
        ),
        None => {
            let rt_name = format!("{name}-rt");
            let rt_tags = part_tags(owner, &rt_name);
            let created = ec2
                .create_route_table()
                .vpc_id(&vpc_id)
                .tag_specifications(spec(ResourceType::RouteTable, &rt_tags))
                .send()
                .await
                .map_err(|e| err("create the route table", e))?;
            let id = created
                .route_table()
                .and_then(|r| r.route_table_id())
                .ok_or_else(|| Error::Cloud("aws: CreateRouteTable returned nothing".into()))?
                .to_string();
            made.push(record(
                region,
                "route_table",
                &id,
                &rt_name,
                &rt_tags,
                &[("vpc", vpc_id.as_str())],
            ));
            (id, false)
        }
    };
    if !has_route {
        match ec2
            .create_route()
            .route_table_id(&rt)
            .destination_cidr_block("0.0.0.0/0")
            .gateway_id(&gateway)
            .send()
            .await
        {
            Ok(_) => {}
            // Another run added it between the look and the call.
            Err(e) if e.code() == Some("RouteAlreadyExists") => {}
            Err(e) => return Err(err("add the route to the internet gateway", e)),
        }
    }
    Ok(Vpc {
        id: vpc_id,
        route_table: rt,
        gateway,
        made,
    })
}

/// The availability zones a computer may be placed in, in one stable order
/// (the position in this list picks the subnet's range).
pub async fn zones(ec2: &aws_sdk_ec2::Client) -> Result<Vec<String>> {
    let out = ec2
        .describe_availability_zones()
        .filters(filter("state", &["available"]))
        .filters(filter("zone-type", &["availability-zone"]))
        .filters(filter("opt-in-status", &["opt-in-not-required"]))
        .send()
        .await
        .map_err(|e| err("list availability zones", e))?;
    let mut z: Vec<String> = out
        .availability_zones()
        .iter()
        .filter_map(|z| z.zone_name().map(str::to_string))
        .collect();
    z.sort();
    z.truncate(MAX_ZONES);
    Ok(z)
}

/// The subnet of ours in `zone` of `vpc`, if there is one.
async fn find_subnet(
    ec2: &aws_sdk_ec2::Client,
    owner: &str,
    vpc: &str,
    zone: &str,
) -> Result<Option<String>> {
    let existing = ec2
        .describe_subnets()
        .filters(filter("vpc-id", &[vpc]))
        .filters(filter("availability-zone", &[zone]))
        .send()
        .await
        .map_err(|e| err("list the Cua subnets", e))?;
    Ok(existing
        .subnets()
        .iter()
        .find(|s| model::is_ours(&tags_of(s.tags()), owner))
        .and_then(|s| s.subnet_id())
        .map(str::to_string))
}

/// The public subnet of `zone` in `vpc`: found, or made and routed. Returns
/// its id and, when made now, its record.
pub async fn ensure_subnet(
    ec2: &aws_sdk_ec2::Client,
    region: &str,
    owner: &str,
    vpc: &Vpc,
    zone: &str,
    index: usize,
) -> Result<(String, Option<Resource>)> {
    let found = find_subnet(ec2, owner, &vpc.id, zone).await?;
    let (id, made) = match found {
        Some(id) => (id, None),
        None => {
            let cidr = subnet_cidr(index).ok_or_else(|| {
                Error::Cloud(format!("aws: no subnet range left for zone {zone}"))
            })?;
            let name = format!("{}-{zone}", vpc_name(owner));
            let tags = part_tags(owner, &name);
            match ec2
                .create_subnet()
                .vpc_id(&vpc.id)
                .availability_zone(zone)
                .cidr_block(&cidr)
                .tag_specifications(spec(ResourceType::Subnet, &tags))
                .send()
                .await
            {
                Ok(out) => {
                    let id = out
                        .subnet()
                        .and_then(|s| s.subnet_id())
                        .ok_or_else(|| Error::Cloud("aws: CreateSubnet returned no subnet".into()))?
                        .to_string();
                    (
                        id.clone(),
                        Some(record(
                            region,
                            "subnet",
                            &id,
                            &name,
                            &tags,
                            &[
                                ("vpc", vpc.id.as_str()),
                                ("zone", zone),
                                ("cidr", cidr.as_str()),
                            ],
                        )),
                    )
                }
                // Another process made this zone's subnet between the look
                // and the create: it is the one to use.
                Err(e) if e.code() == Some("InvalidSubnet.Conflict") => {
                    let mut found = None;
                    for wait in [0u64, 500, 1000, 2000, 4000] {
                        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                        found = find_subnet(ec2, owner, &vpc.id, zone).await?;
                        if found.is_some() {
                            break;
                        }
                    }
                    match found {
                        Some(id) => (id, None),
                        None => return Err(err("create the subnet", e)),
                    }
                }
                Err(e) => return Err(err("create the subnet", e)),
            }
        }
    };
    // Routed through the table (a subnet from an interrupted create may
    // lack the association).
    let rts = ec2
        .describe_route_tables()
        .route_table_ids(&vpc.route_table)
        .send()
        .await
        .map_err(|e| err("read the route table", e))?;
    let routed = rts
        .route_tables()
        .iter()
        .flat_map(|r| r.associations())
        .any(|a| a.subnet_id() == Some(id.as_str()));
    if !routed {
        ec2.associate_route_table()
            .route_table_id(&vpc.route_table)
            .subnet_id(&id)
            .send()
            .await
            .map_err(|e| err("route the subnet to the internet", e))?;
    }
    Ok((id, made))
}

/// Deletes one part of the VPC (the caller has checked it is ours). A part a
/// machine that is still shutting down holds is retried for a while.
pub async fn delete_part(ec2: &aws_sdk_ec2::Client, kind: &str, id: &str) -> Result<()> {
    use std::time::Duration;
    macro_rules! retry {
        ($what:expr, $call:expr, $gone:expr) => {{
            let mut tries = 0;
            loop {
                match $call.await {
                    Ok(_) => break Ok(()),
                    Err(e) if e.code() == Some($gone) => break Ok(()),
                    Err(e) if e.code() == Some("DependencyViolation") && tries < 24 => {
                        tries += 1;
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(e) => break Err(err($what, e)),
                }
            }
        }};
    }
    match kind {
        "subnet" => retry!(
            "delete the subnet",
            ec2.delete_subnet().subnet_id(id).send(),
            "InvalidSubnetID.NotFound"
        ),
        "route_table" => {
            let rt = match ec2.describe_route_tables().route_table_ids(id).send().await {
                Ok(o) => o,
                Err(e) if e.code() == Some("InvalidRouteTableID.NotFound") => return Ok(()),
                Err(e) => return Err(err("read the route table", e)),
            };
            for a in rt.route_tables().iter().flat_map(|r| r.associations()) {
                if a.main().unwrap_or(false) {
                    continue;
                }
                if let Some(assoc) = a.route_table_association_id() {
                    let _ = ec2
                        .disassociate_route_table()
                        .association_id(assoc)
                        .send()
                        .await;
                }
            }
            retry!(
                "delete the route table",
                ec2.delete_route_table().route_table_id(id).send(),
                "InvalidRouteTableID.NotFound"
            )
        }
        "internet_gateway" => {
            let gw = match ec2
                .describe_internet_gateways()
                .internet_gateway_ids(id)
                .send()
                .await
            {
                Ok(o) => o,
                Err(e) if e.code() == Some("InvalidInternetGatewayID.NotFound") => return Ok(()),
                Err(e) => return Err(err("read the internet gateway", e)),
            };
            for vpc in gw
                .internet_gateways()
                .iter()
                .flat_map(|g| g.attachments())
                .filter_map(|a| a.vpc_id())
            {
                // Held while a computer's public address is still released.
                retry!(
                    "detach the internet gateway",
                    ec2.detach_internet_gateway()
                        .internet_gateway_id(id)
                        .vpc_id(vpc)
                        .send(),
                    "Gateway.NotAttached"
                )?;
            }
            retry!(
                "delete the internet gateway",
                ec2.delete_internet_gateway().internet_gateway_id(id).send(),
                "InvalidInternetGatewayID.NotFound"
            )
        }
        "vpc" => retry!(
            "delete the VPC",
            ec2.delete_vpc().vpc_id(id).send(),
            "InvalidVpcID.NotFound"
        ),
        other => Err(Error::InvalidArgument(format!(
            "aws: {other} is not a part of the Cua VPC"
        ))),
    }
}

/// The tags the cloud shows for a part (`None`: gone).
pub async fn describe_part(
    ec2: &aws_sdk_ec2::Client,
    kind: &str,
    id: &str,
) -> Result<Option<BTreeMap<String, String>>> {
    let gone = |c: Option<&str>| {
        matches!(
            c,
            Some(
                "InvalidVpcID.NotFound"
                    | "InvalidSubnetID.NotFound"
                    | "InvalidRouteTableID.NotFound"
                    | "InvalidInternetGatewayID.NotFound"
            )
        )
    };
    match kind {
        "vpc" => match ec2.describe_vpcs().vpc_ids(id).send().await {
            Ok(o) => Ok(o.vpcs().first().map(|v| tags_of(v.tags()))),
            Err(e) if gone(e.code()) => Ok(None),
            Err(e) => Err(err("describe the VPC", e)),
        },
        "subnet" => match ec2.describe_subnets().subnet_ids(id).send().await {
            Ok(o) => Ok(o.subnets().first().map(|s| tags_of(s.tags()))),
            Err(e) if gone(e.code()) => Ok(None),
            Err(e) => Err(err("describe the subnet", e)),
        },
        "route_table" => match ec2.describe_route_tables().route_table_ids(id).send().await {
            Ok(o) => Ok(o.route_tables().first().map(|r| tags_of(r.tags()))),
            Err(e) if gone(e.code()) => Ok(None),
            Err(e) => Err(err("describe the route table", e)),
        },
        "internet_gateway" => match ec2
            .describe_internet_gateways()
            .internet_gateway_ids(id)
            .send()
            .await
        {
            Ok(o) => Ok(o.internet_gateways().first().map(|g| tags_of(g.tags()))),
            Err(e) if gone(e.code()) => Ok(None),
            Err(e) => Err(err("describe the internet gateway", e)),
        },
        other => Err(Error::InvalidArgument(format!(
            "aws: {other} is not a part of the Cua VPC"
        ))),
    }
}

/// Every part of `owner`'s VPC the cloud lists in this region, as records.
pub async fn list_parts(
    ec2: &aws_sdk_ec2::Client,
    region: &str,
    owner: &str,
) -> Result<Vec<Resource>> {
    let mut out = Vec::new();
    let mut push = |kind: &str, id: Option<&str>, tags: BTreeMap<String, String>, vpc: &str| {
        if !model::is_ours(&tags, owner) {
            return;
        }
        let mut r = record(
            region,
            kind,
            id.unwrap_or_default(),
            tags.get("Name").map(String::as_str).unwrap_or_default(),
            &tags,
            &[],
        );
        if !vpc.is_empty() {
            r.extra.insert("vpc".into(), vpc.into());
        }
        r.expires = 0;
        out.push(r);
    };
    let mut req = ec2.describe_vpcs();
    for f in owner_filters(owner) {
        req = req.filters(f);
    }
    for v in req
        .send()
        .await
        .map_err(|e| err("list Cua VPCs", e))?
        .vpcs()
    {
        if !v.is_default().unwrap_or(false) {
            push("vpc", v.vpc_id(), tags_of(v.tags()), "");
        }
    }
    let mut req = ec2.describe_subnets();
    for f in owner_filters(owner) {
        req = req.filters(f);
    }
    for s in req
        .send()
        .await
        .map_err(|e| err("list Cua subnets", e))?
        .subnets()
    {
        push(
            "subnet",
            s.subnet_id(),
            tags_of(s.tags()),
            s.vpc_id().unwrap_or_default(),
        );
    }
    let mut req = ec2.describe_route_tables();
    for f in owner_filters(owner) {
        req = req.filters(f);
    }
    for r in req
        .send()
        .await
        .map_err(|e| err("list Cua route tables", e))?
        .route_tables()
    {
        push(
            "route_table",
            r.route_table_id(),
            tags_of(r.tags()),
            r.vpc_id().unwrap_or_default(),
        );
    }
    let mut req = ec2.describe_internet_gateways();
    for f in owner_filters(owner) {
        req = req.filters(f);
    }
    for g in req
        .send()
        .await
        .map_err(|e| err("list Cua internet gateways", e))?
        .internet_gateways()
    {
        push(
            "internet_gateway",
            g.internet_gateway_id(),
            tags_of(g.tags()),
            "",
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vpc_is_a_slash_16_with_a_slash_20_per_zone_and_they_never_overlap() {
        assert_eq!(VPC_CIDR, "10.77.0.0/16");
        assert_eq!(subnet_cidr(0).unwrap(), "10.77.0.0/20");
        assert_eq!(subnet_cidr(1).unwrap(), "10.77.16.0/20");
        assert_eq!(subnet_cidr(15).unwrap(), "10.77.240.0/20");
        assert!(subnet_cidr(16).is_none());
        let starts: Vec<usize> = (0..MAX_ZONES).map(|i| i * 16 * 256).collect();
        for w in starts.windows(2) {
            assert_eq!(w[1] - w[0], 4096, "a /20 is 4096 addresses");
        }
        assert!(*starts.last().unwrap() + 4096 <= 65536);
    }

    #[test]
    fn names_and_tags_are_cuas() {
        assert_eq!(vpc_name("Abc_123"), "cua-sb-abc_123");
        let t = part_tags("abc", "cua-sb-abc");
        assert!(model::is_ours(&t, "abc"));
        assert!(!model::is_ours(&t, "other"));
        assert_eq!(t.get(tag::GROUP).map(String::as_str), Some(VPC_GROUP));
        assert_eq!(t.get("Name").map(String::as_str), Some("cua-sb-abc"));
    }

    #[test]
    fn the_delete_order_ends_with_the_vpc_and_starts_with_the_group() {
        assert_eq!(DELETE_ORDER.first(), Some(&"security_group"));
        assert_eq!(DELETE_ORDER.last(), Some(&"vpc"));
        for t in ["vpc", "subnet", "route_table", "internet_gateway"] {
            assert!(is_network_type(t));
        }
        assert!(!is_network_type("instance"));
        assert!(!is_network_type("security_group"));
    }
}
