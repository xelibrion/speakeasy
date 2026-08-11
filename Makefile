.PHONY: help build check test fmt fmt-check lint dev bundle bundle-mac clean boundary

# Must match `bundle.macOS.minimumSystemVersion` in tauri.conf.json. The plist
# key only tells Finder what to refuse; this is what the Rust and whisper.cpp
# objects are actually compiled against, and a mismatch fails at dyld time on a
# Mac older than the build machine.
MACOS_DEPLOYMENT_TARGET := 13.0

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

bundle-mac: ## Produce a signed universal .app + .dmg for distribution
	@if [ "$$(uname -s)" != "Darwin" ]; then \
		echo "ERROR: bundle-mac only runs on macOS. Use 'make bundle' instead."; \
		exit 1; \
	fi
	@for target in aarch64-apple-darwin x86_64-apple-darwin; do \
		if ! rustup target list --installed | grep -qx "$$target"; then \
			echo "ERROR: missing rust target $$target."; \
			echo "  rustup target add aarch64-apple-darwin x86_64-apple-darwin"; \
			exit 1; \
		fi; \
	done
	@# Unsigned bundles still run after clearing quarantine, but macOS keys TCC
	@# grants to the code signature: without a stable Developer ID the user
	@# re-grants Accessibility — which is what the text sink needs — on every
	@# update, and has to clear the stale entry by hand first.
	@if [ -z "$$APPLE_SIGNING_IDENTITY" ]; then \
		echo "warning: APPLE_SIGNING_IDENTITY unset — building unsigned."; \
		echo "warning: the recipient must clear quarantine and will re-grant"; \
		echo "warning: Accessibility after every update. Fine for your own"; \
		echo "warning: second machine, not for anyone else's."; \
	fi
	cd apps/desktop && MACOSX_DEPLOYMENT_TARGET=$(MACOS_DEPLOYMENT_TARGET) \
		npm run tauri build -- \
			--target universal-apple-darwin \
			--bundles app,dmg
	@echo
	@echo "bundle: target/universal-apple-darwin/release/bundle/"

clean:
	cargo clean
	rm -rf apps/desktop/node_modules apps/desktop/dist
