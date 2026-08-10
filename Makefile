.PHONY: help build check test fmt fmt-check lint dev bundle clean boundary

help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Build the whole workspace
	cargo build --workspace

check: fmt-check lint test boundary ## Everything CI runs

test: ## Run workspace tests
	cargo test --workspace

fmt: ## Format Rust sources
	cargo fmt --all

fmt-check: ## Verify formatting
	cargo fmt --all -- --check

lint: ## Clippy, warnings denied
	cargo clippy --workspace --all-targets -- -D warnings

boundary: ## Fail if speakeasy-core ever gains a tauri dependency
	@if cargo tree -p speakeasy-core -e normal 2>/dev/null | grep -qi '^\s*[|`-]*\s*tauri'; then \
		echo "ERROR: speakeasy-core depends on tauri. The core must stay shell-agnostic."; \
		exit 1; \
	else \
		echo "ok: speakeasy-core is free of tauri"; \
	fi

dev: ## Run the Tauri app in dev mode
	cd apps/desktop && npm run tauri dev

bundle: ## Produce a release bundle
	cd apps/desktop && npm run tauri build

clean:
	cargo clean
	rm -rf apps/desktop/node_modules apps/desktop/dist
