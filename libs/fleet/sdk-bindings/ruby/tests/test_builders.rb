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
