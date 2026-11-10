// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Public TCP shares on AWS (CUA-2602): one security group rule per share
//! on a Space's own machine, and the share edge (one `t4g.nano`, its own
//! security group and an Elastic IP) in Cua's VPC of the account.
//!
//! Every call goes through the same OIDC role as the rest of `aws`
//! ([`crate::aws::Aws::clients`]); the role's policy
//! (`policy/aws-policy.json`) lets it change only resources tagged
//! `cua-managed=true`, tag only what it creates, and re-tag a share edge's
//! expiry and nothing else.

use std::collections::BTreeMap;

use async_trait::async_trait;
use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::types::{Filter, IpPermission, IpRange, ResourceType, Tag, TagSpecification};
use cua_sandbox_core::Error;

use crate::api::Result;
use crate::aws::{Aws, aws_err as err};
use crate::federation::Purpose;
use crate::model::{Connection, tag};
use crate::share_edge::{EdgeDeployed, EdgePlan, ROLE_EDGE, ROLE_SHARE_RULE};
use crate::share_net::{OpenRule, ShareNetApi};

fn filter(name: &str, values: &[&str]) -> Filter {
    Filter::builder()
        .name(name)
        .set_values(Some(values.iter().map(|v| v.to_string()).collect()))
        .build()
}

fn spec(kind: ResourceType, tags: &BTreeMap<String, String>) -> TagSpecification {
    let mut b = TagSpecification::builder().resource_type(kind);
    for (k, v) in tags {
        b = b.tags(Tag::builder().key(k).value(v).build());
    }
    b.build()
}

fn tags_of(tags: &[Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|t| {
            Some((
                t.key()?.to_owned(),
                t.value().unwrap_or_default().to_owned(),
            ))
        })
        .collect()
}

fn owned(owner: &str, role: &str) -> Vec<Filter> {
    vec![
        filter(&format!("tag:{}", tag::MANAGED), &["true"]),
        filter(&format!("tag:{}", tag::OWNER), &[owner]),
        filter(&format!("tag:{}", tag::ROLE), &[role]),
    ]
}

fn duplicate<E: ProvideErrorMetadata>(e: &E) -> bool {
    e.code() == Some("InvalidPermission.Duplicate")
}

fn gone<E: ProvideErrorMetadata>(e: &E) -> bool {
    matches!(
        e.code(),
        Some(
            "InvalidPermission.NotFound"
                | "InvalidGroup.NotFound"
                | "InvalidInstanceID.NotFound"
                | "InvalidAllocationID.NotFound"
                | "InvalidAssociationID.NotFound"
        )
    )
}

