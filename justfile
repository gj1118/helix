build: fmt lint test
    cargo build --release

install:
    cargo install --path helix-term --locked

build_and_install: build install
    echo "Build and install complete!"

test: unit-test integration-test

unit-test:
    cargo test --workspace --lib

integration-test:
    cargo test --workspace --tests --features integration

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
    cargo fmt --all
