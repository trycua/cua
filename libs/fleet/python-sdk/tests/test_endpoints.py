"""
Parametrized tests for every endpoint in the cyclops-cs SDK.

Uses pytest-httpx to intercept httpx calls so no real server is needed.
Each endpoint has three test categories:
  - base: happy-path / golden response
  - error: 4xx responses parsed into ErrorResponse
  - generated: edge-case parameter variants
"""

import pytest
from pytest_httpx import HTTPXMock

from cua_train import TrainClient
from cua_train.api.batch import cancel_batch, get_batch_results, get_batch_status, submit_batch
from cua_train.api.health import healthz
from cua_train.api.keys import create_key, delete_key, list_keys
from cua_train.api.label import cancel_label, get_label_results, get_label_status, submit_label_batch
from cua_train.client import AuthenticatedClient, Client
from cua_train.models import (
    Action,
    ActionParameters,
    BatchInProgressResponse,
    BatchResultsResponse,
    BatchStatusResponse,
    BatchSubmitRequest,
    BatchSubmitResponse,
    CancelBatchResponse,
    CancelLabelResponse,
    CreateKeyRequest,
    CreateKeyResponse,
    ErrorResponse,
    HealthResponse,
    LabelInProgressResponse,
    LabelResultsResponse,
    LabelStatusResponse,
    ListKeysResponse,
    RunConfig,
)

BASE_URL = "https://cyclops-cs.trycua.com"
TOKEN = "test-bearer-token"


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def anon_client() -> Client:
    return Client(base_url=BASE_URL, raise_on_unexpected_status=True)


@pytest.fixture
def auth_client() -> AuthenticatedClient:
    # TrainClient is a drop-in AuthenticatedClient that also exposes every
    # generated endpoint as a method; using it here exercises both the low-level
    # api.* calls (client=auth_client) and the generic facade (auth_client.<op>).
    return TrainClient(base_url=BASE_URL, token=TOKEN, raise_on_unexpected_status=True)


def test_train_client_refreshes_token(httpx_mock: HTTPXMock):
    """from_key mints a token, then get_httpx_client() transparently re-mints it
    once it's near expiry — no caller involvement."""
    token_ep = "https://auth.example/realms/x/token"
    # First exchange returns an already-stale token (expires_in=0).
    httpx_mock.add_response(method="POST", url=token_ep, json={"access_token": "t1", "expires_in": 0})
    c = TrainClient.from_key(token_url=token_ep, client_id="key-x", client_secret="s", base_url=BASE_URL)
    assert c.token == "t1"

    # Next use refreshes (deadline already passed) and bakes the new bearer.
    httpx_mock.add_response(method="POST", url=token_ep, json={"access_token": "t2", "expires_in": 300})
    client = c.get_httpx_client()
    assert c.token == "t2"
    assert client.headers["Authorization"] == "Bearer t2"


# ---------------------------------------------------------------------------
# /healthz
# ---------------------------------------------------------------------------


class TestHealthz:
    def test_base_ok(self, httpx_mock: HTTPXMock, anon_client):
        """Golden: healthz returns 200 {ok: true}."""
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/healthz",
            status_code=200,
            json={"ok": True},
        )
        resp = healthz.sync_detailed(client=anon_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, HealthResponse)
        assert resp.parsed.ok is True

    def test_base_degraded(self, httpx_mock: HTTPXMock, anon_client):
        """Edge: ok=false still parses cleanly."""
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/healthz",
            status_code=200,
            json={"ok": False},
        )
        assert healthz.sync_detailed(client=anon_client).parsed.ok is False

    def test_unexpected_status(self, httpx_mock: HTTPXMock, anon_client):
        """Error: unexpected 503 raises when raise_on_unexpected_status=True."""
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/healthz", status_code=503, json={"error": "svc unavailable"}
        )
        from cua_train import errors

        with pytest.raises(errors.UnexpectedStatus):
            healthz.sync_detailed(client=anon_client)


# ---------------------------------------------------------------------------
# /api/keys  GET
# ---------------------------------------------------------------------------


GOLDEN_KEY = {
    "id": "kc-uuid-1",
    "client_id": "key-abc123",
    "name": "ci-prod",
    "namespace": "test-pool",
    "owner_sub": "user-sub-1",
}


