use crate::{
    CreateTemplateRequest, CyclopsClient, HttpHeader, HttpRequest, SdkError, Template, routes,
    types::ResourceMetadata,
};
use serde::Serialize;
use std::sync::Arc;
use url::Url;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(serde::Deserialize)]
struct ResourceList<T> {
    items: Vec<T>,
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn create_template(
        self: Arc<Self>,
        request: CreateTemplateRequest,
    ) -> Result<Template, SdkError> {
        let collection_url = routes::template_collection(self.base_url(), &request.namespace)?;
        let template = Template {
            api_version: "osgym.cua.ai/v1alpha1".into(),
            kind: "OSGymSandboxTemplate".into(),
            metadata: ResourceMetadata {
                namespace: request.namespace,
                name: request.name,
                labels: None,
                creation_timestamp: None,
            },
            spec: request.spec,
        };
        routes::template_item(
            self.base_url(),
            &template.metadata.namespace,
            &template.metadata.name,
        )?;
        self.send_json_crud(
            "create template",
            json_request("POST", collection_url, Some(to_json(&template)?)),
            &[200, 201, 202],
        )
        .await
    }

    pub async fn list_templates(
        self: Arc<Self>,
        namespace: String,
    ) -> Result<Vec<Template>, SdkError> {
        let collection_url = routes::template_collection(self.base_url(), &namespace)?;
        let list: ResourceList<Template> = self
            .send_json_crud(
                "list templates",
                json_request("GET", collection_url, None),
                &[200],
            )
            .await?;
        Ok(list.items)
    }

    pub async fn get_template(
        self: Arc<Self>,
        namespace: String,
        name: String,
    ) -> Result<Template, SdkError> {
        let item_url = routes::template_item(self.base_url(), &namespace, &name)?;
        self.send_json_crud("get template", json_request("GET", item_url, None), &[200])
            .await
    }

    pub async fn reconcile_template(
        self: Arc<Self>,
        request: CreateTemplateRequest,
    ) -> Result<Template, SdkError> {
        match self
            .clone()
            .get_template(request.namespace.clone(), request.name.clone())
            .await
        {
            Ok(mut template) => {
                template.spec = request.spec;
                self.update_template(template).await
            }
            Err(SdkError::Status {
                status: 403 | 404, ..
            }) => self.create_template(request).await,
            Err(error) => Err(error),
        }
    }

    pub async fn update_template(
        self: Arc<Self>,
        template: Template,
    ) -> Result<Template, SdkError> {
        let item_url = routes::template_item(
            self.base_url(),
            &template.metadata.namespace,
            &template.metadata.name,
        )?;
        let body = template_merge_patch_json(&template)?;
        self.send_json_crud(
            "update template",
            merge_patch_request(item_url, Some(body)),
            &[200],
        )
        .await
    }

    pub async fn delete_template(self: Arc<Self>, template: Template) -> Result<(), SdkError> {
        let item_url = routes::template_item(
            self.base_url(),
            &template.metadata.namespace,
            &template.metadata.name,
        )?;
        self.send_unit_crud(
            "delete template",
            json_request("DELETE", item_url, None),
            &[200, 202, 204, 404],
        )
        .await
    }
}

/// Serialize a template as a JSON merge patch that says what the caller meant.
///
/// `reconcile_template` overwrites `template.spec` with the caller's complete
/// desired spec, but a merge patch only touches the keys it mentions, and
/// `imagePullSecret` is `skip_serializing_if = "Option::is_none"`. A desired
/// spec with no pull secret therefore left a pull secret already stored on the
/// template in place -- the patch could not express "no secret".
///
/// Writing the absence out as an explicit null makes the patch mean what the
/// caller asked for. It is also what the gateway's pool admission policy
/// requires: a patch that carries `containerDiskImage` is refused unless it
/// also states the pull secret, so every reconcile of an existing template
/// backed by a registry image was rejected with 403 "k8s request is not
/// allowed" (trycua/cua#3159).
fn template_merge_patch_json(template: &Template) -> Result<Vec<u8>, SdkError> {
    let mut value = serde_json::to_value(template).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })?;
    if template.spec.vm_template.image_pull_secret.is_none() {
        value["spec"]["vmTemplate"]["imagePullSecret"] = serde_json::Value::Null;
    }
    to_json(&value)
}

fn merge_patch_request(url: Url, body: Option<Vec<u8>>) -> HttpRequest {
    let mut request = json_request("PATCH", url, body);
    request.headers[1].value = "application/merge-patch+json".into();
    request
}

fn json_request(method: &str, url: Url, body: Option<Vec<u8>>) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![
            HttpHeader {
                name: "accept".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
            HttpHeader {
                name: "content-type".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
        ],
        body,
        timeout_secs: None,
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SdkError> {
    serde_json::to_vec(value).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}
