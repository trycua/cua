use cyclops_sdk_core::{
    AccessToken, Claim, Configuration, CreateClaimRequest, CreatePoolRequest, HttpRequest,
    HttpResponse, OAuthCredentials, Platform, PlatformError, Pool, Sandbox, Sdk,
    ServiceHttpRequest,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Configure {
        configuration: Configuration,
    },
    CreatePool {
        request: Box<CreatePoolRequest>,
    },
    ListPools {
        namespace: String,
    },
    GetPool {
        pool: Pool,
    },
    UpdatePool {
        pool: Pool,
    },
    DeletePool {
        pool: Pool,
    },
    CreateClaim {
        request: Box<CreateClaimRequest>,
    },
    ListClaims {
        namespace: String,
    },
    GetClaim {
        claim: Claim,
    },
    UpdateClaim {
        claim: Claim,
    },
    DeleteClaim {
        claim: Claim,
    },
    WaitClaim {
        claim: Claim,
    },
    #[serde(rename = "service_request")]
    ServiceHttp {
        sandbox: Sandbox,
        service: String,
        request: ServiceHttpRequest,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HostCall<'a> {
    Http { request: HttpRequest },
    Sleep { milliseconds: u64 },
    NowMs,
    AcquireOauthCredentials,
    LoadAccessToken,
    StoreAccessToken { token: &'a AccessToken },
}

#[derive(Deserialize)]
struct HostReply {
    ok: bool,
    value: serde_json::Value,
    error: Option<PlatformError>,
}
#[derive(Serialize)]
struct Reply<T> {
    ok: bool,
    value: Option<T>,
    error: Option<String>,
}
struct RpcPlatform<R, W> {
    input: R,
    output: W,
}

impl<R: BufRead, W: Write> RpcPlatform<R, W> {
    fn read_request(&mut self) -> Result<Option<Request>, String> {
        let mut line = String::new();
        if self
            .input
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Ok(None);
        }
        serde_json::from_str(&line)
            .map(Some)
            .map_err(|error| error.to_string())
    }
    fn write_reply<T: Serialize>(&mut self, reply: &Reply<T>) -> Result<(), String> {
        serde_json::to_writer(&mut self.output, reply).map_err(|error| error.to_string())?;
        writeln!(self.output).map_err(|error| error.to_string())?;
        self.output.flush().map_err(|error| error.to_string())
    }
    fn call<T: DeserializeOwned>(&mut self, call: HostCall<'_>) -> Result<T, PlatformError> {
        serde_json::to_writer(&mut self.output, &call)
            .map_err(|error| PlatformError::Transport(error.to_string()))?;
        writeln!(self.output).map_err(|error| PlatformError::Transport(error.to_string()))?;
        self.output
            .flush()
            .map_err(|error| PlatformError::Transport(error.to_string()))?;
        let mut line = String::new();
        self.input
            .read_line(&mut line)
            .map_err(|error| PlatformError::Transport(error.to_string()))?;
        if line.is_empty() {
            return Err(PlatformError::Transport("host adapter exited".into()));
        }
        let reply: HostReply = serde_json::from_str(&line)
            .map_err(|error| PlatformError::Transport(format!("invalid host reply: {error}")))?;
        if reply.ok {
            serde_json::from_value(reply.value).map_err(|error| {
                PlatformError::Transport(format!("invalid host reply value: {error}"))
            })
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| PlatformError::Transport("host reply omitted error".into())))
        }
    }
    fn call_unit(&mut self, call: HostCall<'_>) -> Result<(), PlatformError> {
        let _: serde_json::Value = self.call(call)?;
        Ok(())
    }
}

impl<R: BufRead, W: Write> Platform for RpcPlatform<R, W> {
    fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
        self.call(HostCall::Http { request })
    }
    fn sleep(&mut self, milliseconds: u64) -> Result<(), PlatformError> {
        self.call_unit(HostCall::Sleep { milliseconds })
    }
    fn now_ms(&mut self) -> u64 {
        self.call(HostCall::NowMs).unwrap_or(0)
    }
    fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError> {
        self.call(HostCall::AcquireOauthCredentials)
    }
    fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError> {
        self.call(HostCall::LoadAccessToken)
    }
    fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError> {
        self.call_unit(HostCall::StoreAccessToken { token })
    }
}

fn reply<T: Serialize>(result: Result<T, cyclops_sdk_core::SdkError>) -> Reply<T> {
    match result {
        Ok(value) => Reply {
            ok: true,
            value: Some(value),
            error: None,
        },
        Err(error) => Reply {
            ok: false,
            value: None,
            error: Some(error.to_string()),
        },
    }
}

fn main() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut platform = RpcPlatform {
        input: stdin.lock(),
        output: stdout.lock(),
    };
    let configuration = match platform.read_request()? {
        Some(Request::Configure { configuration }) => configuration,
        Some(_) => return Err("first request must configure the SDK".into()),
        None => return Ok(()),
    };
    let mut sdk = Sdk::connect(platform, configuration).map_err(|error| error.to_string())?;
    sdk.platform_mut().write_reply(&Reply {
        ok: true,
        value: Some(()),
        error: None,
    })?;
    while let Some(request) = sdk.platform_mut().read_request()? {
        let response = match request {
            Request::Configure { .. } => serde_json::to_value(Reply::<()> {
                ok: false,
                value: None,
                error: Some("SDK is already configured".into()),
            }),
            Request::CreatePool { request } => {
                serde_json::to_value(reply(sdk.create_pool(*request)))
            }
            Request::ListPools { namespace } => {
                serde_json::to_value(reply(sdk.list_pools(&namespace)))
            }
            Request::GetPool { pool } => serde_json::to_value(reply(sdk.get_pool(pool))),
            Request::UpdatePool { pool } => serde_json::to_value(reply(sdk.update_pool(pool))),
            Request::DeletePool { pool } => serde_json::to_value(reply(sdk.delete_pool(pool))),
            Request::CreateClaim { request } => {
                serde_json::to_value(reply(sdk.create_claim(*request)))
            }
            Request::ListClaims { namespace } => {
                serde_json::to_value(reply(sdk.list_claims(&namespace)))
            }
            Request::GetClaim { claim } => serde_json::to_value(reply(sdk.get_claim(claim))),
            Request::UpdateClaim { claim } => serde_json::to_value(reply(sdk.update_claim(claim))),
            Request::DeleteClaim { claim } => serde_json::to_value(reply(sdk.delete_claim(claim))),
            Request::WaitClaim { claim } => serde_json::to_value(reply(sdk.wait_claim(claim))),
            Request::ServiceHttp {
                sandbox,
                service,
                request,
            } => serde_json::to_value(reply(sdk.service_request(&sandbox, &service, request))),
        }
        .map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut sdk.platform_mut().output, &response)
            .map_err(|error| error.to_string())?;
        writeln!(sdk.platform_mut().output).map_err(|error| error.to_string())?;
        sdk.platform_mut()
            .output
            .flush()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