class TestListKeys:
    def test_base(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(method="GET", url=f"{BASE_URL}/api/keys", status_code=200, json={"keys": [GOLDEN_KEY]})
        resp = list_keys.sync_detailed(client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, ListKeysResponse)
        assert len(resp.parsed.keys) == 1
        assert resp.parsed.keys[0].name == "ci-prod"

    def test_empty_list(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(method="GET", url=f"{BASE_URL}/api/keys", status_code=200, json={"keys": []})
        assert list_keys.sync_detailed(client=auth_client).parsed.keys == []

    def test_unauthorized(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/keys", status_code=401, json={"error": "unauthorized"}
        )
        resp = list_keys.sync_detailed(client=auth_client)
        assert resp.status_code.value == 401
        assert isinstance(resp.parsed, ErrorResponse)


# ---------------------------------------------------------------------------
# /api/keys  POST
# ---------------------------------------------------------------------------


class TestCreateKey:
    @pytest.mark.parametrize(
        "name,namespace",
        [
            ("ci-prod", "test-pool"),
            ("my-key", "default"),
            ("key-with-dashes", "namespace-123"),
        ],
    )
    def test_base(self, httpx_mock: HTTPXMock, auth_client, name, namespace):
        httpx_mock.add_response(
            method="POST",
            url=f"{BASE_URL}/api/keys",
            status_code=201,
            json={
                "client_id": f"key-{name}",
                "client_secret": "super-secret",
                "name": name,
                "namespace": namespace,
                "token_url": "https://auth.example.com/token",
            },
        )
        resp = create_key.sync_detailed(client=auth_client, body=CreateKeyRequest(name=name, namespace=namespace))
        assert resp.status_code.value == 201
        assert isinstance(resp.parsed, CreateKeyResponse)
        assert resp.parsed.client_secret == "super-secret"

    def test_bad_request(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/keys", status_code=400, json={"error": "name is required"}
        )
        resp = create_key.sync_detailed(client=auth_client, body=CreateKeyRequest(name="", namespace="pool"))
        assert resp.status_code.value == 400
        assert isinstance(resp.parsed, ErrorResponse)

    def test_forbidden(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/keys", status_code=403, json={"error": "key limit reached"}
        )
        resp = create_key.sync_detailed(client=auth_client, body=CreateKeyRequest(name="extra", namespace="pool"))
        assert resp.status_code.value == 403


# ---------------------------------------------------------------------------
# /api/keys/{id}  DELETE
# ---------------------------------------------------------------------------


class TestDeleteKey:
    @pytest.mark.parametrize("key_id", ["kc-uuid-1", "kc-uuid-2", "00000000-0000-0000-0000-000000000000"])
    def test_base(self, httpx_mock: HTTPXMock, auth_client, key_id):
        httpx_mock.add_response(method="DELETE", url=f"{BASE_URL}/api/keys/{key_id}", status_code=204)
        assert delete_key.sync_detailed(id=key_id, client=auth_client).status_code.value == 204

    def test_forbidden(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="DELETE", url=f"{BASE_URL}/api/keys/other-uuid", status_code=403, json={"error": "not your key"}
        )
        assert delete_key.sync_detailed(id="other-uuid", client=auth_client).status_code.value == 403


# ---------------------------------------------------------------------------
# /api/batch/{pool}/submit  POST
# ---------------------------------------------------------------------------


GOLDEN_SUBMIT_RESP = {
    "batch_id": "batch-abc",
    "name": "my-batch",
    "status": "queued",
    "created_at": "2026-05-01T00:00:00Z",
}
GOLDEN_RUN = RunConfig(
    steps=[Action(action_type="CLICK", parameters=ActionParameters.from_dict({"x": 1, "y": 2}))],
    timeout=60,
)


class TestSubmitBatch:
    @pytest.mark.parametrize(
        "pool,concurrency",
        [
            ("test-pool", 1),
            ("prod-pool", 10),
            ("staging", 50),
        ],
    )
    def test_base(self, httpx_mock: HTTPXMock, auth_client, pool, concurrency):
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/batch/{pool}/submit", status_code=202, json=GOLDEN_SUBMIT_RESP
        )
        resp = submit_batch.sync_detailed(
            pool=pool, client=auth_client, body=BatchSubmitRequest(runs=[GOLDEN_RUN], concurrency=concurrency)
        )
        assert resp.status_code.value == 202
        assert isinstance(resp.parsed, BatchSubmitResponse)
        assert resp.parsed.batch_id == "batch-abc"

    def test_bad_request_empty_runs(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST",
            url=f"{BASE_URL}/api/batch/pool/submit",
            status_code=400,
            json={"error": "runs must not be empty"},
        )
        resp = submit_batch.sync_detailed(pool="pool", client=auth_client, body=BatchSubmitRequest(runs=[]))
        assert resp.status_code.value == 400
        assert isinstance(resp.parsed, ErrorResponse)

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/label/test-pool/my-run/batch", status_code=202, json=GOLDEN_SUBMIT_RESP
        )
        result = auth_client.submit_label_batch(
            pool="test-pool", label="my-run", body=BatchSubmitRequest(runs=[GOLDEN_RUN])
        )
        assert isinstance(result, BatchSubmitResponse)


