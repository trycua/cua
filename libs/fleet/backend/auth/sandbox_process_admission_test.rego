package sandbox_process_admission_test

import data.sandbox_process_admission

template_path := "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"

write(method, vm_template) := {
	"method": method,
	"params": {"path": template_path},
	"body": json.marshal({"spec": {"vmTemplate": vm_template}}),
}

post(vm_template) := write("POST", vm_template)

test_unrelated_request_allowed {
	sandbox_process_admission.allow with input as {
		"method": "POST",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"},
		"body": "not json",
	}
}

test_plain_template_allowed {
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img:1"})
}

test_gvisor_command_args_and_env_allowed {
	sandbox_process_admission.allow with input as post({
		"containerDiskImage": "python:3.12-slim",
		"runtime": "gvisor",
		"command": ["python", "-m", "server"],
		"args": ["--port", "8765"],
		"env": {"FOO": "bar", "_X1": "multi\nline is fine in a pod"},
	})
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "macos", "env": {"A": "b"}})
}

test_kubevirt_command_still_allowed {
	# command predates env/args; existing KubeVirt templates that set
	# it (ignored on KubeVirt) must keep passing.
	sandbox_process_admission.allow with input as post({
		"containerDiskImage": "ghcr.io/trycua/linux:24.04-disk",
		"command": ["/usr/bin/python3", "-m", "http.server"],
	})
	sandbox_process_admission.allow with input as write("PATCH", {"command": ["/bin/true"]})
}

test_kubevirt_env_and_args_denied {
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "args": ["--x"]})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "command": ["/x"], "args": ["--x"]})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "env": {"A": "b"}})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "kubevirt", "env": {"A": "b"}})
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "args": ["--x"]})
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "env": {}, "args": []})
}

test_bad_env_names_denied {
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "env": {"1A": "x"}})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "env": {"A-B": "x"}})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "env": {"A": 1}})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "env": ["A=b"]})
}

test_malformed_body_denied {
	not sandbox_process_admission.allow with input as {"method": "POST", "params": {"path": template_path}, "body": "not json"}
}

test_kubevirt_run_allows_command_args_and_env {
	sandbox_process_admission.allow with input as post({
		"containerDiskImage": "ghcr.io/trycua/linux:24.04-disk",
		"processMode": "Run",
		"command": ["python3", "-m", "http.server"],
		"args": ["$(PORT)"],
		"env": {"PORT": "8765", "GREETING": "hi there"},
	})
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "kubevirt", "processMode": "Run", "env": {"A": "b"}})
	# PATCH: runtime absent reads as kubevirt, so processMode must be stated.
	sandbox_process_admission.allow with input as write("PATCH", {"processMode": "Run", "env": {"A": "b"}})
	not sandbox_process_admission.allow with input as write("PATCH", {"env": {"A": "b"}})
}

test_kubevirt_without_run_refuses_env_and_args_with_the_reason {
	msgs := sandbox_process_admission.violation with input as post({"containerDiskImage": "img", "processMode": "Legacy", "env": {"A": "b"}})
	msgs["vmTemplate.env and args on runtime kubevirt need vmTemplate.processMode: Run (without it KubeVirt never runs command, args or env)"]
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Legacy", "args": ["x"], "command": ["y"]})
}

test_kubevirt_run_refusals {
	# A VM image has no entrypoint for bare args.
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Run", "args": ["--x"]})
	# Values go into guest files: single-line only.
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Run", "env": {"A": "two\nlines"}})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Run", "command": ["/bin/echo", "a\rb"]})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Run", "command": ["/x"], "args": ["a\u007fb"]})
	# Tabs and multi-line values stay fine on pods.
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "runtime": "gvisor", "processMode": "Run", "env": {"A": "two\nlines"}, "args": ["--x"]})
}

test_process_mode_values {
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Legacy"})
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Run"})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "run"})
	not sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Always"})
}

test_legacy_kubevirt_command_still_allowed_with_explicit_legacy {
	sandbox_process_admission.allow with input as post({"containerDiskImage": "img", "processMode": "Legacy", "command": ["/bin/true"]})
}
