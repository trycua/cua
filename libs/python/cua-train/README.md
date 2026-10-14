# cua-train

Python client for the [Cua Cloud](https://run.cua.ai) API.

```bash
pip install cua-train
```

```python
from cua_train import TrainClient

client = TrainClient.from_key(
    client_id="ukey-...",      # from your Cua Cloud account
    client_secret="...",
)

# Control plane: claim a VM
http = client.get_async_httpx_client()
await http.post("/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/my-pool/osgymsandboxclaims", json={...})

# Data plane: reach a service the sandbox publishes
# (/api/svc/<namespace>/<sandbox>-<service>/<path>)
await http.get("/api/svc/my-pool/my-sandbox-web/health")
```
