use crate::SdkError;
use url::Url;

const POOL_COLLECTION_PREFIX: &str = "api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/";
const CLAIM_COLLECTION_PREFIX: &str = "api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/";
const CLAIM_COLLECTION_SUFFIX: &str = "/osgymsandboxclaims";
const POOL_COLLECTION_SUFFIX: &str = "/osgymsandboxwarmpools";
const TEMPLATE_COLLECTION_SUFFIX: &str = "/osgymsandboxtemplates";
const NAMESPACE_COLLECTION: &str = "api/namespaces";
const NAMESPACE_PREFIX: &str = "api/namespaces/";
const SERVICE_COLLECTION_PREFIX: &str = "api/svc/";
const SIGNED_SERVICE_URL_COLLECTION_PREFIX: &str = "api/signed-service-urls/";
const USER_KEY_COLLECTION: &str = "api/user-keys";

pub fn pool_collection(base: &Url, namespace: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    route(
        base,
        format!("{POOL_COLLECTION_PREFIX}{namespace}{POOL_COLLECTION_SUFFIX}"),
    )
}

pub fn pool_item(base: &Url, namespace: &str, name: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    validate_dns_label_for("name", name)?;
    route(
        base,
        format!("{POOL_COLLECTION_PREFIX}{namespace}{POOL_COLLECTION_SUFFIX}/{name}"),
    )
}

pub fn named_pool_item(base: &Url, name: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("name", name)?;
    route(
        base,
        format!("{POOL_COLLECTION_PREFIX}{name}{POOL_COLLECTION_SUFFIX}/{name}"),
    )
}

pub fn template_collection(base: &Url, namespace: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    route(
        base,
        format!("{POOL_COLLECTION_PREFIX}{namespace}{TEMPLATE_COLLECTION_SUFFIX}"),
    )
}

pub fn template_item(base: &Url, namespace: &str, name: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    validate_dns_label_for("name", name)?;
    route(
        base,
        format!("{POOL_COLLECTION_PREFIX}{namespace}{TEMPLATE_COLLECTION_SUFFIX}/{name}"),
    )
}

pub fn namespace_collection(base: &Url) -> Result<Url, SdkError> {
    route(base, NAMESPACE_COLLECTION.into())
}

pub fn namespace_item(base: &Url, namespace: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    route(base, format!("{NAMESPACE_PREFIX}{namespace}"))
}

pub fn signed_service_url_collection(base: &Url, namespace: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    route(
        base,
        format!("{SIGNED_SERVICE_URL_COLLECTION_PREFIX}{namespace}"),
    )
}

pub fn signed_service_url_list(base: &Url, namespace: &str, claim: &str) -> Result<Url, SdkError> {
    validate_signed_service_url_claim(claim)?;
    let mut url = signed_service_url_collection(base, namespace)?;
    url.query_pairs_mut().append_pair("claim", claim);
    Ok(url)
}

pub fn signed_service_url_item(base: &Url, namespace: &str, id: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    validate_signed_service_url_id(id)?;
    route(
        base,
        format!("{SIGNED_SERVICE_URL_COLLECTION_PREFIX}{namespace}/{id}"),
    )
}

pub(crate) fn validate_signed_service_url_claim(claim: &str) -> Result<(), SdkError> {
    let reason = if claim.is_empty() {
        Some("must not be empty")
    } else if claim.len() > 128 {
        Some("must be at most 128 bytes")
    } else if !claim
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
    {
        Some("must contain only ASCII letters, digits, periods, underscores, tildes, and hyphens")
    } else {
        None
    };

    match reason {
        Some(reason) => Err(SdkError::InvalidResourceName {
            field: "claim".into(),
            value: claim.into(),
            reason: reason.into(),
        }),
        None => Ok(()),
    }
}

fn validate_signed_service_url_id(id: &str) -> Result<(), SdkError> {
    let is_uuid = id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        });
    if is_uuid {
        return Ok(());
    }

    Err(SdkError::InvalidResourceName {
        field: "id".into(),
        value: id.into(),
        reason: "must be a UUID".into(),
    })
}

pub fn user_key_collection(base: &Url) -> Result<Url, SdkError> {
    route(base, USER_KEY_COLLECTION.into())
}

