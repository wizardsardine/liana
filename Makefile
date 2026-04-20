# Convenience targets for building coincube alongside its Spark bridge.
#
# coincube-spark-bridge is a standalone Cargo workspace (see the root
# Cargo.toml `exclude` entry) because breez-sdk-spark and breez-sdk-liquid
# can't share a dep graph. `cargo run -p coincube-gui` therefore does NOT
# build the bridge, and at runtime the gui will log
# "Spark bridge unavailable" and render "Spark is not configured" until
# the bridge binary exists at its expected path. Use these targets
# instead of raw cargo invocations to keep both halves in sync.

CARGO ?= cargo
PROFILE ?= debug
BRIDGE_MANIFEST := coincube-spark-bridge/Cargo.toml

ifeq ($(PROFILE),release)
  CARGO_PROFILE_FLAG := --release
else ifeq ($(PROFILE),debug)
  CARGO_PROFILE_FLAG :=
else
  CARGO_PROFILE_FLAG := --profile $(PROFILE)
endif

.PHONY: all build bridge gui run release test clean

all: build

build: bridge gui

bridge:
	$(CARGO) build --manifest-path $(BRIDGE_MANIFEST) $(CARGO_PROFILE_FLAG)

gui:
	$(CARGO) build --package coincube-gui $(CARGO_PROFILE_FLAG)

run: bridge
	$(CARGO) run --package coincube-gui $(CARGO_PROFILE_FLAG)

release:
	$(MAKE) build PROFILE=release

test:
	$(CARGO) test
	$(CARGO) test --manifest-path $(BRIDGE_MANIFEST)

clean:
	$(CARGO) clean
	$(CARGO) clean --manifest-path $(BRIDGE_MANIFEST)
