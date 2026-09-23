//! Client-side fleet grouping over per-pool claims.
//!
//! The platform has no fleet-of-N object: each namespace holds exactly one
//! warm pool (namespace == pool name) and there is no cluster-wide claim
//! list. A "fleet" is therefore purely a shared label — `cua.ai/fleet=<id>` —
//! stamped on every claim created for the group, and membership is recovered
//! by listing a namespace's claims and filtering on that label. No CRD, no
//! server-side state.

use crate::{Claim, CreateClaimRequest, CyclopsClient, SdkError, routes};
use std::{collections::HashMap, sync::Arc};

/// The label key that groups a fleet's claims. Guest and app code that needs
/// to correlate sandboxes to a fleet must use this exact key.
pub const FLEET_LABEL_KEY: &str = "cua.ai/fleet";

/// Total claims a single fan-out may create, across all requested pools.
/// Guards against a typo'd replica count turning into an unbounded burst of
/// sequential claim creations.
pub const MAX_FLEET_CLAIMS: u32 = 256;

/// One pool's share of a fleet: claim `replicas` sandboxes from the warm pool
/// named `pool`. On this platform the pool name is also its namespace.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FleetPoolRequest {
    pub pool: String,
    pub replicas: u32,
}

/// A fleet's identity plus the claims currently known to belong to it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct FleetClaims {
    pub fleet_id: String,
    pub claims: Vec<Claim>,
}

/// The label key a fleet's claims share, for callers that filter or clean up
/// with raw Kubernetes tooling instead of `list_fleet_claims`.
#[uniffi::export]
pub fn fleet_label_key() -> String {
    FLEET_LABEL_KEY.into()
}

#[uniffi::export]
impl CyclopsClient {
    /// Fan out `create_claim` calls across the requested warm pools, tagging
    /// every claim with `cua.ai/fleet=<fleet_id>` so the group can be listed
    /// back later. Duplicate pool entries are aggregated before any network
    /// call. Claims are created sequentially; if one creation fails the error
    /// is returned immediately and claims already created keep their fleet
    /// label, so `list_fleet_claims` still finds them for retry or cleanup.
    pub async fn create_fleet_claims(
        self: Arc<Self>,
        fleet_id: String,
        requests: Vec<FleetPoolRequest>,
    ) -> Result<FleetClaims, SdkError> {
        let labels = fleet_claim_labels(&fleet_id)?;
        let plan = aggregate_fleet_requests(&requests)?;
        let mut claims = Vec::new();
        for (pool_name, replicas) in plan {
            let pool = Arc::clone(&self).get_pool(pool_name).await?;
            for _ in 0..replicas {
                let claim = Arc::clone(&self)
                    .create_claim(CreateClaimRequest {
                        pool: pool.clone(),
                        spec: None,
                        name: None,
                        labels: Some(labels.clone()),
                    })
                    .await?;
                claims.push(claim);
            }
        }
        Ok(FleetClaims { fleet_id, claims })
    }

    /// The fleet's claims within one namespace: enumerate the namespace's
    /// claims and keep those labeled `cua.ai/fleet=<fleet_id>`. A fleet that
    /// spans several pools spans that many namespaces (one pool per
    /// namespace), so call this once per member pool.
    pub async fn list_fleet_claims(
        self: Arc<Self>,
        namespace: String,
        fleet_id: String,
    ) -> Result<FleetClaims, SdkError> {
        validate_fleet_id(&fleet_id)?;
        let claims = Arc::clone(&self).list_claims(namespace).await?;
        Ok(FleetClaims {
            claims: filter_fleet_claims(claims, &fleet_id),
            fleet_id,
        })
    }
}

fn validate_fleet_id(fleet_id: &str) -> Result<(), SdkError> {
    routes::validate_dns_label_for("fleet id", fleet_id)
}

fn fleet_claim_labels(fleet_id: &str) -> Result<HashMap<String, String>, SdkError> {
    validate_fleet_id(fleet_id)?;
    Ok(HashMap::from([(
        FLEET_LABEL_KEY.to_owned(),
        fleet_id.to_owned(),
    )]))
}

/// Collapse the request set into one `(pool, replicas)` entry per pool in
/// first-seen order, validating pool names and replica counts up front so a
/// bad request fails before any claim is created.
fn aggregate_fleet_requests(requests: &[FleetPoolRequest]) -> Result<Vec<(String, u32)>, SdkError> {
    if requests.is_empty() {
        return Err(SdkError::Configuration {
            reason: "fleet must request at least one claim".into(),
        });
    }

    let mut plan: Vec<(String, u32)> = Vec::new();
    let mut total: u32 = 0;
    for request in requests {
        routes::validate_dns_label_for("pool name", &request.pool)?;
        if request.replicas == 0 {
            return Err(SdkError::Configuration {
                reason: format!("pool {:?} requests zero replicas", request.pool),
            });
        }
        total = total
            .checked_add(request.replicas)
            .filter(|total| *total <= MAX_FLEET_CLAIMS)
            .ok_or_else(|| SdkError::Configuration {
                reason: format!("fleet requests more than {MAX_FLEET_CLAIMS} claims"),
            })?;
        match plan.iter_mut().find(|(pool, _)| pool == &request.pool) {
            Some((_, replicas)) => *replicas += request.replicas,
            None => plan.push((request.pool.clone(), request.replicas)),
        }
    }
    Ok(plan)
}

