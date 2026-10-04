# mudita build entrypoints. toolchains come from mise (see mise.toml), so every
# tool is invoked through `mise exec` for deterministic builds.

MISE := mise exec --
FLUTTER := $(MISE) flutter
DART := $(MISE) dart
CARGO := $(MISE) cargo
CLI := target/release/mudita
E2E := cd test/e2e && $(MISE) uv run

# the production web build is deployed at veilid.tech/mudita.
WEB_BASE_HREF ?= /mudita/

.DEFAULT_GOAL := build

DANA_MAIN := lib/dana/main.dart
DANA_BUNDLE := build/dana
BINDU_BUNDLE := build/bindu
# the app's own binary is already called mudita.
APP_SERVER := mudita-server

.PHONY: build build-cli build-web build-web-e2e build-linux build-dana run \
	run-dana deps wasm core-wasm \
	analyze fmt fmt-check test test-e2e screenshots precommit clean

build: build-cli build-web

build-cli:
	$(CARGO) build --release

# the web app is the flutter build plus the core it loads from wasm/.
build-web: deps core-wasm
	$(FLUTTER) build web --base-href $(WEB_BASE_HREF)

# served from / with the window.muditaTest hook compiled in.
build-web-e2e: deps core-wasm
	$(FLUTTER) build web --dart-define=MUDITA_E2E=true --output build/web-e2e

# the two linux apps: bindu (one file) and the window of dana (the library).
# they are one flutter project and one linux runner, so both build into the
# same bundle directory; each is copied out under its own, with the server
# it starts beside it under a name that is not the app's own.
build-linux: deps build-cli
	$(FLUTTER) build linux
	$(call bundle,$(BINDU_BUNDLE))

build-dana: deps build-cli
	$(FLUTTER) build linux -t $(DANA_MAIN)
	$(call bundle,$(DANA_BUNDLE))

define bundle
	rm -rf $(1)
	cp -r build/linux/x64/release/bundle $(1)
	cp $(CLI) $(1)/$(APP_SERVER)
endef

# from source there is no bundle, so the app is told where the server is.
run: deps build-cli
	MUDITA_SERVER=$(CURDIR)/$(CLI) $(FLUTTER) run -d linux

run-dana: deps build-cli
	MUDITA_SERVER=$(CURDIR)/$(CLI) \
		$(FLUTTER) run -d linux -t $(DANA_MAIN)

deps:
	$(FLUTTER) pub get

# the mudita core for a web page (the wasm/ crate), which is all of the
# network the web app has. slow the first time: it compiles veilid.
wasm core-wasm:
	$(MISE) bash scripts/build_core_wasm.sh

analyze:
	$(FLUTTER) analyze
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(E2E) ruff check .

fmt:
	$(DART) format lib test
	$(CARGO) fmt --all
	$(E2E) ruff format .

fmt-check:
	$(DART) format --output=none --set-exit-if-changed lib test
	$(CARGO) fmt --all --check
	$(E2E) ruff format --check .

# hermetic: dart unit + widget tests and rust unit tests.
test:
	$(FLUTTER) test
	$(CARGO) test --workspace

# transfers real files between the cli, the web app and the core in a page
# over the live veilid network, so it needs outbound network access and an
# installed chrome.
test-e2e: build-cli build-web-e2e core-wasm
	$(E2E) pytest

# the pictures of the library's window in README.md, drawn over a fake
# server into docs/screenshots.
screenshots: deps
	$(FLUTTER) test test/screenshots/dana.dart

precommit: fmt-check analyze test

clean:
	$(FLUTTER) clean
	$(CARGO) clean
