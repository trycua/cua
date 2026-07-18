package main

import (
	"strings"
	"testing"
)

func TestRenderRustGeneratesBuilders(t *testing.T) {
	roots := []*rootType{{
		Name: "ExampleSpec",
		Node: &node{
			Name: "ExampleSpec",
			Type: "object",
			Properties: []*node{
				{Name: "ExampleSpecName", JSONName: "name", Type: "string", Required: true},
				{Name: "ExampleSpecCount", JSONName: "count", Type: "integer", Required: false},
			},
		},
	}}

	source := string(renderRust(roots))
	for _, expected := range []string{
		"pub fn builder() -> ExampleSpecBuilder",
		"pub struct ExampleSpecBuilder",
		"name: Option<String>",
		"count: Option<u32>",
		"pub fn name(mut self, value: String) -> Self",
		"pub fn count(mut self, value: u32) -> Self",
		`ok_or_else(|| "name is required".to_string())?`,
		"count: self.count",
	} {
		if !strings.Contains(source, expected) {
			t.Fatalf("generated Rust missing %q:\n%s", expected, source)
		}
	}
}