fn claim_in_fleet(claim: &Claim, fleet_id: &str) -> bool {
    claim
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(FLEET_LABEL_KEY))
        .is_some_and(|value| value == fleet_id)
}

fn filter_fleet_claims(claims: Vec<Claim>, fleet_id: &str) -> Vec<Claim> {
    claims
        .into_iter()
        .filter(|claim| claim_in_fleet(claim, fleet_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        FLEET_LABEL_KEY, FleetPoolRequest, MAX_FLEET_CLAIMS, aggregate_fleet_requests,
        claim_in_fleet, filter_fleet_claims, fleet_claim_labels, fleet_label_key,
    };
    use crate::{Claim, ResourceMetadata, SdkError};
    use cyclops_sdk_schema::{ClaimSpec, SandboxTemplateRef};
    use std::collections::HashMap;

    fn request(pool: &str, replicas: u32) -> FleetPoolRequest {
        FleetPoolRequest {
            pool: pool.into(),
            replicas,
        }
    }

    fn claim(name: &str, labels: Option<HashMap<String, String>>) -> Claim {
        Claim {
            api_version: "osgym.cua.ai/v1alpha1".into(),
            kind: "OSGymSandboxClaim".into(),
            metadata: ResourceMetadata {
                namespace: "spaces-linux".into(),
                name: name.into(),
                labels,
                creation_timestamp: None,
            },
            spec: ClaimSpec {
                sandbox_template_ref: SandboxTemplateRef {
                    name: "spaces-linux-template".into(),
                },
                warmpool: None,
                bind_deadline: None,
                ttl_seconds_after_created: None,
                lifecycle: None,
            },
            status: None,
        }
    }

    #[test]
    fn exported_label_key_matches_constant() {
        assert_eq!(fleet_label_key(), FLEET_LABEL_KEY);
        assert_eq!(fleet_label_key(), "cua.ai/fleet");
    }

    #[test]
    fn labels_carry_the_fleet_id_under_the_shared_key() {
        assert_eq!(
            fleet_claim_labels("space-42").unwrap(),
            HashMap::from([("cua.ai/fleet".to_owned(), "space-42".to_owned())])
        );
    }

    #[test]
    fn labels_reject_non_dns_label_fleet_ids() {
        for fleet_id in ["", "UPPER", "has spaces", "-leading", "trailing-", "a/b"] {
            assert!(
                matches!(
                    fleet_claim_labels(fleet_id),
                    Err(SdkError::InvalidResourceName { .. })
                ),
                "expected {fleet_id:?} to be rejected"
            );
        }
    }

    #[test]
    fn aggregation_sums_duplicates_in_first_seen_order() {
        let plan = aggregate_fleet_requests(&[
            request("spaces-linux", 2),
            request("other-pool", 1),
            request("spaces-linux", 3),
        ])
        .unwrap();

        assert_eq!(
            plan,
            vec![("spaces-linux".into(), 5), ("other-pool".into(), 1)]
        );
    }

    #[test]
    fn aggregation_rejects_empty_zero_replica_and_invalid_pools() {
        assert!(matches!(
            aggregate_fleet_requests(&[]),
            Err(SdkError::Configuration { .. })
        ));
        assert!(matches!(
            aggregate_fleet_requests(&[request("spaces-linux", 0)]),
            Err(SdkError::Configuration { .. })
        ));
        assert!(matches!(
            aggregate_fleet_requests(&[request("Not-A-Label", 1)]),
            Err(SdkError::InvalidResourceName { .. })
        ));
    }

    #[test]
    fn aggregation_caps_total_replicas() {
        assert!(matches!(
            aggregate_fleet_requests(&[request("spaces-linux", MAX_FLEET_CLAIMS + 1)]),
            Err(SdkError::Configuration { .. })
        ));
        assert!(matches!(
            aggregate_fleet_requests(&[
                request("spaces-linux", MAX_FLEET_CLAIMS),
                request("other-pool", u32::MAX),
            ]),
            Err(SdkError::Configuration { .. })
        ));
        assert!(aggregate_fleet_requests(&[request("spaces-linux", MAX_FLEET_CLAIMS)]).is_ok());
    }

    #[test]
    fn filtering_keeps_only_claims_labeled_with_the_fleet_id() {
        let mine = claim(
            "claim-mine",
            Some(HashMap::from([(
                FLEET_LABEL_KEY.to_owned(),
                "space-42".to_owned(),
            )])),
        );
        let other_fleet = claim(
            "claim-other",
            Some(HashMap::from([(
                FLEET_LABEL_KEY.to_owned(),
                "space-99".to_owned(),
            )])),
        );
        let unrelated_label = claim(
            "claim-unrelated",
            Some(HashMap::from([("team".to_owned(), "space-42".to_owned())])),
        );
        let unlabeled = claim("claim-unlabeled", None);

        assert!(claim_in_fleet(&mine, "space-42"));
        assert!(!claim_in_fleet(&other_fleet, "space-42"));
        assert!(!claim_in_fleet(&unrelated_label, "space-42"));
        assert!(!claim_in_fleet(&unlabeled, "space-42"));

        let kept = filter_fleet_claims(
            vec![mine.clone(), other_fleet, unrelated_label, unlabeled],
            "space-42",
        );
        assert_eq!(kept, vec![mine]);
    }
}
