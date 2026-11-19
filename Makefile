BIN := modmed
CARGO ?= cargo

.PHONY: build release test lint fmt fmt-check clean install deps smoke verify dev mock

# Every target that produces a binary signs it. `cargo build` and
# `cargo install` ad-hoc sign, minting a new code identity each time, and
# macOS scopes keychain "Always Allow" grants to that identity: an unsigned
# rebuild silently revokes the grant and the next run prompts again.
build: SIGN_TARGET = target/debug/$(BIN)
build:
	$(CARGO) build
	@$(SIGN)

release: SIGN_TARGET = target/release/$(BIN)
release:
	$(CARGO) build --release
	@$(SIGN)

test:
	$(CARGO) test

lint:
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

clean:
	$(CARGO) clean

deps:
	$(CARGO) fetch

# `--locked` builds from Cargo.lock, the versions the tests and audit ran on.
install: SIGN_TARGET = $${CARGO_INSTALL_ROOT:-$$HOME/.cargo}/bin/$(BIN)
install:
	$(CARGO) install --path . --locked --force
	@$(SIGN)

dev: SIGN_TARGET = target/debug/$(BIN)
dev:
	$(CARGO) build
	@$(SIGN)

# A local stand-in for a ModMed practice (SMART + FHIR) serving one
# synthetic patient, and ModMed's directory serving synthetic practices, for
# dogfooding. CONTRIBUTING.md shows how to point modmed at them.
mock:
	$(CARGO) run --example mock_modmed

# Offline smoke test on the release binary. It never reads the real keychain
# or the network: MODMED_KEYCHAIN_SERVICE names a service that holds
# nothing, and the config, the vault and the directory cache live under
# target/smoke.
SMOKE_DIR := $(CURDIR)/target/smoke
SMOKE := MODMED_KEYCHAIN_SERVICE=piekstra.modmed.hermetic-tests \
	XDG_CONFIG_HOME=$(SMOKE_DIR)/config \
	MODMED_CACHE_DIR=$(SMOKE_DIR)/cache \
	./target/release/$(BIN)
smoke: release
	@$(SMOKE) --version >/dev/null
	@$(SMOKE) info --json >/dev/null
	@$(SMOKE) completions bash >/dev/null
	@$(SMOKE) orgs list --state XYZ >/dev/null 2>&1; test $$? -eq 2 || { echo "orgs list took a bad state"; exit 1; }
	@$(SMOKE) auth status >/dev/null 2>&1; test $$? -eq 2 || { echo "auth status ran without --profile"; exit 1; }
	@$(SMOKE) profiles add smoke >/dev/null
	@$(SMOKE) auth status --profile smoke --json >/dev/null
	@$(SMOKE) patients list --profile smoke --json >/dev/null
	@$(SMOKE) profiles remove smoke --force >/dev/null
	@for c in "orgs refresh" "orgs list" "orgs search" "profiles list" "profiles add" \
		"profiles remove" "auth set-credential" "auth login" \
		"auth status" "auth logout" "patients list" "records export" \
		"documents list" "documents download" "api" \
		"config set" "config show"; do \
		$(SMOKE) $$c --help >/dev/null || exit 1; done
	@echo "smoke ok"

verify: fmt-check lint test smoke

define SIGN
if [ -x "$$HOME/Dev/cli-common/scripts/dev-sign.sh" ]; then \
	"$$HOME/Dev/cli-common/scripts/dev-sign.sh" "$(SIGN_TARGET)"; \
else echo "cli-common/scripts/dev-sign.sh not found — $(SIGN_TARGET) left ad-hoc signed"; fi
endef
