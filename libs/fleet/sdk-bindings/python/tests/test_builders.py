import unittest

import fleet_sdk


class GeneratedBuilderTest(unittest.TestCase):
    def test_generated_builders_cover_fluent_nested_records_and_omission(self):
        service = fleet_sdk.SandboxServiceBuilder().name("mcp").target_port(3000).build()
        vm = (
            fleet_sdk.VmTemplateBuilder()
            .container_disk_image("registry.example/vm:latest")
            .image_pull_secret("registry-secret")
            .cpu_cores(4)
            .memory("8Gi")
            .services([service])
            .build()
        )
        template_spec = fleet_sdk.OsGymSandboxTemplateSpecBuilder().vm_template(vm).build()
        request = (
            fleet_sdk.CreateTemplateRequestBuilder()
            .namespace("default")
            .name("desktop")
            .spec(template_spec)
            .build()
        )

        self.assertIs(type(service), fleet_sdk.SandboxService)
        self.assertIs(type(vm), fleet_sdk.VmTemplate)
        self.assertIs(type(template_spec), fleet_sdk.OsGymSandboxTemplateSpec)
        self.assertIs(type(request), fleet_sdk.CreateTemplateRequest)
        self.assertIsNone(service.protocol)
        self.assertIsNone(vm.command)
        self.assertEqual(vm.image_pull_secret, "registry-secret")
        self.assertEqual(vm.cpu_cores, 4)
        self.assertEqual(vm.memory, "8Gi")

    def test_builder_setters_preserve_prior_versions(self):
        base = fleet_sdk.VmTemplateBuilder()
        first = base.container_disk_image("registry.example/first:latest")
        second = base.container_disk_image("registry.example/second:latest")

        with self.assertRaises(fleet_sdk.SchemaBuildError.MissingRequiredField):
            base.build()
        self.assertEqual(first.build().container_disk_image, "registry.example/first:latest")
        self.assertEqual(second.build().container_disk_image, "registry.example/second:latest")

    def test_generated_builders_return_stable_required_field_errors(self):
        with self.assertRaises(fleet_sdk.SchemaBuildError.MissingRequiredField) as error:
            fleet_sdk.VmTemplateBuilder().build()
        self.assertEqual(error.exception.record_type, "VmTemplate")
        self.assertEqual(error.exception.field, "container_disk_image")

        with self.assertRaises(fleet_sdk.SdkBuildError.MissingRequiredField) as error:
            fleet_sdk.CreatePoolRequestBuilder().build()
        self.assertEqual(error.exception.record_type, "CreatePoolRequest")
        self.assertEqual(error.exception.field, "namespace")

    def test_legacy_record_constructor_remains_available(self):
        reference = fleet_sdk.SandboxTemplateRef(name="legacy")
        self.assertIs(type(reference), fleet_sdk.SandboxTemplateRef)
        self.assertEqual(reference.name, "legacy")

    def test_http_request_builder_skips_optionals_and_enforces_required_fields(self):
        request = (
            fleet_sdk.HttpRequestBuilder()
            .method("GET")
            .url("https://run.cua.ai/v1/pools")
            .headers([])
            .build()
        )
        bounded = (
            fleet_sdk.HttpRequestBuilder()
            .method("GET")
            .url("https://run.cua.ai/v1/pools")
            .headers([])
            .timeout_secs(30)
            .build()
        )

        self.assertIs(type(request), fleet_sdk.HttpRequest)
        self.assertIsNone(request.body)
        self.assertIsNone(request.timeout_secs)
        self.assertEqual(bounded.timeout_secs, 30)
        with self.assertRaises(fleet_sdk.SdkBuildError.MissingRequiredField) as error:
            fleet_sdk.HttpRequestBuilder().method("GET").headers([]).build()
        self.assertEqual(error.exception.record_type, "HttpRequest")
        self.assertEqual(error.exception.field, "url")

    def test_http_request_constructor_treats_timeout_secs_as_optional(self):
        request = fleet_sdk.HttpRequest(
            method="GET", url="https://run.cua.ai/v1/pools", headers=[], body=None
        )
        self.assertIsNone(request.timeout_secs)

    def test_remaining_frontend_builders_are_generated(self):
        configuration = (
            fleet_sdk.CyclopsTokenProviderConfigurationBuilder()
            .base_url("https://api.example.test")
            .pool_poll_interval_ms(5000)
            .pool_poll_limit(120)
            .claim_poll_interval_ms(5000)
            .claim_poll_limit(120)
            .build()
        )
        user_key = (
            fleet_sdk.CreateUserApiKeyRequestBuilder()
            .name("automation")
            .scope([])
            .build()
        )
        autoscaling = (
            fleet_sdk.WarmPoolAutoscalingBuilder()
            .min_pool_size(1)
            .initial_pool_size(2)
            .max_pool_size(5)
            .build()
        )

        self.assertEqual(configuration.pool_poll_interval_ms, 5000)
        self.assertEqual(user_key.scope, [])
        self.assertEqual(autoscaling.max_pool_size, 5)
        with self.assertRaises(fleet_sdk.SdkBuildError.MissingRequiredField):
            fleet_sdk.TemplateBuilder().build()
        with self.assertRaises(fleet_sdk.SdkBuildError.MissingRequiredField):
            fleet_sdk.CreateClaimRequestBuilder().build()


if __name__ == "__main__":
    unittest.main()
