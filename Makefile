# Release utilities
# Python and npm packages release through Release Please (see
# .github/releases/README.md); this Makefile only shows current versions.

.PHONY: help show-versions

help: ## Show this help message
	@echo "Release utilities"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-25s %s\n", $$1, $$2}'
	@echo ""
	@echo "Release Please opens a release PR per package from conventional commit titles."

# Show current versions
show-versions: ## Show the released version of every Release Please package
	@python3 -c "import json; [print(f'  {p:45} {v}') for p, v in json.load(open('.release-please-manifest.json')).items()]"
