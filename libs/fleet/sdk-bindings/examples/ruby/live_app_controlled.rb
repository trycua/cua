#!/usr/bin/env ruby
require 'json'
require 'net/http'
require 'uri'
require 'cyclops_sdk'

class NetHttpClient < CyclopsSdk::HttpClient
  def execute(request)
    uri = URI(request.url)
    native = Net::HTTPGenericRequest.new(request.method, !request.body.nil?, request.method != 'HEAD', uri.request_uri, request.headers.to_h { |header| [header.name, header.value] })
    native.body = request.body unless request.body.nil?
    response = Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == 'https', open_timeout: 30, read_timeout: 60) { |http| http.request(native) }
    CyclopsSdk::HttpResponse.new(status: response.code.to_i, headers: response.each_header.map { |name, value| CyclopsSdk::HttpHeader.new(name: name, value: value) }, body: response.body.to_s.b)
  rescue StandardError => error
    raise CyclopsSdk::HttpError::Transport.new(reason: error.message)
  end
end

def required(name)
  ENV.fetch(name).tap { |value| raise "#{name} is required" if value.empty? }
end

def pool_spec
  CyclopsSdk::PoolSpec.new(replicas: 1, template: CyclopsSdk::PoolTemplate.new(runtime: nil, runtime_class_name: nil, node_selector: nil, tolerations: nil, command: nil, container_disk_image: required('CUA_IMAGE'), image_pull_secret: required('CUA_IMAGE_PULL_SECRET'), cpu_cores: 4, memory: '4Gi', firmware: nil, probes: nil, oidc: nil), autoscaling: nil, services: [CyclopsSdk::SandboxService.new(name: 'mcp', target_port: 3000, protocol: nil)])
end

def initialize_mcp(client, sandbox)
  body = JSON.generate(jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'cyclops-uniffi-ruby', version: '0.1.0' } }).b
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 300
  loop do
    response = client.service_request(sandbox, 'mcp', '/mcp', CyclopsSdk::HttpRequest.new(method: 'POST', url: 'https://ignored.invalid/mcp', headers: [CyclopsSdk::HttpHeader.new(name: 'accept', value: 'application/json, text/event-stream'), CyclopsSdk::HttpHeader.new(name: 'content-type', value: 'application/json')], body: body))
    return response.status if response.status.between?(200, 299)
    if [502, 503, 504].include?(response.status) && Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
      sleep 5
      next
    end
    raise "MCP initialize failed with HTTP #{response.status}: #{response.body.inspect}"
  end
end

namespace = required('CYCLOPS_NAMESPACE')
credentials = CyclopsSdk::CyclopsCredentials.new(required('CUA_CLIENT_ID'), required('CUA_CLIENT_SECRET'))
configuration = CyclopsSdk::CyclopsConfiguration.new(base_url: required('CUA_BASE_URL'), token_url: required('CUA_TOKEN_URL'), credentials: credentials, pool_poll_interval_ms: 5000, pool_poll_limit: 120, claim_poll_interval_ms: 5000, claim_poll_limit: 120)
client = CyclopsSdk::CyclopsClient.connect(configuration, NetHttpClient.new)
pool = nil
claim = nil
begin
  pool = client.create_pool(CyclopsSdk::CreatePoolRequest.new(namespace: namespace, spec: pool_spec))
  claim = client.create_claim(CyclopsSdk::CreateClaimRequest.new(pool: pool, spec: nil))
  sandbox = client.wait_claim(claim)
  status = initialize_mcp(client, sandbox)
  puts JSON.generate(namespace: namespace, pool: pool.metadata.name, claim: claim.metadata.name, sandbox: sandbox.name, mcp_status: status)
ensure
  begin
    client.delete_claim(claim) unless claim.nil?
  ensure
    client.delete_pool(pool) unless pool.nil?
  end
end
