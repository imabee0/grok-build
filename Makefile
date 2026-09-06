# bcode - see AGENTS.md
.PHONY: help verify brand build check rebrand sync fmt clean-backup

BIN := bcode-pager-bin

help:
	@grep -hE '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | column -t -s "$$(printf '\t')"

brand: ## Enforce the zero-branding guarantee
	@tools/verify-no-upstream-brand.sh

fmt: ## Check formatting
	@cargo fmt --all -- --check

check: ## Type-check the composition root
	@cargo check -p $(BIN)

verify: brand fmt check ## The ship gate: run this locally before any push
	@echo "verify: ok"

build: ## Release binary
	@cargo build -p $(BIN) --release

rebrand: ## Regenerate the rebrand in the working tree (codemod + fmt)
	@python3 tools/rebrand.py && cargo fmt --all

sync: ## Pull a new upstream snapshot and rebuild main on top of it
	@tools/sync-upstream.sh

clean-backup: ## Drop the safety branch left by `make sync`
	@git branch -D main-backup
