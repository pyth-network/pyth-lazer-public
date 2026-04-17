# Pyth Pro contracts, SDKs, and tools

## Repository Setup

This repository contains a mix of Rust and TypeScript code managed in a single workspace.

The project uses:

- **Rust** via `rustup` and `cargo`
- **mise** for JavaScript/TypeScript runtime/tool version management
- **bun** for JavaScript/TypeScript package management
- **turbo** for workspace task orchestration

### Prerequisites

Install the required tools from their official websites:

-   rustup (Rust toolchain manager): https://rustup.rs/
-   mise (JS/TS runtime manager): https://mise.jdx.dev/

Follow the installation instructions provided on those websites, then restart your shell.

After installation, verify the tools are available:

``` bash
rustup --version
cargo --version
rustc --version
mise --version
```

### Project bootstrap

From the repository root, install all configured tools:

``` bash
mise install
```

This installs the runtime versions declared in the repository (such as Bun).

Install JavaScript / TypeScript workspace dependencies:

``` bash
bun install
```

## Common development commands

### Build everything

``` bash
cargo build
bun turbo run build
```

### Run tests

``` bash
cargo test
bun turbo run test
```

### Run linters and checks

``` bash
cargo clippy --all-targets
cargo fmt --check
bun turbo run lint
```

### License

Licensed under <a href="LICENSE">Apache License, Version 2.0</a>.
