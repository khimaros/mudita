# mudita build entrypoints. toolchains come from mise (see mise.toml), so every
# tool is invoked through `mise exec` for deterministic builds.

MISE := mise exec --
FLUTTER := $(MISE) flutter
DART := $(MISE) dart
CARGO := $(MISE) cargo
DANA := target/release/dana
E2E := cd test/e2e && $(MISE) uv run

# the production web build is deployed at veilid.tech/mudita.
WEB_BASE_HREF ?= /mudita/

.DEFAULT_GOAL := build

MUDITA_MAIN := lib/desktop/main.dart
MUDITA_ID := com.khimaros.mudita
BINDU_ID := com.khimaros.mudita.bindu

.PHONY: build build-dana build-bindu-web build-bindu-web-e2e build-bindu \
	build-mudita run-bindu run-mudita deps wasm core-wasm \
	analyze fmt fmt-check test test-e2e screenshots precommit clean

build: build-dana build-bindu-web

# the command line and the server: dana.
build-dana:
	$(CARGO) build --release

# the web app is the flutter build plus the core it loads from wasm/.
build-bindu-web: deps core-wasm
	$(FLUTTER) build web --base-href $(WEB_BASE_HREF)

# served from / with the window.muditaTest hook compiled in.
build-bindu-web-e2e: deps core-wasm
	$(FLUTTER) build web --dart-define=MUDITA_E2E=true --output build/web-e2e

# the two linux apps: bindu (one file) and mudita (the full desktop app).
# they are one flutter project and one linux runner, so both build into the
# same bundle directory; each is copied out under its own name, with dana,
# the server it starts, beside it.
build-bindu: deps build-dana
	MUDITA_APP_ID=$(BINDU_ID) $(FLUTTER) build linux
	$(call bundle,bindu)

build-mudita: deps build-dana
	MUDITA_APP_ID=$(MUDITA_ID) $(FLUTTER) build linux -t $(MUDITA_MAIN)
	$(call bundle,mudita)

define bundle
	rm -rf build/$(1)
	cp -r build/linux/x64/release/bundle build/$(1)
	mv build/$(1)/app build/$(1)/$(1)
	cp $(DANA) build/$(1)/dana
endef

# from source there is no bundle, so the app is told where the server is.
run-bindu: deps build-dana
	MUDITA_APP_ID=$(BINDU_ID) MUDITA_SERVER=$(CURDIR)/$(DANA) \
		$(FLUTTER) run -d linux

run-mudita: deps build-dana
	MUDITA_APP_ID=$(MUDITA_ID) MUDITA_SERVER=$(CURDIR)/$(DANA) \
		$(FLUTTER) run -d linux -t $(MUDITA_MAIN)

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
test-e2e: build-dana build-bindu-web-e2e core-wasm
	$(E2E) pytest

# the pictures of the desktop app in README.md, drawn over a fake server
# into docs/screenshots.
screenshots: deps
	$(FLUTTER) test test/screenshots/desktop.dart

precommit: fmt-check analyze test

clean:
	$(FLUTTER) clean
	$(CARGO) clean