pub fn user_key_item(base: &Url, id: &str) -> Result<Url, SdkError> {
    let mut url = user_key_collection(base)?;
    url.path_segments_mut()
        .map_err(|_| SdkError::Configuration {
            reason: "base_url cannot be used for user API key routes".into(),
        })?
        .push(id);
    Ok(url)
}

pub fn validate_dns_label(value: &str) -> Result<(), SdkError> {
    validate_dns_label_for("resource name", value)
}

pub(crate) fn validate_dns_label_for(field: &str, value: &str) -> Result<(), SdkError> {
    let reason = if value.is_empty() {
        Some("must not be empty")
    } else if value.len() > 63 {
        Some("must be at most 63 bytes")
    } else if value.starts_with('-') {
        Some("must not start with a hyphen")
    } else if value.ends_with('-') {
        Some("must not end with a hyphen")
    } else if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Some("must contain only lowercase ASCII letters, digits, and hyphens")
    } else {
        None
    };

    match reason {
        Some(reason) => Err(SdkError::InvalidResourceName {
            field: field.into(),
            value: value.into(),
            reason: reason.into(),
        }),
        None => Ok(()),
    }
}

fn route(base: &Url, suffix: String) -> Result<Url, SdkError> {
    let prefix = base.path().trim_end_matches('/');
    let path = if prefix.is_empty() {
        format!("/{suffix}")
    } else {
        format!("{prefix}/{suffix}")
    };
    let mut url = base.clone();
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub fn claim_collection(base: &Url, namespace: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    route(
        base,
        format!("{CLAIM_COLLECTION_PREFIX}{namespace}{CLAIM_COLLECTION_SUFFIX}"),
    )
}

pub fn claim_item(base: &Url, namespace: &str, name: &str) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    validate_dns_label_for("name", name)?;
    route(
        base,
        format!("{CLAIM_COLLECTION_PREFIX}{namespace}{CLAIM_COLLECTION_SUFFIX}/{name}"),
    )
}

pub fn service_url(
    base: &Url,
    namespace: &str,
    service_name: &str,
    path: &str,
) -> Result<Url, SdkError> {
    validate_dns_label_for("namespace", namespace)?;
    validate_dns_label_for("service", service_name)?;

    let (path, query) = validate_service_path(path)?;
    let mut url = route(
        base,
        format!("{SERVICE_COLLECTION_PREFIX}{namespace}/{service_name}{path}"),
    )?;
    url.set_query(query);
    Ok(url)
}

const MAX_PERCENT_DECODE_PASSES: usize = 8;

fn validate_service_path(path: &str) -> Result<(&str, Option<&str>), SdkError> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains('#') || path.contains('\\')
    {
        return Err(SdkError::InvalidServicePath { path: path.into() });
    }

    let (path_only, query) = path
        .split_once('?')
        .map_or((path, None), |(path, query)| (path, Some(query)));
    if !is_safe_service_component(path_only, true)
        || query.is_some_and(|query| !is_safe_service_component(query, false))
    {
        return Err(SdkError::InvalidServicePath { path: path.into() });
    }

    Ok((path_only, query))
}

fn is_safe_service_component(component: &str, is_path: bool) -> bool {
    let mut bytes = component.as_bytes().to_vec();
    for _ in 0..MAX_PERCENT_DECODE_PASSES {
        if contains_ascii_control(&bytes) || (is_path && has_ambiguous_path_shape(&bytes)) {
            return false;
        }

        let Some((decoded, had_percent_escape)) = decode_percent_escapes(&bytes, is_path) else {
            return false;
        };
        if !had_percent_escape {
            return true;
        }
        bytes = decoded;
    }

    false
}

fn contains_ascii_control(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| *byte <= 0x1f || *byte == 0x7f)
}

fn has_ambiguous_path_shape(bytes: &[u8]) -> bool {
    if bytes
        .split(|byte| *byte == b'/')
        .any(|segment| matches!(segment, b"." | b".."))
    {
        return true;
    }

    bytes.first() == Some(&b'/')
        && std::str::from_utf8(&bytes[1..]).is_ok_and(|path| Url::parse(path).is_ok())
}

