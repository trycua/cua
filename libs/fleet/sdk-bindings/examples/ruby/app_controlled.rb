require 'json'
require 'cyclops_sdk'

require 'json'
require 'thread'
Expected = Struct.new(:method, :url, :headers, :body, :status, :response)
BASE = 'https://cyclops.invalid'; TOKEN = 'https://keycloak.invalid/token'; JSON_HEADERS = [['accept','application/json'],['content-type','application/json'],['authorization','Bearer offline-token']]
GENERATED_CLAIM_NAME = /\Aclaim-[a-z0-9](?:[-a-z0-9]*[a-z0-9])?\z/

def normalized_generated_claim_body(body)
  return nil if body.nil?
  value = JSON.parse(body)
  name = value.fetch('metadata').fetch('name')
  raise "invalid generated claim name: #{name.inspect}" unless name.bytesize <= 63 && GENERATED_CLAIM_NAME.match?(name)
  value['metadata']['name'] = 'claim-generated'
  JSON.generate(value).b
end

def pool_json; { apiVersion:'osgym.cua.ai/v1alpha1',kind:'OSGymSandboxWarmPool',metadata:{namespace:'default',name:'default',labels:nil},spec:{replicas:1,sandboxTemplateRef:{name:'default'}},status:nil }; end
def template_json; { apiVersion:'osgym.cua.ai/v1alpha1',kind:'OSGymSandboxTemplate',metadata:{namespace:'default',name:'default',labels:nil},spec:{vmTemplate:{containerDiskImage:'registry.example/desktop:offline',services:[{name:'mcp',targetPort:8080}]}} }; end
def claim_json(bound=false); value={apiVersion:'osgym.cua.ai/v1alpha1',kind:'OSGymSandboxClaim',metadata:{namespace:'default',name:'default',labels:nil},spec:{sandboxTemplateRef:{name:'default'}},status:nil}; value[:status]={phase:'Bound',sandbox:{name:'offline-sandbox'}} if bound; value; end
def queue
 pool_url="#{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxwarmpools/default"; template_url="#{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxtemplates/default"; claim_url="#{BASE}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default"; pool_body=JSON.generate(pool_json).b; claim_body='{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"claim-1","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":null}'.b
 [Expected.new('POST',TOKEN,[['accept','application/json'],['content-type','application/x-www-form-urlencoded'],['authorization','Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ=']], 'grant_type=client_credentials'.b,200,{access_token:'offline-token',expires_in:3600}),Expected.new('POST',"#{BASE}/api/namespaces",JSON_HEADERS,'{"name":"default"}'.b,201,{}),Expected.new('POST',pool_url.sub(%r{/default\z}, ''),JSON_HEADERS,pool_body,201,pool_json),Expected.new('POST',claim_url.sub(%r{/default\z}, ''),JSON_HEADERS,claim_body,201,claim_json),Expected.new('GET',claim_url,JSON_HEADERS,nil,200,claim_json(true)),Expected.new('GET',template_url,JSON_HEADERS,nil,200,template_json),Expected.new('POST',"#{BASE}/api/svc/default/offline-sandbox-mcp/mcp",[['authorization','Bearer offline-token']],'{"offline":true}'.b,202,'offline service accepted'),Expected.new('DELETE',claim_url,JSON_HEADERS,nil,204,''),Expected.new('DELETE',pool_url,JSON_HEADERS,nil,204,''),Expected.new('DELETE',"#{BASE}/api/namespaces/default",JSON_HEADERS,nil,204,'')]
end

class ScriptedHttpClient < FleetSdk::HttpClient
  def initialize; @expected=queue; @mutex=Mutex.new; end
  def execute(request)
    @mutex.synchronize do
      item=@expected.shift or raise 'unexpected request'
      actual=[request.method,request.url,request.headers.map{|h|[h.name,h.value]},request.body]
      expected=[item.method,item.url,item.headers,item.body]
      if request.method == 'POST' && request.url.end_with?('/osgymsandboxclaims')
        raise "request mismatch: #{actual.inspect}" unless actual.first(3) == expected.first(3) && normalized_generated_claim_body(request.body) == normalized_generated_claim_body(item.body)
      else
        raise "request mismatch: #{actual.inspect}" unless actual == expected
      end
      body=item.response.is_a?(String) ? item.response.b : JSON.generate(item.response).b
      FleetSdk::HttpResponse.new(status:item.status,headers:[],body:body)
    end
  end
  def assert_exhausted!; raise @expected.inspect unless @expected.empty?; end
end

spec = FleetSdk::OSGymSandboxWarmPoolSpec.new(replicas: 1, sandbox_template_ref: FleetSdk::SandboxTemplateRef.new(name: 'default'), autoscaling: nil)
transport = ScriptedHttpClient.new
credentials = FleetSdk::CyclopsCredentials.new('client-id', 'client-secret')
client = FleetSdk::CyclopsClient.connect(FleetSdk::CyclopsConfiguration.new(base_url: 'https://cyclops.invalid', token_url: 'https://keycloak.invalid/token', credentials: credentials, pool_poll_interval_ms: 1, pool_poll_limit: 1, claim_poll_interval_ms: 1, claim_poll_limit: 2), transport)
pool = client.create_pool(FleetSdk::CreatePoolRequest.new(namespace: 'default', spec: spec))
claim = client.create_claim(FleetSdk::CreateClaimRequest.new(pool: pool, spec: FleetSdk::ClaimSpec.new(sandbox_template_ref: FleetSdk::SandboxTemplateRef.new(name: pool.metadata.name), warmpool: nil, bind_deadline: nil, lifecycle: nil)))
sandbox = client.wait_claim(claim)
service = client.service_request(sandbox, 'mcp', '/mcp', FleetSdk::HttpRequest.new(method: 'POST', url: 'https://ignored.invalid/mcp', headers: [], body: '{"offline":true}'.b))
client.delete_claim(claim)
client.delete_pool(pool)
transport.assert_exhausted!
raise 'unexpected service response' unless service.status == 202
puts "pool=#{pool.metadata.name} claim=#{claim.metadata.name} sandbox=#{sandbox.name} service_status=#{service.status}"