# ---------------------------------------------------------------------------
# /api/batch/{pool}/{id}/status  GET
# ---------------------------------------------------------------------------


GOLDEN_BATCH_STATUS = {
    "batch_id": "batch-abc",
    "name": "my-batch",
    "status": "running",
    "progress": {"completed": 3, "total": 10, "ok": 2, "failed": 1},
    "created_at": "2026-05-01T00:00:00Z",
    "updated_at": "2026-05-01T00:05:00Z",
}


class TestGetBatchStatus:
    def test_base(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/batch/test-pool/batch-abc/status",
            status_code=200,
            json=GOLDEN_BATCH_STATUS,
        )
        resp = get_batch_status.sync_detailed(pool="test-pool", id="batch-abc", client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, BatchStatusResponse)
        assert resp.parsed.status == "running"

    def test_not_found(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/batch/test-pool/unknown/status",
            status_code=404,
            json={"error": "batch not found"},
        )
        resp = get_batch_status.sync_detailed(pool="test-pool", id="unknown", client=auth_client)
        assert resp.status_code.value == 404
        assert isinstance(resp.parsed, ErrorResponse)

    @pytest.mark.parametrize("status_val", ["queued", "running", "completed", "failed", "cancelled"])
    def test_all_statuses(self, httpx_mock: HTTPXMock, auth_client, status_val):
        body = {**GOLDEN_BATCH_STATUS, "status": status_val}
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/batch/pool/batch-abc/status", status_code=200, json=body
        )
        resp = get_batch_status.sync_detailed(pool="pool", id="batch-abc", client=auth_client)
        assert resp.parsed.status == status_val

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/pool/test-label/status", status_code=200, json=GOLDEN_LABEL_STATUS
        )
        result = auth_client.get_label_status(pool="pool", label="test-label")
        assert isinstance(result, LabelStatusResponse)


# ---------------------------------------------------------------------------
# /api/batch/{pool}/{id}/results  GET
# ---------------------------------------------------------------------------


GOLDEN_BATCH_RESULTS = {
    "batch_id": "batch-abc",
    "name": "my-batch",
    "status": "completed",
    "runs": [
        {
            "run_id": 1,
            "vm_id": "vm-1",
            "ok": True,
            "reset_ms": 100,
            "step_ms": [200],
            "shutdown_ms": 50,
            "actions": [],
            "screenshots": [],
        }
    ],
    "s3_bucket": "my-bucket",
    "s3_key": "results/batch-abc.json",
    "s3_presigned_url": "https://s3.example.com/presigned",
    "summary": {
        "n_ok": 1,
        "n_fail": 0,
        "total_wall_ms": 350,
        "reset_stats": {"n": 1, "min": 100, "mean": 100.0, "max": 100},
        "step_stats": {"n": 1, "min": 200, "mean": 200.0, "max": 200},
        "shutdown_stats": {"n": 1, "min": 50, "mean": 50.0, "max": 50},
    },
}


class TestGetBatchResults:
    def test_base_completed(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/batch/pool/batch-abc/results", status_code=200, json=GOLDEN_BATCH_RESULTS
        )
        resp = get_batch_results.sync_detailed(pool="pool", id="batch-abc", client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, BatchResultsResponse)
        assert resp.parsed.status == "completed"

    def test_in_progress_202(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/batch/pool/batch-abc/results",
            status_code=202,
            json={"batch_id": "batch-abc", "status": "running", "message": "batch still running"},
        )
        resp = get_batch_results.sync_detailed(pool="pool", id="batch-abc", client=auth_client)
        assert resp.status_code.value == 202
        assert isinstance(resp.parsed, BatchInProgressResponse)

    def test_not_found(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/batch/pool/unknown/results", status_code=404, json={"error": "not found"}
        )
        assert get_batch_results.sync_detailed(pool="pool", id="unknown", client=auth_client).status_code.value == 404

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/pool/lbl/results", status_code=200, json=GOLDEN_LABEL_RESULTS
        )
        result = auth_client.get_label_results(pool="pool", label="lbl")
        assert isinstance(result, LabelResultsResponse)


# ---------------------------------------------------------------------------
# /api/batch/{pool}/{id}  DELETE
# ---------------------------------------------------------------------------


