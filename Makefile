# Python Package Release Makefile
# This Makefile provides convenient targets for bumping versions of all Python packages
# using bump2version. After running a target, remember to push: git push origin main

.PHONY: help

help: ## Show this help message
	@echo "Python Package Release Automation"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-25s %s\n", $$1, $$2}'
	@echo ""
	@echo "After bumping, push changes with: git push origin main"

# Core package targets
bump-patch-core: ## Bump patch version of cua-core (0.1.8 → 0.1.9)
	@echo "Bumping cua-core patch version..."
	cd libs/python/core && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-core: ## Bump minor version of cua-core (0.1.8 → 0.2.0)
	@echo "Bumping cua-core minor version..."
	cd libs/python/core && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-core: ## Bump major version of cua-core (0.1.8 → 1.0.0)
	@echo "Bumping cua-core major version..."
	cd libs/python/core && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# Pylume package targets
bump-patch-pylume: ## Bump patch version of pylume (0.2.2 → 0.2.3)
	@echo "Bumping pylume patch version..."
	cd libs/python/pylume && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-pylume: ## Bump minor version of pylume (0.2.2 → 0.3.0)
	@echo "Bumping pylume minor version..."
	cd libs/python/pylume && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-pylume: ## Bump major version of pylume (0.2.2 → 1.0.0)
	@echo "Bumping pylume major version..."
	cd libs/python/pylume && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# Computer package targets
bump-patch-computer: ## Bump patch version of cua-computer (0.4.0 → 0.4.1)
	@echo "Bumping cua-computer patch version..."
	cd libs/python/computer && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-computer: ## Bump minor version of cua-computer (0.4.0 → 0.5.0)
	@echo "Bumping cua-computer minor version..."
	cd libs/python/computer && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-computer: ## Bump major version of cua-computer (0.4.0 → 1.0.0)
	@echo "Bumping cua-computer major version..."
	cd libs/python/computer && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# SOM package targets
bump-patch-som: ## Bump patch version of cua-som (0.1.0 → 0.1.1)
	@echo "Bumping cua-som patch version..."
	cd libs/python/som && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-som: ## Bump minor version of cua-som (0.1.0 → 0.2.0)
	@echo "Bumping cua-som minor version..."
	cd libs/python/som && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-som: ## Bump major version of cua-som (0.1.0 → 1.0.0)
	@echo "Bumping cua-som major version..."
	cd libs/python/som && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# Agent package targets
bump-patch-agent: ## Bump patch version of cua-agent (0.4.0 → 0.4.1)
	@echo "Bumping cua-agent patch version..."
	cd libs/python/agent && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-agent: ## Bump minor version of cua-agent (0.4.0 → 0.5.0)
	@echo "Bumping cua-agent minor version..."
	cd libs/python/agent && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-agent: ## Bump major version of cua-agent (0.4.0 → 1.0.0)
	@echo "Bumping cua-agent major version..."
	cd libs/python/agent && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# Computer Server package targets
bump-patch-computer-server: ## Bump patch version of cua-computer-server (0.1.0 → 0.1.1)
	@echo "Bumping cua-computer-server patch version..."
	cd libs/python/computer-server && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-computer-server: ## Bump minor version of cua-computer-server (0.1.0 → 0.2.0)
	@echo "Bumping cua-computer-server minor version..."
	cd libs/python/computer-server && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-computer-server: ## Bump major version of cua-computer-server (0.1.0 → 1.0.0)
	@echo "Bumping cua-computer-server major version..."
	cd libs/python/computer-server && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# MCP Server package targets
bump-patch-mcp-server: ## Bump patch version of cua-mcp-server (0.1.0 → 0.1.1)
	@echo "Bumping cua-mcp-server patch version..."
	cd libs/python/mcp-server && bump2version patch
	@echo "✓ Done! Now run: git push origin main"

bump-minor-mcp-server: ## Bump minor version of cua-mcp-server (0.1.0 → 0.2.0)
	@echo "Bumping cua-mcp-server minor version..."
	cd libs/python/mcp-server && bump2version minor
	@echo "✓ Done! Now run: git push origin main"

bump-major-mcp-server: ## Bump major version of cua-mcp-server (0.1.0 → 1.0.0)
	@echo "Bumping cua-mcp-server major version..."
	cd libs/python/mcp-server && bump2version major
	@echo "✓ Done! Now run: git push origin main"

# Convenience targets for common workflows
bump-all-patch: ## Bump patch version for ALL packages (use with caution!)
	@echo "⚠️  Bumping patch version for ALL packages..."
	@read -p "Are you sure? [y/N] " -n 1 -r; \
	echo; \
	if [[ $$REPLY =~ ^[Yy]$$ ]]; then \
		$(MAKE) bump-patch-core && \
		$(MAKE) bump-patch-pylume && \
		$(MAKE) bump-patch-computer && \
		$(MAKE) bump-patch-som && \
		$(MAKE) bump-patch-agent && \
		$(MAKE) bump-patch-computer-server && \
		$(MAKE) bump-patch-mcp-server; \
	fi

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
	@echo "  cua-mcp-server:     $$(grep 'current_version' libs/python/mcp-server/.bumpversion.cfg | cut -d' ' -f3)"
