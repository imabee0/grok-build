# bcode - see AGENTS.md
.PHONY: help verify brand build check test egress coexist rebrand sync fmt clean-backup install uninstall

PKG := bcode-pager-bin
BIN := bcode

# Where `make install` puts things. The binary lives in bcode's own home, so an
# install owns nothing outside `~/.bcode`; LINKDIR gets a symlink to it, which is
# the only file written elsewhere and the only thing `make uninstall` removes
# from there.
BCODE_HOME ?= $(HOME)/.bcode
LINKDIR    ?= $(HOME)/.local/bin

# The crates this fork's feature stack touches. `make test` covers these; the
# whole workspace is 94 crates and takes far longer than a ship gate should.
TOUCHED := bcode-pricing bcode-models bcode-status-line bcode-chat-state \
           bcode-sampling-types bcode-sampler bcode-pager bcode-workspace \
           bcode-shell bcode-tools bcode-file-utils bcode-agent

help:
	@grep -hE '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | column -t -s "$$(printf '\t')"

brand: ## Enforce the zero-branding guarantee
	@tools/verify-no-upstream-brand.sh

fmt: ## Check formatting
	@cargo fmt --all -- --check

check: ## Type-check the composition root
	@cargo check -p $(PKG)

coexist: ## Assert other agent CLIs' state is untouched
	@cargo build -q -p $(PKG) && tools/verify-coexistence.sh

test: ## Tests for the crates this fork changes
	@cargo test $(addprefix -p ,$(TOUCHED))

egress: ## Assert a session with no provider selected reaches no network
	@cargo build -q -p $(PKG) && tools/verify-no-egress.sh target/debug/$(BIN)

verify: brand fmt check test ## The ship gate: run this locally before any push
	@echo "verify: ok"

build: ## Release binary
	@cargo build -p $(PKG) --release

install: build ## Install to $(BCODE_HOME)/bin and link it into $(LINKDIR)
	@mkdir -p $(BCODE_HOME)/bin $(LINKDIR)
	@install -m 755 target/release/$(BIN) $(BCODE_HOME)/bin/$(BIN).new
	@mv -f $(BCODE_HOME)/bin/$(BIN).new $(BCODE_HOME)/bin/$(BIN)
	@ln -sfn $(BCODE_HOME)/bin/$(BIN) $(LINKDIR)/$(BIN)
	@echo "installed $$($(BCODE_HOME)/bin/$(BIN) --version) -> $(BCODE_HOME)/bin/$(BIN)"
	@echo "linked    $(LINKDIR)/$(BIN)"
	@case ":$$PATH:" in *":$(LINKDIR):"*) ;; \
	  *) echo "note: $(LINKDIR) is not on PATH; add it to run \`$(BIN)\`" ;; esac

uninstall: ## Remove the installed binary and its link (leaves $(BCODE_HOME) data)
	@rm -f $(BCODE_HOME)/bin/$(BIN)
	@if [ -L $(LINKDIR)/$(BIN) ] && [ "$$(readlink $(LINKDIR)/$(BIN))" = "$(BCODE_HOME)/bin/$(BIN)" ]; then \
	  rm -f $(LINKDIR)/$(BIN); echo "removed $(LINKDIR)/$(BIN)"; fi
	@echo "removed $(BCODE_HOME)/bin/$(BIN) (sessions, auth and config under $(BCODE_HOME) are kept)"

rebrand: ## Regenerate the rebrand in the working tree (codemod + fmt)
	@python3 tools/rebrand.py && cargo fmt --all

sync: ## Pull a new upstream snapshot and rebuild main on top of it
	@tools/sync-upstream.sh

clean-backup: ## Drop the safety branch left by `make sync`
	@git branch -D main-backup