class TestCancelBatch:
    @pytest.mark.parametrize(
        "pool,id,expected_status",
        [
            ("test-pool", "batch-abc", "cancelling"),
            ("prod-pool", "batch-xyz", "completed"),
            ("staging", "batch-999", "cancelled"),
        ],
    )
    def test_base(self, httpx_mock: HTTPXMock, auth_client, pool, id, expected_status):
        httpx_mock.add_response(
            method="DELETE",
            url=f"{BASE_URL}/api/batch/{pool}/{id}",
            status_code=200,
            json={"batch_id": id, "status": expected_status},
        )
        resp = cancel_batch.sync_detailed(pool=pool, id=id, client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, CancelBatchResponse)
        assert resp.parsed.status == expected_status

    def test_not_found(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="DELETE", url=f"{BASE_URL}/api/batch/pool/unknown", status_code=404, json={"error": "not found"}
        )
        assert cancel_batch.sync_detailed(pool="pool", id="unknown", client=auth_client).status_code.value == 404

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="DELETE", url=f"{BASE_URL}/api/label/pool/test-label", status_code=200, json=GOLDEN_CANCEL_LABEL
        )
        result = auth_client.cancel_label(pool="pool", label="test-label")
        assert isinstance(result, CancelLabelResponse)


# ---------------------------------------------------------------------------
# /api/label/{pool}/{label}/batch  POST
# ---------------------------------------------------------------------------


class TestSubmitLabelBatch:
    @pytest.mark.parametrize(
        "pool,label,concurrency",
        [
            ("test-pool", "test-label", 1),
            ("prod-pool", "prod-run-2026-05-20", 10),
            ("staging", "label-with-dashes", 50),
        ],
    )
    def test_base(self, httpx_mock: HTTPXMock, auth_client, pool, label, concurrency):
        resp_body = {**GOLDEN_SUBMIT_RESP, "name": label}
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/label/{pool}/{label}/batch", status_code=202, json=resp_body
        )
        resp = submit_label_batch.sync_detailed(
            pool=pool,
            label=label,
            client=auth_client,
            body=BatchSubmitRequest(runs=[GOLDEN_RUN], concurrency=concurrency),
        )
        assert resp.status_code.value == 202
        assert isinstance(resp.parsed, BatchSubmitResponse)

    def test_bad_request(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST",
            url=f"{BASE_URL}/api/label/pool/test/batch",
            status_code=400,
            json={"error": "runs must not be empty"},
        )
        resp = submit_label_batch.sync_detailed(
            pool="pool", label="test", client=auth_client, body=BatchSubmitRequest(runs=[])
        )
        assert resp.status_code.value == 400
        assert isinstance(resp.parsed, ErrorResponse)

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="POST", url=f"{BASE_URL}/api/label/test-pool/my-run/batch", status_code=202, json=GOLDEN_SUBMIT_RESP
        )
        result = auth_client.submit_label_batch(
            pool="test-pool", label="my-run", body=BatchSubmitRequest(runs=[GOLDEN_RUN])
        )
        assert isinstance(result, BatchSubmitResponse)


# ---------------------------------------------------------------------------
# /api/label/{pool}/{label}/status  GET
# ---------------------------------------------------------------------------


GOLDEN_LABEL_STATUS = {
    "label": "test-label",
    "batch_count": 2,
    "all_completed": False,
    "progress": {"completed": 5, "total": 10, "ok": 4, "failed": 1},
    "batches": [
        {
            "batch_id": "batch-abc",
            "status": "running",
            "progress": {"completed": 5, "total": 10, "ok": 4, "failed": 1},
            "created_at": "2026-05-01T00:00:00Z",
            "updated_at": "2026-05-01T01:00:00Z",
        }
    ],
}


class TestGetLabelStatus:
    def test_base(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/label/test-pool/test-label/status",
            status_code=200,
            json=GOLDEN_LABEL_STATUS,
        )
        resp = get_label_status.sync_detailed(pool="test-pool", label="test-label", client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, LabelStatusResponse)
        assert resp.parsed.all_completed is False

    def test_all_completed(self, httpx_mock: HTTPXMock, auth_client):
        body = {**GOLDEN_LABEL_STATUS, "all_completed": True}
        httpx_mock.add_response(method="GET", url=f"{BASE_URL}/api/label/pool/done/status", status_code=200, json=body)
        assert (
            get_label_status.sync_detailed(pool="pool", label="done", client=auth_client).parsed.all_completed is True
        )

    def test_not_found(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/label/pool/unknown/status",
            status_code=404,
            json={"error": "label not found"},
        )
        assert get_label_status.sync_detailed(pool="pool", label="unknown", client=auth_client).status_code.value == 404

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/pool/lbl/status", status_code=200, json=GOLDEN_LABEL_STATUS
        )
        result = auth_client.get_label_status(pool="pool", label="lbl")
        assert isinstance(result, LabelStatusResponse)


