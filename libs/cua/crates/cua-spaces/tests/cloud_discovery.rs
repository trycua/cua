//! Post-provisioning discovery with a fake cloud provider, fake registry and
//! loopback relay. No cloud account, VM, container, or real resource is used.
use async_trait::async_trait;
use cua_host::testing::FakeRelay;
use cua_image::testing::FakeRegistry;
use cua_sandbox_core::{
    ImageMode, PortExposure, Provider, ProviderCapabilities, ProviderCreate, ProviderInstance,
    Result, RunKind, Sandboxes, ServiceEndpoint, Status,
};
use cua_spaces::{RelayAccount, SpaceCreate, Spaces};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const MACHINE: &str = "0123abcd4567ef89";
struct Tokens(Mutex<String>);
#[async_trait]
impl cua_host::AccountTokens for Tokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        Ok(self.0.lock().unwrap().clone())
    }
}
struct Cloud {
    tokens: Arc<Tokens>,
    fail_discovery: bool,
    creates: AtomicUsize,
    deletes: AtomicUsize,
    instance: Mutex<Option<ProviderInstance>>,
}
#[async_trait]
impl Provider for Cloud {
    fn name(&self) -> &'static str {
        "aws"
    }
    fn joins_relay(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            runtime: "fake",
            arches: vec!["amd64"],
            image_mode: ImageMode::Direct,
            ports: PortExposure::None,
            command: true,
            env_to_entrypoint: false,
            private_registry: false,
            suspend: false,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: &[],
            gpus: vec![],
        }
    }
    fn check_configured(&self) -> Result<()> {
        Ok(())
    }
    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        let mut instance = ProviderInstance::new("fake-instance", &spec.name, Status::Running);
        instance.details.insert(
            cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE.into(),
            MACHINE.into(),
        );
        *self.instance.lock().unwrap() = Some(instance.clone());
        if self.fail_discovery {
            *self.tokens.0.lock().unwrap() = "bad".into();
        }
        Ok(instance)
    }
    async fn get(&self, _: &str) -> Result<ProviderInstance> {
        Ok(self.instance.lock().unwrap().clone().unwrap())
    }
    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        Ok(self.instance.lock().unwrap().clone().into_iter().collect())
    }
    async fn delete(&self, _: &str) -> Result<()> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn endpoint(&self, _: &ProviderInstance, _: u16) -> Result<ServiceEndpoint> {
        panic!("no guest endpoint needed")
    }
}

#[tokio::test]
async fn created_cloud_resources_survive_discovery_failure_and_missing_details() {
    let mut registry = FakeRegistry::default();
    registry.index("ghcr.io/example/discovery:1", &["amd64"], false, None);
    cua_image::resolve::set_source(Some(Arc::new(registry)));
    // Warm failure, cold failure, and a successful-but-empty directory.
    for (warm, fail_discovery) in [(true, true), (false, true), (false, false)] {
        let relay = FakeRelay::start().await;
        relay.add_account("owner", "owner-id", None);
        if warm {
            cua_host::RelayClient::new(&relay.url)
                .unwrap()
                .register(
                    "owner",
                    &cua_host::relay::RegisterRequest {
                        id: MACHINE.into(),
                        name: "existing".into(),
                        allow: vec![],
                        host: None,
                        meta: Default::default(),
                    },
                )
                .await
                .unwrap();
        }
        let tokens = Arc::new(Tokens(Mutex::new("owner".into())));
        let cloud = Arc::new(Cloud {
            tokens: tokens.clone(),
            fail_discovery,
            creates: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            instance: Mutex::new(None),
        });
        let home = tempfile::tempdir().unwrap();
        let sandboxes = Sandboxes::builder()
            .provider(cloud.clone())
            .state_dir(home.path().join("sandboxes"))
            .build();
        let spaces = Spaces::builder()
            .home(home.path())
            .sandboxes(sandboxes)
            .relay(RelayAccount::new(&relay.url, tokens))
            .build();
        if warm {
            assert_eq!(spaces.list_all().await.unwrap().len(), 1);
        }
        let result = spaces
            .create(SpaceCreate {
                on: Some(cua_sandbox_core::placement::On::Provider("aws".into())),
                name: Some("created-once".into()),
                image: Some("ghcr.io/example/discovery:1".into()),
                ..Default::default()
            })
            .await;
        if warm {
            assert_eq!(
                result.unwrap().ready().unwrap().id,
                format!("relay:{MACHINE}")
            );
        } else {
            let error = match result {
                Err(e) => e,
                Ok(_) => panic!("missing details must be explicit"),
            };
            assert_eq!(
                error.to_string(),
                "relay: The cloud sandbox was created, but its Space details could not be read. Inspect the existing sandbox before retrying creation."
            );
        }
        assert_eq!(cloud.creates.load(Ordering::SeqCst), 1);
        assert_eq!(cloud.deletes.load(Ordering::SeqCst), 0);
        assert!(home.path().join("sandboxes/created-once.json").exists());
        assert!(
            spaces
                .sandboxes()
                .by_relay_machine(MACHINE)
                .unwrap()
                .is_some()
        );
    }
}
