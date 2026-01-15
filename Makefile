# Python Package Release Makefile
# Version bumps are managed via GitHub Actions workflows (see Development.md)
# This Makefile provides utility targets for checking versions and dry-run testing

.PHONY: help

help: ## Show this help message
	@echo "Python Package Release Utilities"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-25s %s\n", $$1, $$2}'
	@echo ""
	@echo "⚠️  For production version bumps, use GitHub Actions:"
	@echo "    https://github.com/trycua/cua/actions/workflows/release-bump-version.yml"

# Dry run targets (test without making changes)
dry-run-patch-%: ## Dry run for patch version bump (e.g., make dry-run-patch-core)
	@echo "Dry run: Bumping $* patch version..."
	cd libs/python/$* && bump2version --dry-run --verbose patch

dry-run-minor-%: ## Dry run for minor version bump (e.g., make dry-run-minor-core)
	@echo "Dry run: Bumping $* minor version..."
	cd libs/python/$* && bump2version --dry-run --verbose minor

dry-run-major-%: ## Dry run for major version bump (e.g., make dry-run-major-core)
	@echo "Dry run: Bumping $* major version..."
	cd libs/python/$* && bump2version --dry-run --verbose major

# Show current versions
show-versions: ## Show current versions of all packages
	@echo "Current Python package versions:"
	@echo "  cua-core:           $$(grep 'current_version' libs/python/core/.bumpversion.cfg | cut -d' ' -f3)"
	@echo "  pylume:             $$(grep 'current_version' libs/python/pylume/.bumpversion.cfg | cut -d' ' -f3)"
	@echo "  cua-computer:       $$(grep 'current_version' libs/python/computer/.bumpversion.cfg | cut -d' ' -f3)"
	@echo "  cua-som:            $$(grep 'current_version' libs/python/som/.bumpversion.cfg | cut -d' ' -f3)"
	@echo "  cua-agent:          $$(grep 'current_version' libs/python/agent/.bumpversion.cfg | cut -d' ' -f3)"
	@echo "  cua-computer-server: $$(grep 'current_version' libs/python/computer-server/.bumpversion.cfg | cut -d' ' -f3)"
