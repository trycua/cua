import inspect
import unittest

import cyclops_sdk
import httpx


class PublicApiTest(unittest.TestCase):
    def test_typed_resource_api_only(self):
        self.assertEqual(list(inspect.signature(cyclops_sdk.connect).parameters), ["configuration"])
        expected = {
            "create_pool": ["self", "request"],
            "list_pools": ["self", "namespace"],
            "get_pool": ["self", "pool"],
            "update_pool": ["self", "pool"],
            "delete_pool": ["self", "pool"],
            "create_claim": ["self", "request"],
            "list_claims": ["self", "namespace"],
            "get_claim": ["self", "claim"],
            "update_claim": ["self", "claim"],
            "delete_claim": ["self", "claim"],
            "wait_claim": ["self", "claim"],
            "service_client": ["self", "sandbox", "service"],
            "close": ["self"],
        }
        public = {
            name: value
            for name, value in inspect.getmembers(cyclops_sdk.SDK, inspect.isfunction)
            if not name.startswith("_")
        }
        self.assertEqual(set(public), set(expected))
        for name, parameters in expected.items():
            self.assertEqual(list(inspect.signature(public[name]).parameters), parameters)
        expected_annotations = {
            "create_pool": ({"request": cyclops_sdk.CreatePoolRequest}, cyclops_sdk.Pool),
            "list_pools": ({"namespace": str}, list[cyclops_sdk.Pool]),
            "get_pool": ({"pool": cyclops_sdk.Pool}, cyclops_sdk.Pool),
            "update_pool": ({"pool": cyclops_sdk.Pool}, cyclops_sdk.Pool),
            "delete_pool": ({"pool": cyclops_sdk.Pool}, None),
            "create_claim": ({"request": cyclops_sdk.CreateClaimRequest}, cyclops_sdk.Claim),
            "list_claims": ({"namespace": str}, list[cyclops_sdk.Claim]),
            "get_claim": ({"claim": cyclops_sdk.Claim}, cyclops_sdk.Claim),
            "update_claim": ({"claim": cyclops_sdk.Claim}, cyclops_sdk.Claim),
            "delete_claim": ({"claim": cyclops_sdk.Claim}, None),
            "wait_claim": ({"claim": cyclops_sdk.Claim}, cyclops_sdk.Sandbox),
            "service_client": ({"sandbox": cyclops_sdk.Sandbox, "service": str}, httpx.Client),
        }
        for name, (parameters, return_annotation) in expected_annotations.items():
            signature = inspect.signature(public[name])
            self.assertEqual(
                {parameter: signature.parameters[parameter].annotation for parameter in parameters},
                parameters,
            )
            self.assertEqual(signature.return_annotation, return_annotation)
        self.assertFalse(hasattr(cyclops_sdk, "RawClient"))
        self.assertTrue(hasattr(cyclops_sdk, "CreatePoolRequest"))
        self.assertTrue(hasattr(cyclops_sdk, "CreateClaimRequest"))
        self.assertTrue(hasattr(cyclops_sdk, "Pool"))
        self.assertTrue(hasattr(cyclops_sdk, "PoolSpec"))
        self.assertTrue(hasattr(cyclops_sdk, "Claim"))
        self.assertTrue(hasattr(cyclops_sdk, "ClaimSpec"))
        self.assertTrue(hasattr(cyclops_sdk, "Sandbox"))
        self.assertTrue(issubclass(cyclops_sdk.UnknownServiceError, ValueError))

    def test_service_client_sends_relative_request_through_core(self):
        sdk = object.__new__(cyclops_sdk.SDK)
        calls = []

        def call(request):
            calls.append(request)
            return {"status": 201, "body": "initialized"}

        sdk._call = call
        sandbox = {
            "namespace": "pool-one",
            "claim": "claim-one",
            "sandbox": "sandbox-one",
            "services": ["mcp"],
        }
        with sdk.service_client(sandbox, "mcp") as client:
            response = client.post("/mcp?session=one", json={"jsonrpc": "2.0"})
        self.assertEqual(response.status_code, 201)
        self.assertEqual(response.text, "initialized")
        self.assertEqual(calls[0]["op"], "service_request")
        self.assertEqual(calls[0]["service"], "mcp")
        self.assertEqual(calls[0]["request"]["path"], "/mcp?session=one")
        self.assertNotIn("url", calls[0]["request"])

    def test_unknown_service_error_lists_available_names(self):
        sdk = object.__new__(cyclops_sdk.SDK)
        sandbox = {
            "namespace": "pool-one",
            "claim": "claim-one",
            "sandbox": "sandbox-one",
            "services": ["novnc", "mcp", "mcp"],
        }
        with self.assertRaises(cyclops_sdk.UnknownServiceError) as raised:
            sdk.service_client(sandbox, "metrics")
        self.assertEqual(raised.exception.requested, "metrics")
        self.assertEqual(raised.exception.available, ["mcp", "novnc"])


if __name__ == "__main__":
    unittest.main()
