SHELL := /bin/bash

.PHONY: install check-toolchain check-static check-boundaries check-migrations \
	check-fixtures check-markhand-gates check-roadmap check-dependencies check-rust check-rust-tests \
	check-knowledge-features check-knowledge-extraction check-knowledge-extraction-rust \
	check-web check-desktop check-foundation bundle-linux dev-up dev-health dev-down dev-reset

install:
	pnpm install --frozen-lockfile

check-toolchain:
	bash scripts/check-web-toolchain.sh
	rustc --version | grep -q '^rustc 1\.88\.'
	cargo --version
	python3 --version

check-boundaries:
	python3 scripts/check-architecture-boundaries.py
	python3 scripts/check-architecture-boundaries.py --self-test

check-migrations:
	python3 scripts/check-migration-manifest.py --check
	python3 scripts/check-migration-manifest.py --self-test

check-fixtures:
	python3 scripts/check-fixtures.py
	python3 scripts/check-fixtures.py --root app/src-tauri/fixtures/knowledge/v1
	python3 scripts/check-fixtures.py --root crates/knowledge/fixtures
	python3 scripts/check-fixtures.py --self-test

check-markhand-gates:
	python3 scripts/check-markhand-gates.py
	python3 scripts/check-markhand-gates.py --self-test

check-roadmap:
	python3 scripts/build-roadmap.py --check

check-dependencies:
	python3 scripts/check-dependency-policy.py
	python3 scripts/check-dependency-policy.py --self-test

check-static: check-boundaries check-migrations check-fixtures check-markhand-gates check-roadmap check-dependencies

check-rust:
	bash scripts/check-rust-quality.sh

check-knowledge-features:
	bash scripts/check-knowledge-features.sh

check-knowledge-extraction:
	bash scripts/check-knowledge-extraction.sh

check-knowledge-extraction-rust:
	bash scripts/check-knowledge-extraction.sh --rust-only

check-rust-tests:
	cargo test -p fileconv-core
	cargo test -p fileconv-core --features llm llm
	cargo test -p fileconv-desktop
	cargo test -p fileconv-cli metrics
	cargo test -p fileconv-knowledge -p fileconv-server

check-web:
	pnpm --filter markhand-web format:check
	pnpm --filter markhand-web lint
	pnpm --filter markhand-web test
	pnpm --filter markhand-web api:check
	pnpm --filter markhand-web build

check-desktop:
	pnpm --filter markhand-desktop test
	pnpm --filter markhand-desktop build

check-foundation: check-toolchain check-static check-rust check-knowledge-extraction check-web

bundle-linux:
	pnpm --dir app tauri build --bundles deb --no-sign --ci
	bash scripts/validate-desktop-bundle.sh

dev-up:
	deploy/scripts/up.sh

dev-health:
	deploy/scripts/health.sh

dev-down:
	deploy/scripts/down.sh

dev-reset:
	deploy/scripts/reset.sh
