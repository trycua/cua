require 'cyclops_sdk'
service = FleetSdk::SandboxServiceBuilder.new.name('mcp').target_port(3000).build
vm = FleetSdk::VmTemplateBuilder.new.container_disk_image('registry.example/vm:latest').image_pull_secret('registry-secret').cpu_cores(4).memory('8Gi').services([service]).build
template = FleetSdk::OSGymSandboxTemplateSpecBuilder.new.vm_template(vm).build
request = FleetSdk::CreateTemplateRequestBuilder.new.namespace('default').name('desktop').spec(template).build
raise 'wrong service type' unless service.instance_of?(FleetSdk::SandboxService)
raise 'wrong VM type' unless vm.instance_of?(FleetSdk::VmTemplate)
raise 'wrong request type' unless request.instance_of?(FleetSdk::CreateTemplateRequest)
raise 'optional protocol was not omitted' unless service.protocol.nil?
raise 'optional command was not omitted' unless vm.command.nil?
begin
  FleetSdk::VmTemplateBuilder.new.build
  raise 'missing required field did not fail'
rescue FleetSdk::SchemaBuildError::MissingRequiredField => error
  raise 'wrong schema error' unless error.record_type == 'VmTemplate' && error.field == 'container_disk_image'
end
legacy = FleetSdk::SandboxTemplateRef.new(name: 'legacy')
raise 'legacy constructor changed' unless legacy.name == 'legacy'
configuration = FleetSdk::CyclopsTokenProviderConfigurationBuilder.new
  .base_url('https://api.example.test')
  .pool_poll_interval_ms(5000)
  .pool_poll_limit(120)
  .claim_poll_interval_ms(5000)
  .claim_poll_limit(120)
  .build
user_key = FleetSdk::CreateUserApiKeyRequestBuilder.new.name('automation').scope([]).build
autoscaling = FleetSdk::WarmPoolAutoscalingBuilder.new
  .min_pool_size(1).initial_pool_size(2).max_pool_size(5).build
raise 'wrong polling value' unless configuration.pool_poll_interval_ms == 5000
raise 'scope default changed' unless user_key.scope.empty?
raise 'wrong autoscaling maximum' unless autoscaling.max_pool_size == 5
begin
  FleetSdk::TemplateBuilder.new.build
  raise 'missing Template field did not fail'
rescue FleetSdk::SdkBuildError::MissingRequiredField
end
begin
  FleetSdk::CreateClaimRequestBuilder.new.build
  raise 'missing claim pool did not fail'
rescue FleetSdk::SdkBuildError::MissingRequiredField
end
http_request = FleetSdk::HttpRequestBuilder.new.method('GET').url('https://run.cua.ai/v1/pools').headers([]).build
raise 'wrong http request type' unless http_request.instance_of?(FleetSdk::HttpRequest)
raise 'optional body was not omitted' unless http_request.body.nil?
raise 'optional timeout was not omitted' unless http_request.timeout_secs.nil?
bounded = FleetSdk::HttpRequestBuilder.new.method('GET').url('https://run.cua.ai/v1/pools').headers([]).timeout_secs(30).build
raise 'wrong bounded timeout' unless bounded.timeout_secs == 30
begin
  FleetSdk::HttpRequestBuilder.new.method('GET').headers([]).build
  raise 'missing http request url did not fail'
rescue FleetSdk::SdkBuildError::MissingRequiredField => error
  raise 'wrong http request error' unless error.record_type == 'HttpRequest' && error.field == 'url'
end
legacy_http_request = FleetSdk::HttpRequest.new(method: 'GET', url: 'https://run.cua.ai/v1/pools', headers: [], body: nil)
raise 'constructor timeout default changed' unless legacy_http_request.timeout_secs.nil?