# ---------------------------------------------------------------------------
# /api/label/{pool}/{label}/results  GET
# ---------------------------------------------------------------------------


GOLDEN_LABEL_RESULTS = {
    "label": "test-label",
    "batch_count": 1,
    "completed_batch_count": 1,
    "all_completed": True,
    "batches": [GOLDEN_BATCH_RESULTS],
    "summary": GOLDEN_BATCH_RESULTS["summary"],
}


class TestGetLabelResults:
    def test_base_completed(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/pool/lbl/results", status_code=200, json=GOLDEN_LABEL_RESULTS
        )
        resp = get_label_results.sync_detailed(pool="pool", label="lbl", client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, LabelResultsResponse)
        assert resp.parsed.all_completed is True

    def test_drain_mode(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/label/pool/lbl/results?completed_only=true",
            status_code=200,
            json=GOLDEN_LABEL_RESULTS,
        )
        resp = get_label_results.sync_detailed(pool="pool", label="lbl", client=auth_client, completed_only=True)
        assert resp.status_code.value == 200

    def test_in_progress_202(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET",
            url=f"{BASE_URL}/api/label/pool/lbl/results",
            status_code=202,
            json={"label": "lbl", "batch_count": 1, "all_completed": False, "message": "still running"},
        )
        resp = get_label_results.sync_detailed(pool="pool", label="lbl", client=auth_client)
        assert resp.status_code.value == 202
        assert isinstance(resp.parsed, LabelInProgressResponse)

    @pytest.mark.parametrize(
        "pool,label",
        [
            ("test-pool", "test-label"),
            ("prod-pool", "prod-2026"),
            ("staging", "my-experiment"),
        ],
    )
    def test_generated_pool_label(self, httpx_mock: HTTPXMock, auth_client, pool, label):
        body = {**GOLDEN_LABEL_RESULTS, "label": label}
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/{pool}/{label}/results", status_code=200, json=body
        )
        resp = get_label_results.sync_detailed(pool=pool, label=label, client=auth_client)
        assert resp.parsed.label == label

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="GET", url=f"{BASE_URL}/api/label/pool/lbl/results", status_code=200, json=GOLDEN_LABEL_RESULTS
        )
        result = auth_client.get_label_results(pool="pool", label="lbl")
        assert isinstance(result, LabelResultsResponse)


# ---------------------------------------------------------------------------
# /api/label/{pool}/{label}  DELETE
# ---------------------------------------------------------------------------


GOLDEN_CANCEL_LABEL = {
    "label": "test-label",
    "batch_count": 2,
    "cancelled_count": 1,
    "already_done": 1,
    "batches": [
        {"batch_id": "batch-abc", "status": "cancelling"},
        {"batch_id": "batch-xyz", "status": "completed"},
    ],
}


class TestCancelLabel:
    @pytest.mark.parametrize(
        "pool,label",
        [
            ("test-pool", "test-label"),
            ("prod-pool", "prod-run"),
            ("staging", "experiment-42"),
        ],
    )
    def test_base(self, httpx_mock: HTTPXMock, auth_client, pool, label):
        body = {**GOLDEN_CANCEL_LABEL, "label": label}
        httpx_mock.add_response(method="DELETE", url=f"{BASE_URL}/api/label/{pool}/{label}", status_code=200, json=body)
        resp = cancel_label.sync_detailed(pool=pool, label=label, client=auth_client)
        assert resp.status_code.value == 200
        assert isinstance(resp.parsed, CancelLabelResponse)

    def test_not_found(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="DELETE",
            url=f"{BASE_URL}/api/label/pool/unknown",
            status_code=404,
            json={"error": "label not found"},
        )
        assert cancel_label.sync_detailed(pool="pool", label="unknown", client=auth_client).status_code.value == 404

    def test_flat_wrapper(self, httpx_mock: HTTPXMock, auth_client):
        httpx_mock.add_response(
            method="DELETE", url=f"{BASE_URL}/api/label/pool/lbl", status_code=200, json=GOLDEN_CANCEL_LABEL
        )
        result = auth_client.cancel_label(pool="pool", label="lbl")
        assert isinstance(result, CancelLabelResponse)
