# cua-fleet

Platform wheels for the Cua Fleet UniFFI SDK.

```bash
pip install cua-fleet
```

The distribution exposes the generated `fleet_sdk` module directly:

```python
from fleet_sdk import CyclopsClient, CyclopsConfiguration, CyclopsCredentials

client = CyclopsClient(
    CyclopsConfiguration(
        api_base="https://api.cua.ai",
        credentials=CyclopsCredentials(
            client_id="ukey-...",
            client_secret="...",
        ),
    )
)
```

There is no `cua_fleet` compatibility module. Import all generated Fleet SDK
symbols from `fleet_sdk`.
