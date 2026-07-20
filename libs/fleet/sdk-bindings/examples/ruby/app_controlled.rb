require 'json'
require 'cyclops_sdk'

require 'json'
require 'thread'
Expected = Struct.new(:method, :url, :headers, :body, :status, :response)
BASE = 'https://cyclops.invalid'; TOKEN = 'https://keycloak.invalid/token'; JSON_HEADERS = [['accept','application/json'],['content-type','application/json'],['authorization','Bearer offline-token']]
def pool_json; { apiVersion:'cua.ai/v1',kind:'OSGymWorkspacePool',metadata:{namespace:'default',name:'default',labels:nil},spec:{replicas:1,template:{containerDiskImage:'registry.example/desktop:offline'},services:[{name:'mcp',targetPort:8080}]},status:nil }; end
def claim_json(bound=false); value={apiVersion:'osgym.cua.ai/v1alpha1',kind:'OSGymSandboxClaim',metadata:{namespace:'default',name:'default',labels:nil},spec:{sandboxTemplateRef:{name:'default'}},status:nil}; value[:status]={phase:'Bound',sandbox:{name:'offline-sandbox'}} if bound; value; end
def queue
 pool_url="#{BASE}/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default"; claim_url="#{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default"; pool_body=JSON.generate(pool_json).b; claim_body='{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"claim-1","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":null}'.b
 [Expected.new('POST',TOKEN,[['accept','application/json'],['content-type','application/x-www-form-urlencoded']], 'grant_type=client_credentials&client_id=client-id&client_secret=client-secret'.b,200,{access_token:'offline-token',expires_in:3600}),Expected.new('POST',"#{BASE}/api/namespaces",JSON_HEADERS,'{"name":"default"}'.b,201,{}),Expected.new('POST',pool_url.sub(%r{/default\z}, ''),JSON_HEADERS,pool_body,201,pool_json),Expected.new('POST',claim_url.sub(%r{/default\z}, ''),JSON_HEADERS,claim_body,201,claim_json),Expected.new('GET',claim_url,JSON_HEADERS,nil,200,claim_json(true)),Expected.new('GET',pool_url,JSON_HEADERS,nil,200,pool_json),Expected.new('POST',"#{BASE}/api/svc/default/offline-sandbox-mcp/mcp",[['authorization','Bearer offline-token']],'{"offline":true}'.b,202,'offline service accepted'),Expected.new('DELETE',claim_url,JSON_HEADERS,nil,204,''),Expected.new('DELETE',pool_url,JSON_HEADERS,nil,204,''),Expected.new('DELETE',"#{BASE}/api/namespaces/default",JSON_HEADERS,nil,204,'')]
end

class ScriptedHttpClient < CyclopsSdk::HttpClient
  def initialize; @expected=queue; @mutex=Mutex.new; end
  def execute(request)
    @mutex.synchronize do
      item=@expected.shift or raise 'unexpected request'
      actual=[request.method,request.url,request.headers.map{|h|[h.name,h.value]},request.body]
      raise "request mismatch: #{actual.inspect}" unless actual == [item.method,item.url,item.headers,item.body]
      body=item.response.is_a?(String) ? item.response.b : JSON.generate(item.response).b
      CyclopsSdk::HttpResponse.new(status:item.status,headers:[],body:body)
    end
  end
  def assert_exhausted!; raise @expected.inspect unless @expected.empty?; end
end

vm_template = CyclopsSdk::VmTemplate.new(container_disk_image: 'registry.example/desktop:offline', command: nil, runtime: nil, runtime_class_name: nil, node_selector: nil, tolerations: nil, image_pull_policy: nil, image_pull_secret: nil, cpu_cores: nil, memory: nil, firmware: nil, probes: nil, services: nil, oidc: nil)
spec = CyclopsSdk::PoolSpec.new(replicas: 1, template: CyclopsSdk::PoolTemplate.new(runtime: nil, runtime_class_name: nil, node_selector: nil, tolerations: nil, command: nil, container_disk_image: vm_template.container_disk_image, image_pull_secret: nil, cpu_cores: nil, memory: nil, firmware: nil, probes: nil, oidc: nil), autoscaling: nil, services: [CyclopsSdk::SandboxService.new(name: 'mcp', target_port: 8080, protocol: nil)])
transport = ScriptedHttpClient.new
credentials = CyclopsSdk::CyclopsCredentials.new('client-id', 'client-secret')
client = CyclopsSdk::CyclopsClient.connect(CyclopsSdk::CyclopsConfiguration.new(base_url: 'https://cyclops.invalid', token_url: 'https://keycloak.invalid/token', credentials: credentials, pool_poll_interval_ms: 1, pool_poll_limit: 1, claim_poll_interval_ms: 1, claim_poll_limit: 2), transport)
pool = client.create_pool(CyclopsSdk::CreatePoolRequest.new(namespace: 'default', spec: spec))
claim = client.create_claim(CyclopsSdk::CreateClaimRequest.new(pool: pool, spec: CyclopsSdk::ClaimSpec.new(sandbox_template_ref: CyclopsSdk::SandboxTemplateRef.new(name: pool.metadata.name), warmpool: nil, bind_deadline: nil, lifecycle: nil)))
sandbox = client.wait_claim(claim)
service = client.service_request(sandbox, 'mcp', '/mcp', CyclopsSdk::HttpRequest.new(method: 'POST', url: 'https://ignored.invalid/mcp', headers: [], body: '{"offline":true}'.b))
client.delete_claim(claim)
client.delete_pool(pool)
transport.assert_exhausted!
raise 'unexpected service response' unless service.status == 202
puts "pool=#{pool.metadata.name} claim=#{claim.metadata.name} sandbox=#{sandbox.name} service_status=#{service.status}"
