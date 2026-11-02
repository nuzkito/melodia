COMPOSE := docker compose run --rm

.PHONY: help build-dev build-release smoke test lint demo web

# The first target: `make` alone lists the targets and does not start a build.
help:
	@echo "make build-dev      quick Windows build to try changes (.exe and DLL only)"
	@echo "make build-release  optimized Windows build: installer, zip and SHA256SUMS.txt"
	@echo "make smoke          demo app on Xvfb, screenshots in dist/screenshots/"
	@echo "make test           all tests"
	@echo "make lint           rustfmt and clippy"
	@echo "make demo           app in demo mode (through WSLg)"
	@echo "make web            landing page and update server on 127.0.0.1:8897"

# Quick Windows build to try changes: only the .exe and the DLL (`fast` profile).
build-dev:
	$(COMPOSE) build-dev

# Optimized Windows build to publish: the installer, the zip and SHA256SUMS.txt.
build-release:
	$(COMPOSE) build-release

smoke:
	$(COMPOSE) smoke

test:
	$(COMPOSE) test

lint:
	$(COMPOSE) lint

demo:
	MELODIA_DEMO=1 $(COMPOSE) --service-ports app

# Landing page (web/) on 127.0.0.1:8897, in the background (also the update server).
web:
	docker compose up -d web
