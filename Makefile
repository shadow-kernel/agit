# Common developer commands. `make help` lists them.

.PHONY: help build release check test fmt clippy serve web web-dev sdk-python sdk-ts bench clean

help:
	@grep -E '^[a-z-]+:' Makefile | cut -d: -f1 | tr '\n' ' '; echo

build:
	cargo build

release:
	cargo build --release

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --all

serve: release
	./target/release/agit-server --data-dir ./agit-data --listen 127.0.0.1:7878 --web-dir web/dist

web:
	cd web && pnpm install && pnpm build

web-dev:
	cd web && pnpm install && pnpm dev

sdk-python:
	cd sdk/python && python3 -m pip install -e ".[dev]" && python3 -m pytest -q

sdk-ts:
	cd sdk/typescript && pnpm install && pnpm build && pnpm test

bench: release
	cargo run --release -p agit-core --example bench_commits -- 3000

clean:
	cargo clean && rm -rf web/dist web/node_modules sdk/typescript/dist sdk/typescript/node_modules
