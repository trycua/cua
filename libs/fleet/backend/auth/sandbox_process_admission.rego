# Admission over what a sandbox template asks its sandboxes to run:
# spec.vmTemplate.{command,args,env,processMode} on
# OSGymSandboxTemplate writes.
#
# command, args and env run on every runtime once vmTemplate.processMode is
# Run: pod runtimes (gvisor, macos) set them on the sandbox container, and
# KubeVirt renders them into the sandbox's cloud-init. Without processMode (or
# with Legacy) KubeVirt keeps its old behavior: command is ignored, and env
# and args are refused here with a message naming processMode: Run, instead of
# a sandbox that silently ignores them. command predates this module and no
# rule refuses it, so no existing template is newly refused. KubeVirt Run
# values must be single-line (they go into files in the guest), and args need
# a command there (a VM image has no entrypoint). Runtime absent means
# kubevirt (the CRD default) and
# processMode absent means Legacy, on PATCH too: state both alongside these
# fields. Env names follow the POSIX shape. The pool-operator
# (osgym/pool-operator/sandbox_process.py) enforces the same rules as a
# backstop.
#
# Same shape as the other body admissions: `applies` matches every template
# write, `allow { not applies }` passes the rest of the surface.
package sandbox_process_admission

default allow = false

env_name_pattern := `^[A-Za-z_][A-Za-z0-9_]*$`

pod_runtimes := {"gvisor", "macos"}

process_modes := {"Legacy", "Run"}

# Any C0 control character or DEL (newlines included).
control_char_pattern := `[\x00-\x1f\x7f]`

write_method {
	input.method == "POST"
}

write_method {
	input.method == "PUT"
}

write_method {
	input.method == "PATCH"
}

# apis/osgym.cua.ai/v1alpha1/namespaces/{ns}/osgymsandboxtemplates[/{name}]
template_path {
	parts := split(input.params.path, "/")
	count(parts) >= 6
	count(parts) <= 7
	parts[0] == "apis"
	parts[1] == "osgym.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymsandboxtemplates"
}

applies {
	write_method
	template_path
}

allow {
	not applies
}

request_object := json.unmarshal(input.body) {
	applies
}

vm_template := object.get(object.get(request_object, "spec", {}), "vmTemplate", {})

runtime := lower(object.get(vm_template, "runtime", "kubevirt"))

pod_runtime {
	pod_runtimes[runtime]
}

non_empty(key) {
	value := object.get(vm_template, key, null)
	value != null
	count(value) > 0
}

invalid_env(env) {
	env != null
	not is_object(env)
}

invalid_env(env) {
	is_object(env)
	some name
	env[name]
	not regex.match(env_name_pattern, name)
}

invalid_env(env) {
	is_object(env)
	value := env[_]
	not is_string(value)
}

violation["vmTemplate.env names must match ^[A-Za-z_][A-Za-z0-9_]*$ and values must be strings"] {
	invalid_env(object.get(vm_template, "env", null))
}

process_mode := object.get(vm_template, "processMode", "Legacy")

run_mode {
	process_mode == "Run"
}

violation["vmTemplate.processMode must be Legacy or Run"] {
	not process_modes[process_mode]
}

run_only_fields := ["env", "args"]

violation["vmTemplate.env and args on runtime kubevirt need vmTemplate.processMode: Run (without it KubeVirt never runs command, args or env)"] {
	not pod_runtime
	not run_mode
	non_empty(run_only_fields[_])
}

violation["vmTemplate.args needs vmTemplate.command on runtime kubevirt (a VM image has no entrypoint)"] {
	not pod_runtime
	run_mode
	non_empty("args")
	not non_empty("command")
}

violation["vmTemplate.env values and command/args must not contain control characters on runtime kubevirt"] {
	not pod_runtime
	run_mode
	env := object.get(vm_template, "env", null)
	is_object(env)
	value := env[_]
	is_string(value)
	regex.match(control_char_pattern, value)
}

violation["vmTemplate.env values and command/args must not contain control characters on runtime kubevirt"] {
	not pod_runtime
	run_mode
	field := ["command", "args"][_]
	values := object.get(vm_template, field, null)
	is_array(values)
	value := values[_]
	is_string(value)
	regex.match(control_char_pattern, value)
}

allow {
	applies
	is_object(request_object)
	count(violation) == 0
}
