# Makefile for mirage — Rust binary + C++ Dolphin plugin
# Rust: cargo, C++: cmake (plugins/dolphin/ -> build-dolphin/)

CARGO        := cargo
CMAKE        := cmake
CMAKE_SRC    := plugins/dolphin
CMAKE_BUILD  := build-dolphin
INSTALL_BIN  := /usr/bin/mirage

.PHONY: all build debug install uninstall test clean

all: build

build:
	$(CARGO) build --release
	$(CMAKE) -S $(CMAKE_SRC) -B $(CMAKE_BUILD) \
		-DCMAKE_INSTALL_PREFIX=/usr \
		-DCMAKE_BUILD_TYPE=Release
	$(CMAKE) --build $(CMAKE_BUILD)

debug:
	$(CARGO) build
	$(CMAKE) -S $(CMAKE_SRC) -B $(CMAKE_BUILD) \
		-DCMAKE_INSTALL_PREFIX=/usr \
		-DCMAKE_BUILD_TYPE=Debug
	$(CMAKE) --build $(CMAKE_BUILD)

install: build
	sudo install -Dm755 target/release/mirage $(INSTALL_BIN)
	sudo $(CMAKE) --install $(CMAKE_BUILD)

uninstall:
	sudo rm -f $(INSTALL_BIN)
	@if [ -f $(CMAKE_BUILD)/install_manifest.txt ]; then \
		sudo xargs -d '\n' rm -f < $(CMAKE_BUILD)/install_manifest.txt; \
		echo "Plugins removed."; \
	else \
		echo "No install_manifest.txt found. Run 'make install' first."; \
	fi

test:
	$(CARGO) test
	$(CARGO) clippy
	$(CARGO) fmt -- --check

clean:
	$(CARGO) clean
	rm -rf $(CMAKE_BUILD)
