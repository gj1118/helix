build:
    cargo build --release

install:
    cargo install --path helix-term --locked

build_and_install: build install
    echo "Build and install complete!"
