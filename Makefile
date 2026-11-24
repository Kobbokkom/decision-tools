.PHONY: dev check lint test build format verify

dev:
	cargo run -- --config examples/workflows.toml --db decision-tools.sqlite3 --demo stdio

check:
	cargo check --locked

lint:
	cargo clippy --locked --all-targets -- -D warnings
	cargo fmt --check

test:
	cargo test --locked
	cargo build --locked
	python3 tests/smoke.py target/debug/decision-tools

build:
	cargo build --locked --release

format:
	cargo fmt

verify: check lint test build