#[async_trait]
impl ShareNetApi for Aws {
    fn provider(&self) -> &'static str {
        "aws"
    }

    async fn open_port(
        &self,
        conn: &Connection,
        group: &str,
        port: u16,
        cidrs: &[String],
        tags: &BTreeMap<String, String>,
    ) -> Result<String> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let mut perm = IpPermission::builder()
            .ip_protocol("tcp")
            .from_port(i32::from(port))
            .to_port(i32::from(port));
        for cidr in cidrs {
            if cidr.contains(':') {
                perm = perm.ipv6_ranges(
                    aws_sdk_ec2::types::Ipv6Range::builder()
                        .cidr_ipv6(cidr)
                        .description("cua public TCP share")
                        .build(),
                );
            } else {
                perm = perm.ip_ranges(
                    IpRange::builder()
                        .cidr_ip(cidr)
                        .description("cua public TCP share")
                        .build(),
                );
            }
        }
        let out = c
            .ec2
            .authorize_security_group_ingress()
            .group_id(group)
            .ip_permissions(perm.build())
            .tag_specifications(spec(ResourceType::SecurityGroupRule, tags))
            .send()
            .await;
        match out {
            Ok(o) => o
                .security_group_rules()
                .first()
                .and_then(|r| r.security_group_rule_id())
                .map(str::to_owned)
                .ok_or_else(|| Error::Cloud("AWS returned no rule id".into())),
            Err(e) if e.as_service_error().is_some_and(duplicate) => Err(Error::InvalidArgument(
                format!("port {port} of that computer is already open"),
            )),
            Err(e) => Err(err("open a share port", e)),
        }
    }

    async fn close_port(&self, conn: &Connection, group: &str, rule: &str) -> Result<()> {
        let c = self.clients(conn, Purpose::Manage).await?;
        match c
            .ec2
            .revoke_security_group_ingress()
            .group_id(group)
            .security_group_rule_ids(rule)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.as_service_error().is_some_and(gone) => Ok(()),
            Err(e) => Err(err("close a share port", e)),
        }
    }

    async fn list_rules(&self, conn: &Connection, owner: &str) -> Result<Vec<OpenRule>> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let mut out = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let page = c
                .ec2
                .describe_security_group_rules()
                .set_filters(Some(owned(owner, ROLE_SHARE_RULE)))
                .set_next_token(next.clone())
                .send()
                .await
                .map_err(|e| err("list share ports", e))?;
            for r in page.security_group_rules() {
                if r.is_egress() == Some(true) {
                    continue;
                }
                let cidrs = r
                    .cidr_ipv4()
                    .or(r.cidr_ipv6())
                    .map(|c| vec![c.to_owned()])
                    .unwrap_or_default();
                out.push(OpenRule {
                    id: r.security_group_rule_id().unwrap_or_default().to_owned(),
                    group: r.group_id().unwrap_or_default().to_owned(),
                    port: r.from_port().unwrap_or(0).clamp(0, 65535) as u16,
                    cidrs,
                    tags: tags_of(r.tags()),
                });
            }
            next = page.next_token().map(str::to_owned);
            if next.is_none() {
                break;
            }
        }
        Ok(out)
    }

    async fn deploy_edge(&self, conn: &Connection, plan: &EdgePlan) -> Result<EdgeDeployed> {
        let c = self.clients(conn, Purpose::Create).await?;
        let ec2 = &c.ec2;
        let owner = plan.tags.get(tag::OWNER).cloned().unwrap_or_default();
        // Cua's VPC of this account (never the default one), and a subnet.
        let vpc = crate::awsnet::ensure_vpc(ec2, &conn.region, &owner).await?;
        let zones = crate::awsnet::zones(ec2).await?;
        let zone = zones
            .first()
            .cloned()
            .ok_or_else(|| Error::Cloud("no availability zone".into()))?;
        let (subnet, _) =
            crate::awsnet::ensure_subnet(ec2, &conn.region, &owner, &vpc, &zone, 0).await?;

        // Everything made so far, removed again if a later step fails.
        let mut made = EdgeDeployed {
            provider: "aws".into(),
            region: conn.region.clone(),
            instance: String::new(),
            group: String::new(),
            address_id: String::new(),
            ip: String::new(),
            host: String::new(),
            ports: plan.ports,
            expires: plan.expires,
        };
        let result: Result<()> = async {
            let sg = ec2
                .create_security_group()
                .group_name(&plan.name)
                .description("Cua share edge: the tunnel and the share ports, nothing else")
                .vpc_id(&vpc.id)
                .tag_specifications(spec(ResourceType::SecurityGroup, &plan.tags))
                .send()
                .await
                .map_err(|e| err("create the share edge's firewall", e))?;
            made.group = sg.group_id().unwrap_or_default().to_owned();
            for rule in &plan.ingress {
                let mut perm = IpPermission::builder()
                    .ip_protocol("tcp")
                    .from_port(i32::from(rule.from))
                    .to_port(i32::from(rule.to));
                for cidr in &rule.cidrs {
                    perm = perm.ip_ranges(
                        IpRange::builder()
                            .cidr_ip(cidr)
                            .description(&rule.why)
                            .build(),
                    );
                }
                match ec2
                    .authorize_security_group_ingress()
                    .group_id(&made.group)
                    .ip_permissions(perm.build())
                    .send()
                    .await
                {
                    Ok(_) => {}
                    Err(e) if e.as_service_error().is_some_and(duplicate) => {}
                    Err(e) => return Err(err("open the share edge's ports", e)),
                }
            }
            let (image, _) = self.ubuntu(ec2, &plan.arch).await?;
            use base64::Engine as _;
            let user_data = base64::engine::general_purpose::STANDARD.encode(&plan.user_data);
            let run = ec2
                .run_instances()
                .image_id(image)
                .instance_type(aws_sdk_ec2::types::InstanceType::from(
                    plan.machine_type.as_str(),
                ))
                .min_count(1)
                .max_count(1)
                .subnet_id(&subnet)
                .security_group_ids(&made.group)
                .user_data(user_data)
                .metadata_options(
                    aws_sdk_ec2::types::InstanceMetadataOptionsRequest::builder()
                        .http_tokens(aws_sdk_ec2::types::HttpTokensState::Required)
                        .http_endpoint(aws_sdk_ec2::types::InstanceMetadataEndpointState::Enabled)
                        .build(),
                )
                .block_device_mappings(
                    aws_sdk_ec2::types::BlockDeviceMapping::builder()
                        .device_name("/dev/sda1")
                        .ebs(
                            aws_sdk_ec2::types::EbsBlockDevice::builder()
                                .volume_size(plan.disk_gb as i32)
                                .volume_type(aws_sdk_ec2::types::VolumeType::Gp3)
                                .delete_on_termination(true)
                                .encrypted(true)
                                .build(),
                        )
                        .build(),
                )
                .tag_specifications(spec(ResourceType::Instance, &plan.tags))
                .tag_specifications(spec(ResourceType::Volume, &plan.tags))
                .send()
                .await
                .map_err(|e| err("start the share edge", e))?;
            made.instance = run
                .instances()
                .first()
                .and_then(|i| i.instance_id())
                .unwrap_or_default()
                .to_owned();
            let eip = ec2
                .allocate_address()
                .domain(aws_sdk_ec2::types::DomainType::Vpc)
                .tag_specifications(spec(ResourceType::ElasticIp, &plan.tags))
                .send()
                .await
                .map_err(|e| err("reserve the share edge's address", e))?;
            made.address_id = eip.allocation_id().unwrap_or_default().to_owned();
            made.ip = eip.public_ip().unwrap_or_default().to_owned();
            // The instance must be running before the address attaches.
            for _ in 0..60 {
                let d = ec2
                    .describe_instances()
                    .instance_ids(&made.instance)
                    .send()
                    .await
                    .map_err(|e| err("wait for the share edge", e))?;
                let running = d
                    .reservations()
                    .iter()
                    .flat_map(|r| r.instances())
                    .any(|i| {
                        i.state().and_then(|s| s.name()).map(|n| n.as_str()) == Some("running")
                    });
                if running {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            ec2.associate_address()
                .allocation_id(&made.address_id)
                .instance_id(&made.instance)
                .send()
                .await
                .map_err(|e| err("attach the share edge's address", e))?;
            made.host = format!(
                "ec2-{}.{}.compute.amazonaws.com",
                made.ip.replace('.', "-"),
                conn.region
            );
            Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(made),
            Err(e) => {
                let _ = self.remove_edge(conn, &made).await;
                Err(e)
            }
        }
    }

    async fn renew_edge(&self, conn: &Connection, edge: &EdgeDeployed, expires: u64) -> Result<()> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let ids: Vec<String> = [&edge.instance, &edge.group, &edge.address_id]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect();
        c.ec2
            .create_tags()
            .set_resources(Some(ids))
            .tags(
                Tag::builder()
                    .key(tag::EXPIRES)
                    .value(expires.to_string())
                    .build(),
            )
            .send()
            .await
            .map_err(|e| err("renew the share edge", e))?;
        Ok(())
    }

    async fn remove_edge(&self, conn: &Connection, edge: &EdgeDeployed) -> Result<()> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let ec2 = &c.ec2;
        if !edge.address_id.is_empty() {
            // Detach first; a missing association is fine.
            if let Ok(d) = ec2
                .describe_addresses()
                .allocation_ids(&edge.address_id)
                .send()
                .await
            {
                for a in d.addresses() {
                    if let Some(assoc) = a.association_id() {
                        let _ = ec2
                            .disassociate_address()
                            .association_id(assoc)
                            .send()
                            .await;
                    }
                }
            }
        }
        if !edge.instance.is_empty() {
            match ec2
                .terminate_instances()
                .instance_ids(&edge.instance)
                .send()
                .await
            {
                Ok(_) => {}
                Err(e) if e.as_service_error().is_some_and(gone) => {}
                Err(e) => return Err(err("remove the share edge", e)),
            }
            // The group can go only once the instance is gone.
            for _ in 0..60 {
                let d = ec2
                    .describe_instances()
                    .instance_ids(&edge.instance)
                    .send()
                    .await;
                let done = match d {
                    Ok(d) => d
                        .reservations()
                        .iter()
                        .flat_map(|r| r.instances())
                        .all(|i| {
                            i.state().and_then(|s| s.name()).map(|n| n.as_str())
                                == Some("terminated")
                        }),
                    Err(_) => true,
                };
                if done {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
        if !edge.address_id.is_empty() {
            match ec2
                .release_address()
                .allocation_id(&edge.address_id)
                .send()
                .await
            {
                Ok(_) => {}
                Err(e) if e.as_service_error().is_some_and(gone) => {}
                Err(e) => return Err(err("release the share edge's address", e)),
            }
        }
        if !edge.group.is_empty() {
            match ec2
                .delete_security_group()
                .group_id(&edge.group)
                .send()
                .await
            {
                Ok(_) => {}
                Err(e) if e.as_service_error().is_some_and(gone) => {}
                Err(e) => return Err(err("remove the share edge's firewall", e)),
            }
        }
        Ok(())
    }

    async fn machine_address(&self, conn: &Connection, instance: &str) -> Result<Option<String>> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let d = c
            .ec2
            .describe_instances()
            .instance_ids(instance)
            .send()
            .await
            .map_err(|e| err("read the computer's address", e))?;
        Ok(d.reservations()
            .iter()
            .flat_map(|r| r.instances())
            .find_map(|i| i.public_ip_address().map(str::to_owned)))
    }

    async fn list_edges(&self, conn: &Connection, owner: &str) -> Result<Vec<EdgeDeployed>> {
        let c = self.clients(conn, Purpose::Manage).await?;
        let ec2 = &c.ec2;
        let d = ec2
            .describe_instances()
            .set_filters(Some(owned(owner, ROLE_EDGE)))
            .send()
            .await
            .map_err(|e| err("list share edges", e))?;
        let addrs = ec2
            .describe_addresses()
            .set_filters(Some(owned(owner, ROLE_EDGE)))
            .send()
            .await
            .map_err(|e| err("list share edge addresses", e))?;
        let mut out = Vec::new();
        for i in d.reservations().iter().flat_map(|r| r.instances()) {
            if i.state().and_then(|s| s.name()).map(|n| n.as_str()) == Some("terminated") {
                continue;
            }
            let id = i.instance_id().unwrap_or_default().to_owned();
            let tags = tags_of(i.tags());
            let addr = addrs
                .addresses()
                .iter()
                .find(|a| a.instance_id() == Some(id.as_str()));
            let ip = addr
                .and_then(|a| a.public_ip())
                .unwrap_or_default()
                .to_owned();
            out.push(EdgeDeployed {
                provider: "aws".into(),
                region: conn.region.clone(),
                instance: id,
                group: i
                    .security_groups()
                    .first()
                    .and_then(|g| g.group_id())
                    .unwrap_or_default()
                    .to_owned(),
                address_id: addr
                    .and_then(|a| a.allocation_id())
                    .unwrap_or_default()
                    .to_owned(),
                host: if ip.is_empty() {
                    String::new()
                } else {
                    format!(
                        "ec2-{}.{}.compute.amazonaws.com",
                        ip.replace('.', "-"),
                        conn.region
                    )
                },
                ip,
                ports: crate::share_edge::EDGE_PORTS,
                expires: tags
                    .get(tag::EXPIRES)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            });
        }
        // An address left behind by a crash between allocate and run.
        for a in addrs.addresses() {
            if a.instance_id().is_none() {
                out.push(EdgeDeployed {
                    provider: "aws".into(),
                    region: conn.region.clone(),
                    instance: String::new(),
                    group: String::new(),
                    address_id: a.allocation_id().unwrap_or_default().to_owned(),
                    ip: a.public_ip().unwrap_or_default().to_owned(),
                    host: String::new(),
                    ports: crate::share_edge::EDGE_PORTS,
                    expires: 0,
                });
            }
        }
        Ok(out)
    }
}
