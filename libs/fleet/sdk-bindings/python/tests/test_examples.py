from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[3]
EXAMPLES = {
    "python": ROOT / "sdk-bindings/examples/python/app_controlled.py",
    "typescript": ROOT / "sdk-bindings/examples/typescript/app-controlled.ts",
    "go": ROOT / "sdk-bindings/go/examples/app_controlled.go",
    "rust": ROOT / "sdk-bindings/rust/examples/app_controlled.rs",
}


class ExampleContractTest(unittest.TestCase):
    def test_examples_use_app_controlled_sequence(self):
        sequences = {
            "python": ["connect(", "create_pool(", "create_claim(", "wait_claim(", "run_agent(sdk, sandbox)", "print(", "finally:", "delete_claim(", "delete_pool(", "close("],
            "typescript": ["connect(", "createPool(", "createClaim(", "waitClaim(", "runAgent(sdk, sandbox)", "console.log(", "finally {", "deleteClaim(", "deletePool(", "close("],
            "go": ["Connect(", "sdk.Close()", "CreatePool(", "sdk.DeletePool(pool)", "CreateClaim(", "sdk.DeleteClaim(claim)", "WaitClaim(", "runAgent(sdk, sandbox)", "fmt.Println("],
            "rust": ["Sdk::connect(", "create_pool(", "create_claim(", "wait_claim(", "run_agent(&mut sdk, sandbox)", "println!(", "delete_claim(", "delete_pool(", "close("],
        }
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            flow = source[source.index("def main"):] if language == "python" else source[source.index("const sdk"):] if language == "typescript" else source[source.index("func main"):] if language == "go" else source[source.index("fn main"):]
            positions = [flow.index(token) for token in sequences[language]]
            self.assertEqual(positions, sorted(positions), language)
            self.assertIn("initialize", source, language)
            self.assertRegex(source, r"service_client|serviceFetch|ServiceClient", language)
            self.assertIn("/mcp", source, language)
            self.assertNotIn("external run_agent", source.lower(), language)

    def test_examples_emit_machine_readable_resource_results(self):
        tokens = {
            "python": ['sandbox["claim"]', "json.dumps(result"],
            "typescript": ["sandbox.claim", "JSON.stringify(result)"],
            "go": ["sandbox.Claim", "json.Marshal(result)"],
            "rust": ['"claim": sandbox.claim', 'println!("{result}")'],
        }
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            for token in tokens[language]:
                self.assertIn(token, source, language)


    def test_examples_create_mcp_service_alias(self):
        tokens = {
            "python": ['"services": [{"name": "mcp", "targetPort": 3000, "protocol": "TCP"}]'],
            "typescript": ['services: [{ name: "mcp", targetPort: 3000, protocol: "TCP" }]'],
            "go": ['Services: []cyclopssdk.PoolSpecService{', 'Name: "mcp"', 'TargetPort: 3000'],
            "rust": ['PoolSpecService::builder()', '.name("mcp".into())', '.target_port(3000)'],
        }
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            for token in tokens[language]:
                self.assertIn(token, source, language)

    def test_examples_retry_transient_mcp_gateway_errors(self):
        tokens = {
            "python": ["time.monotonic()", "(502, 503, 504)", "time.sleep("],
            "typescript": ["Date.now()", "[502, 503, 504]", "setTimeout"],
            "go": ["time.Now()", "response.StatusCode == 502", "time.Sleep("],
            "rust": ["Instant::now()", "matches!(response.status, 502..=504)", "thread::sleep("],
        }
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            for token in tokens[language]:
                self.assertIn(token, source, language)

    def test_examples_use_named_service_clients_with_relative_paths(self):
        tokens = {
            "python": ['service_client(sandbox, "mcp")', "mcp.post(", '"/mcp"'],
            "typescript": ['serviceFetch(sandbox, "mcp")', 'mcp("/mcp"'],
            "go": ['ServiceClient(sandbox, "mcp")', 'NewRequest("POST", "/mcp"'],
            "rust": ['service_client(&sandbox, "mcp")', '.post("/mcp")'],
        }
        prohibited = ["mcp_url", "MCPURL", "mcpUrl", "-mcp"]
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            for token in tokens[language]:
                self.assertIn(token, source, language)
            for token in prohibited:
                self.assertNotIn(token, source, language)

    def test_bindings_do_not_expose_url_based_http_clients(self):
        binding_files = [
            ROOT / "sdk-bindings/python/cyclops_sdk/__init__.py",
            ROOT / "sdk-bindings/typescript/src/index.ts",
            ROOT / "sdk-bindings/go/sdk.go",
            ROOT / "sdk-bindings/rust/src/lib.rs",
            ROOT / "sdk-core/wit/cyclops-sdk.wit",
        ]
        prohibited = ["raw_client", "rawClient", "RawClient", "raw-request", '"raw_request"']
        for path in binding_files:
            source = path.read_text(encoding="utf-8")
            for token in prohibited:
                self.assertNotIn(token, source, path)

    def test_private_image_examples_pass_pull_secret(self):
        tokens = {
            "python": ['os.environ["CUA_IMAGE_PULL_SECRET"]', '"imagePullSecret"'],
            "typescript": ['required("CUA_IMAGE_PULL_SECRET")', 'imagePullSecret'],
            "go": ['required("CUA_IMAGE_PULL_SECRET")', 'ImagePullSecret'],
            "rust": ['required("CUA_IMAGE_PULL_SECRET")', '.image_pull_secret('],
        }
        for language, path in EXAMPLES.items():
            source = path.read_text(encoding="utf-8")
            for token in tokens[language]:
                self.assertIn(token, source, language)


if __name__ == "__main__":
    unittest.main()
