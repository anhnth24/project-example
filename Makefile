SHELL := /bin/bash

.PHONY: install check-toolchain check-static check-ci check-boundaries check-migrations \
	check-fixtures check-markhand-gates check-roadmap check-dependencies check-rust check-rust-tests \
	check-knowledge-features check-knowledge-extraction check-knowledge-extraction-rust \
	check-corpus check-corpus-pending check-web check-desktop check-foundation \
	check-spike spike-up spike-health spike-down spike-reset spike-lifecycle \
	check-desktop-baseline p0-desktop-baseline bundle-linux dev-up dev-health dev-down dev-reset \
	dev-server-smoke dev-init dev-seed-all dev-seed-password dev-print-defaults dev-download-embedding

install:
	pnpm install --frozen-lockfile

check-toolchain:
	bash scripts/check-web-toolchain.sh
	rustc --version | grep -q '^rustc 1\.88\.'
	cargo --version
	python3 --version

check-ci:
	python3 scripts/classify-ci-changes.py --self-test

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
	python3 scripts/check-phase0-decisions.py --self-test
	python3 scripts/check-runtime-license-inventory.py --self-test

check-corpus:
	python3 scripts/validate_corpus.py --reproducible
	python3 scripts/validate_corpus.py --self-test

check-corpus-pending:
	python3 scripts/validate_corpus.py --allow-pending --reproducible
	python3 scripts/validate_corpus.py --self-test

check-roadmap:
	python3 scripts/build-roadmap.py --check

check-dependencies:
	python3 scripts/check-dependency-policy.py
	python3 scripts/check-dependency-policy.py --self-test

check-static: check-ci check-boundaries check-migrations check-fixtures check-markhand-gates check-roadmap check-dependencies check-spike check-desktop-baseline

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

check-spike:
	python3 scripts/validate_spike.py
	python3 scripts/validate_spike.py --self-test

spike-up:
	deploy/spike/up.sh

spike-health:
	deploy/spike/health.sh

spike-down:
	deploy/spike/down.sh

spike-reset:
	deploy/spike/reset.sh

spike-lifecycle:
	deploy/spike/verify-lifecycle.sh

p0-desktop-baseline:
	bash bench/markhand_web/scripts/run_desktop_baseline.sh

check-desktop-baseline:
	python3 scripts/validate_desktop_baseline.py
	python3 scripts/validate_desktop_baseline.py --self-test

check-foundation: check-toolchain check-static check-rust check-knowledge-extraction check-web

bundle-linux:
	pnpm --dir app tauri build --bundles deb --no-sign --ci
	bash scripts/validate-desktop-bundle.sh

dev-up:
	deploy/scripts/up.sh

dev-health:
	deploy/scripts/health.sh

dev-server-smoke:
	deploy/scripts/server-smoke.sh

dev-init:
	deploy/scripts/init-dev-env.sh

dev-seed-all:
	deploy/scripts/seed-dev-all.sh

dev-seed-password:
	deploy/scripts/seed-dev-password.sh

dev-print-defaults:
	deploy/scripts/print-dev-defaults.sh

dev-download-embedding:
	deploy/scripts/download-aiteamvn-embedding.sh

dev-down:
	deploy/scripts/down.sh

dev-reset:
	deploy/scripts/reset.sh
