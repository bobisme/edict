# Build the Rust binary
build:
    cargo build

# Run tests
test:
    cargo test

# Install the binary to ~/.cargo/bin
install:
    cargo install --path . --locked

# Lint with clippy
lint:
    cargo clippy --all-targets -- -D warnings

# Format with rustfmt
fmt:
    cargo fmt

# Check types without building
check:
    cargo check