fn decode_percent_escapes(bytes: &[u8], is_path: bool) -> Option<(Vec<u8>, bool)> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut had_percent_escape = false;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let value = percent_escape_value(bytes.get(index + 1)?, bytes.get(index + 2)?)?;
        if value <= 0x1f || value == 0x7f || (is_path && matches!(value, b'/' | b'\\')) {
            return None;
        }
        decoded.push(value);
        had_percent_escape = true;
        index += 3;
    }

    Some((decoded, had_percent_escape))
}

fn percent_escape_value(high: &u8, low: &u8) -> Option<u8> {
    Some(hex_value(*high)? << 4 | hex_value(*low)?)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        claim_collection, claim_item, namespace_collection, namespace_item, pool_collection,
        pool_item, signed_service_url_collection, signed_service_url_item, signed_service_url_list,
        template_collection, template_item,
    };
    use url::Url;

    #[test]
    fn signed_service_url_routes_validate_and_append_paths() {
        let base = Url::parse("https://cyclops.example:8443/").unwrap();
        let id = "31e1c9bb-8cc9-4c50-9cf4-51798b6978e4";

        assert_eq!(
            signed_service_url_collection(&base, "tenant-a")
                .unwrap()
                .as_str(),
            "https://cyclops.example:8443/api/signed-service-urls/tenant-a"
        );
        assert_eq!(
            signed_service_url_list(&base, "tenant-a", "claim-a")
                .unwrap()
                .as_str(),
            "https://cyclops.example:8443/api/signed-service-urls/tenant-a?claim=claim-a"
        );
        assert_eq!(
            signed_service_url_item(&base, "tenant-a", id)
                .unwrap()
                .as_str(),
            format!("https://cyclops.example:8443/api/signed-service-urls/tenant-a/{id}")
        );
        assert!(signed_service_url_collection(&base, "Tenant-A").is_err());
        assert!(signed_service_url_list(&base, "tenant-a", "bad claim").is_err());
        assert!(signed_service_url_item(&base, "tenant-a", "bad-id").is_err());
    }

    #[test]
    fn routes_append_to_root_base_without_double_slashes() {
        let base = Url::parse("https://cyclops.example:8443/").unwrap();

        assert_eq!(
            pool_collection(&base, "example-pool").unwrap().as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxwarmpools"
        );
        assert_eq!(
            pool_item(&base, "example-pool", "example-pool")
                .unwrap()
                .as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxwarmpools/example-pool"
        );
        assert_eq!(
            namespace_collection(&base).unwrap().as_str(),
            "https://cyclops.example:8443/api/namespaces"
        );
        assert_eq!(
            namespace_item(&base, "example-pool").unwrap().as_str(),
            "https://cyclops.example:8443/api/namespaces/example-pool"
        );
        assert_eq!(
            claim_collection(&base, "example-pool").unwrap().as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxclaims"
        );
        assert_eq!(
            claim_item(&base, "example-pool", "example-claim")
                .unwrap()
                .as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxclaims/example-claim"
        );
        assert_eq!(
            template_collection(&base, "example-pool").unwrap().as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxtemplates"
        );
        assert_eq!(
            template_item(&base, "example-pool", "example-template")
                .unwrap()
                .as_str(),
            "https://cyclops.example:8443/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxtemplates/example-template"
        );
    }

    #[test]
    fn routes_preserve_base_path_prefix_without_double_slashes() {
        let base = Url::parse("https://gateway.example/cyclops/").unwrap();

        assert_eq!(
            pool_collection(&base, "example-pool").unwrap().as_str(),
            "https://gateway.example/cyclops/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxwarmpools"
        );
        assert_eq!(
            pool_item(&base, "example-pool", "example-pool")
                .unwrap()
                .as_str(),
            "https://gateway.example/cyclops/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxwarmpools/example-pool"
        );
        assert_eq!(
            namespace_collection(&base).unwrap().as_str(),
            "https://gateway.example/cyclops/api/namespaces"
        );
        assert_eq!(
            namespace_item(&base, "example-pool").unwrap().as_str(),
            "https://gateway.example/cyclops/api/namespaces/example-pool"
        );
    }
}
