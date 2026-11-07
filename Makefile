.PHONY: all build test clean release help

all: build

build:
	python3 build.py

test:
	cargo test

clean:
	rm -rf build target

help:
	@echo "ThinkFlow Build System"
	@echo ""
	@echo "Targets:"
	@echo "  make build   - Run tests and build kext, SSDT, and CLI tool (build.py)"
	@echo "  make test    - Run Rust core unit tests (cargo test)"
	@echo "  make clean   - Remove build/ and target/ artifacts"

release:
	@if [ -z "$(VERSION)" ]; then echo "Usage: make release VERSION=1.0.2"; exit 1; fi
	./scripts/gitflow-release.sh "$(VERSION)"
