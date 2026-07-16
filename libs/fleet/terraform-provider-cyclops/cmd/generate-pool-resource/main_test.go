package main

import (
	"bytes"
	"os"
	"os/exec"
	"strings"
	"testing"
)

func TestValidateMappingAcceptsValidMapping(t *testing.T) {
	stderr, err := runValidationHelper(t, "valid")
	if err != nil {
		t.Fatalf("validateMapping() failed: %v\nstderr:\n%s", err, stderr)
	}
}

func TestValidateMappingRejectsInvalidMapping(t *testing.T) {
	tests := []struct {
		name       string
		scenario   string
		wantStderr string
	}{
		{name: "unsupported value type", scenario: "unsupported_value_type", wantStderr: `field "name" has unsupported value_type "bool"`},
		{name: "unsupported mode", scenario: "unsupported_mode", wantStderr: `field "name" has unsupported mode "write_only"`},
		{name: "requires replace on non-string", scenario: "requires_replace_int64", wantStderr: `field "replicas" sets requires_replace with value_type "int64"`},
		{name: "unsupported collection", scenario: "unsupported_collection", wantStderr: `block "service" has unsupported collection "list"`},
		{name: "duplicate top-level name", scenario: "duplicate_top_level_name", wantStderr: `duplicate top-level Terraform name "name"`},
		{name: "duplicate top-level Go name", scenario: "duplicate_top_level_go_name", wantStderr: `duplicate top-level Go name "Name"`},
		{name: "duplicate block name", scenario: "duplicate_block_name", wantStderr: `duplicate top-level Terraform name "service"`},
		{name: "duplicate nested name", scenario: "duplicate_nested_name", wantStderr: `duplicate Terraform field "name" in block "service"`},
		{name: "duplicate nested Go name", scenario: "duplicate_nested_go_name", wantStderr: `duplicate Go field "Name" in block "service"`},
		{name: "missing CRD path", scenario: "missing_crd_path", wantStderr: `spec.missing is not an object`},
		{name: "wrong CRD type", scenario: "wrong_crd_type", wantStderr: `field replicas maps to CRD type "string", expected "integer"`},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			stderr, err := runValidationHelper(t, test.scenario)
			if err == nil {
				t.Fatalf("validateMapping() succeeded, want failure")
			}
			if !strings.Contains(stderr, test.wantStderr) {
				t.Fatalf("stderr = %q, want it to contain %q", stderr, test.wantStderr)
			}
		})
	}
}

func TestRenderIsDeterministic(t *testing.T) {
	root, config := validationFixture("valid")
	first := render(root, config)
	second := render(root, config)
	if !bytes.Equal(first, second) {
		t.Fatal("render() produced different output for identical input")
	}
}

func TestValidationHelperProcess(t *testing.T) {
	scenario := os.Getenv("GO_GENERATOR_VALIDATION_SCENARIO")
	if scenario == "" {
		return
	}
	root, config := validationFixture(scenario)
	validateMapping(root, config)
}

func runValidationHelper(t *testing.T, scenario string) (string, error) {
	t.Helper()
	command := exec.Command(os.Args[0], "-test.run=^TestValidationHelperProcess$")
	command.Env = append(os.Environ(), "GO_GENERATOR_VALIDATION_SCENARIO="+scenario)
	output, err := command.CombinedOutput()
	return string(output), err
}

func validationFixture(scenario string) (map[string]any, mapping) {
	root := map[string]any{
		"properties": map[string]any{
			"spec": map[string]any{
				"type": "object",
				"properties": map[string]any{
					"replicas": map[string]any{"type": "integer"},
					"services": map[string]any{
						"type": "array",
						"items": map[string]any{
							"type": "object",
							"properties": map[string]any{
								"name":       map[string]any{"type": "string"},
								"targetPort": map[string]any{"type": "integer"},
							},
						},
					},
				},
			},
		},
	}
	config := mapping{
		Attributes: []field{
			{Name: "name", GoName: "Name", ValueType: "string", Mode: "required", RequiresReplace: true},
			{Name: "replicas", GoName: "Replicas", ValueType: "int64", Mode: "required", CRDPath: "spec.replicas"},
		},
		Blocks: []block{{
			Name: "service", GoName: "Services", Model: "serviceModel", Collection: "set", CRDPath: "spec.services",
			Fields: []field{{Name: "name", GoName: "Name", ValueType: "string", Mode: "required", CRDPath: "name"}},
		}},
	}

	switch scenario {
	case "valid":
	case "unsupported_value_type":
		config.Attributes[0].ValueType = "bool"
	case "unsupported_mode":
		config.Attributes[0].Mode = "write_only"
	case "requires_replace_int64":
		config.Attributes[1].RequiresReplace = true
	case "unsupported_collection":
		config.Blocks[0].Collection = "list"
	case "duplicate_top_level_name":
		config.Blocks[0].Name = "name"
	case "duplicate_top_level_go_name":
		config.Blocks[0].GoName = "Name"
	case "duplicate_block_name":
		duplicate := config.Blocks[0]
		duplicate.GoName = "OtherServices"
		duplicate.Model = "otherServiceModel"
		config.Blocks = append(config.Blocks, duplicate)
	case "duplicate_nested_name":
		config.Blocks[0].Fields = append(config.Blocks[0].Fields, field{Name: "name", GoName: "OtherName", ValueType: "string", Mode: "required", CRDPath: "name"})
	case "duplicate_nested_go_name":
		config.Blocks[0].Fields = append(config.Blocks[0].Fields, field{Name: "target_port", GoName: "Name", ValueType: "int64", Mode: "required", CRDPath: "targetPort"})
	case "missing_crd_path":
		config.Attributes[1].CRDPath = "spec.missing"
	case "wrong_crd_type":
		root["properties"].(map[string]any)["spec"].(map[string]any)["properties"].(map[string]any)["replicas"].(map[string]any)["type"] = "string"
	default:
		panic("unknown validation scenario: " + scenario)
	}
	return root, config
}
