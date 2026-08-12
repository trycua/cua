import asyncio
import json
import re
from dataclasses import dataclass

from fleet_sdk import (ClaimSpec, CreateClaimRequest, CreatePoolRequest, CyclopsClient,
    CyclopsConfiguration, CyclopsCredentials, HttpClient, HttpRequest, HttpResponse,
    OsGymSandboxWarmPoolSpec, Sandbox, SandboxTemplateRef, VmTemplate)

BASE = 'https://cyclops.invalid'
TOKEN = 'https://keycloak.invalid/realms/offline/protocol/openid-connect/token'
JSON_HEADERS = [('accept', 'application/json'), ('content-type', 'application/json'), ('authorization', 'Bearer offline-token')]
SERVICE_HEADERS = [('authorization', 'Bearer offline-token')]
SERVICE_URL = f'{BASE}/api/svc/default/offline-sandbox-mcp/mcp'

@dataclass(frozen=True)
class Expected:
    method: str
    url: str
    headers: list
    body: bytes | None
    status: int
    response: object

def normalized_generated_claim_body(body):
    if body is None:
        return None
    value = json.loads(body)
    name = value['metadata']['name']
    assert len(name.encode()) <= 63 and re.fullmatch(r'claim-[a-z0-9](?:[-a-z0-9]*[a-z0-9])?', name), name
    value['metadata']['name'] = 'claim-generated'
    return json.dumps(value, separators=(',', ':'), sort_keys=True).encode()

class ScriptedHttpClient(HttpClient):
    def __init__(self, expected):
        self.expected = list(expected)
        self.requests = []
        self.lock = asyncio.Lock()

    async def execute(self, request):
        async with self.lock:
            assert self.expected, f'unexpected request: {request.method} {request.url}'
            item = self.expected.pop(0)
            actual_headers = [(header.name, header.value) for header in request.headers]
            actual = (request.method, request.url, actual_headers)
            expected = (item.method, item.url, item.headers)
            assert actual == expected, f"request mismatch:\nactual={actual!r}\nexpected={expected!r}"
            if request.method == 'POST' and request.url.endswith('/osgymsandboxclaims'):
                assert normalized_generated_claim_body(request.body) == normalized_generated_claim_body(item.body)
            else:
                assert (request.method, request.url, actual_headers, request.body) == (item.method, item.url, item.headers, item.body)
            self.requests.append(request)
            body = item.response if isinstance(item.response, bytes) else json.dumps(item.response, separators=(',', ':')).encode()
            return HttpResponse(status=item.status, headers=[], body=body)

    def assert_exhausted(self):
        assert not self.expected, self.expected

def token_expected():
    return Expected('POST', TOKEN, [('accept', 'application/json'), ('content-type', 'application/x-www-form-urlencoded'), ('authorization', 'Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ=')], b'grant_type=client_credentials', 200, {'access_token': 'offline-token', 'expires_in': 3600})

def service_expected(body, response):
    return Expected('POST', SERVICE_URL, SERVICE_HEADERS, body, 202, response)

def expected_service_calls(*calls):
    return [token_expected(), *(service_expected(body, response) for body, response in calls)]

def offline_sandbox():
    return Sandbox(namespace='default', claim='default', name='offline-sandbox', services=['mcp'])

def service_request(body):
    return HttpRequest(method='POST', url='https://ignored.invalid/mcp', headers=[], body=body)

def pool_spec():
    return OsGymSandboxWarmPoolSpec(replicas=1, sandbox_template_ref=SandboxTemplateRef(name='default'), autoscaling=None)

def pool_response():
    return {'apiVersion': 'osgym.cua.ai/v1alpha1', 'kind': 'OSGymSandboxWarmPool', 'metadata': {'namespace': 'default', 'name': 'default', 'labels': None}, 'spec': {'replicas': 1, 'sandboxTemplateRef': {'name': 'default'}}, 'status': None}

def template_response():
    return {'apiVersion': 'osgym.cua.ai/v1alpha1', 'kind': 'OSGymSandboxTemplate', 'metadata': {'namespace': 'default', 'name': 'default', 'labels': None}, 'spec': {'vmTemplate': {'containerDiskImage': 'registry.example/desktop:offline', 'services': [{'name': 'mcp', 'targetPort': 8080}]}}}

def claim_response(bound=False):
    result = {'apiVersion': 'osgym.cua.ai/v1alpha1', 'kind': 'OSGymSandboxClaim', 'metadata': {'namespace': 'default', 'name': 'default', 'labels': None}, 'spec': {'sandboxTemplateRef': {'name': 'default'}}, 'status': None}
    if bound:
        result['status'] = {'phase': 'Bound', 'sandbox': {'name': 'offline-sandbox'}}
    return result

def expected_lifecycle():
    pool_body = json.dumps(pool_response(), separators=(',', ':')).encode()
    claim_body = b'{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"claim-1","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":null}'
    claim_url = f'{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default'
    pool_url = f'{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxwarmpools/default'
    template_url = f'{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxtemplates/default'
    return [
        token_expected(),
        Expected('POST', f'{BASE}/api/namespaces', JSON_HEADERS, b'{"name":"default"}', 201, {}),
        Expected('POST', pool_url.removesuffix('/default'), JSON_HEADERS, pool_body, 201, pool_response()),
        Expected('POST', claim_url.removesuffix('/default'), JSON_HEADERS, claim_body, 201, claim_response()),
        Expected('GET', claim_url, JSON_HEADERS, None, 200, claim_response(True)),
        Expected('GET', template_url, JSON_HEADERS, None, 200, template_response()),
        service_expected(b'{"offline":true}', b'offline service accepted'),
        Expected('DELETE', claim_url, JSON_HEADERS, None, 204, b''),
        Expected('DELETE', pool_url, JSON_HEADERS, None, 204, b''),
        Expected('DELETE', f'{BASE}/api/namespaces/default', JSON_HEADERS, None, 204, b''),
    ]

def client(transport):
    return CyclopsClient.connect(CyclopsConfiguration(base_url=BASE, token_url=TOKEN, credentials=CyclopsCredentials('client-id', 'client-secret'), pool_poll_interval_ms=1, pool_poll_limit=1, claim_poll_interval_ms=1, claim_poll_limit=2), transport)

async def run_lifecycle(transport):
    vm = VmTemplate(container_disk_image='registry.example/desktop:offline', command=None, runtime=None, runtime_class_name=None, node_selector=None, tolerations=None, image_pull_policy=None, image_pull_secret=None, cpu_cores=None, memory=None, firmware=None, probes=None, services=None, oidc=None)
    assert vm.container_disk_image
    sdk = client(transport)
    pool = await sdk.create_pool(CreatePoolRequest(namespace='default', spec=pool_spec()))
    claim = await sdk.create_claim(CreateClaimRequest(pool=pool, spec=ClaimSpec(sandbox_template_ref=SandboxTemplateRef(name=pool.metadata.name), warmpool=None, bind_deadline=None, lifecycle=None)))
    sandbox = await sdk.wait_claim(claim)
    service = await sdk.service_request(sandbox, 'mcp', '/mcp', service_request(b'{"offline":true}'))
    await sdk.delete_claim(claim)
    await sdk.delete_pool(pool)
    transport.assert_exhausted()
    return pool, claim, sandbox, service
