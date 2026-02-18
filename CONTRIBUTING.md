# Contributing to LiveClaw

Thank you for your interest in contributing to LiveClaw! This document provides guidelines for contributing to the project.

## Table of Contents

- [Contribution Workflow](#contribution-workflow)
- [Getting Started](#getting-started)
- [Project Structure](#project-structure)
- [Quality Gates](#quality-gates)
- [Build Commands](#build-commands)
- [Testing](#testing)
- [Code Style](#code-style)
- [Pull Request Checklist](#pull-request-checklist)
- [Architecture Notes](#architecture-notes)

## Contribution Workflow

We follow an issue-first workflow. Every code change should trace back to an issue for visibility and coordination.

### 1. Open or Claim an Issue

Before writing code, make sure there's a GitHub issue for the work:

- **Bug?** Open a bug report with reproduction steps.
- **Feature?** Open a feature request describing the motivation and proposed approach.
- **Already exists?** Comment on the issue to signal you're working on it.

### 2. Create a Feature Branch

```bash
git checkout main
git pull origin main
git checkout -b feat/my-feature    # or fix/my-bug, docs/my-update
```

Branch naming conventions:
- `feat/<description>` — new features
- `fix/<description>` — bug fixes
- `docs/<description>` — documentation changes
- `refactor/<description>` — code improvements without behavior change
- `test/<description>` — test additions or improvements

### 3. Develop with Quality Gates

Run these before every commit. CI enforces them — save yourself the round-trip:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 4. Submit a PR

- Reference the issue: `Fixes #123` in the PR description
- Fill out the PR template checklist
- Keep PRs focused — one logical change per PR
- Don't mix unrelated changes

### 5. Review and Merge

- PRs require review and passing CI before merge
- Address review feedback with additional commits (don't force-push during review)
- Squash or merge commit at maintainer discretion

## Getting Started

```bash
# Clone LiveClaw
git clone https://github.com/zavora-ai/liveclaw.git
cd liveclaw

# Clone ADK-Rust as a sibling directory
cd ..
git clone <adk-rust-repo-url> adk-rust
cd liveclaw

# Run the dev setup script
chmod +x dev/setup.sh
./dev/setup.sh

# Build and validate
cargo build --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Prerequisites

- Rust 1.75+ (2021 edition)
- [ADK-Rust](https://github.com/adk-rust) cloned as a sibling directory (`../adk-rust/`)

### Environment Variables

Copy the dev config and fill in your API key:

```bash
cp dev/liveclaw.dev.toml liveclaw.toml
# Edit liveclaw.toml with your voice provider API key
```

**Important:** Never commit `.env` files, API keys, local paths, or IDE configuration.

## Project Structure

LiveClaw is a Cargo workspace with two crates:

```
liveclaw-gateway/
  src/
    lib.rs             Module declarations
    protocol.rs        GatewayMessage/GatewayResponse types + proptest round-trips
    pairing.rs         PairingGuard authentication + proptest
    server.rs          Gateway WebSocket server, session routing, health check

liveclaw-app/
  src/
    lib.rs             Module declarations
    main.rs            Application entry point — wires all components together
    config.rs          LiveClawConfig and all sub-configs + TOML parsing
    agent.rs           RealtimeAgent construction with callbacks
    security.rs        Shell injection detection, rate limiter, access control
    memory.rs          MemoryAdapter bridging adk-memory to adk-core
    plugins.rs         PII redaction, memory auto-save, guardrail plugins
    graph.rs           Optional GraphAgent wrapping
    runner.rs          Runner construction

docs/
  design.md            Architecture, data flows, component interfaces
  requirements.md      User stories, acceptance criteria, correctness properties

dev/
  liveclaw.dev.toml    Development configuration template
  setup.sh             Dev environment setup script
```

## Quality Gates

Every PR must pass these checks. CI enforces them automatically.

### The Three Gates

| Gate | Command | What It Catches |
|------|---------|-----------------|
| Format | `cargo fmt --all -- --check` | Inconsistent formatting |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | Warnings, dead code, anti-patterns |
| Test | `cargo test --workspace` | Regressions, broken logic |

### Quick Validation

Run all three in sequence:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If all three pass, your PR will pass CI.

### What "Zero Warnings" Means

Clippy runs with `-D warnings` — every warning is a compile error. This includes:

- Unused imports, variables, and dead code
- `println!`/`eprintln!` in library code (use `tracing` instead)
- Unnecessary clones, redundant closures, etc.

Fix warnings before pushing. Don't suppress them with `#[allow(...)]` unless there's a documented reason.

## Build Commands

| Command | Description |
|---------|-------------|
| `cargo build --workspace` | Build all crates |
| `cargo test --workspace` | Run all tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Run clippy lints |
| `cargo fmt --all` | Format all code |
| `cargo run -- liveclaw.toml` | Run the application |

## Testing

```bash
# Full workspace
cargo test --workspace

# Single crate
cargo test -p liveclaw-gateway
cargo test -p liveclaw-app
```

### Test Organization

- Unit tests: `#[cfg(test)]` modules in source files
- Property tests: using `proptest` (100+ iterations) for protocol round-trips, pairing, security, and config

### Writing Tests for New Code

| Change Type | Expected Tests |
|-------------|----------------|
| New public function | Unit test with happy path + error cases |
| New trait implementation | Integration test exercising the trait contract |
| Bug fix | Regression test that fails without the fix |
| Serialization/config | Property test with `proptest` (100+ iterations) |

If your change is purely internal refactoring with no behavior change, existing tests passing is sufficient.

## Code Style

### Rust Conventions

- Edition 2021
- `thiserror` for library error types
- `async-trait` for async trait methods
- `Arc<T>` for shared ownership across async boundaries
- `tokio::sync::RwLock` for async-safe interior mutability
- Builder pattern for complex configuration
- `tracing` for structured logging (never `println!` or `eprintln!` in library code)

### Error Handling

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MyError {
    #[error("Descriptive message: {0}")]
    Variant(String),

    #[error("Actionable guidance: {details}")]
    WithGuidance { details: String },
}
```

### Documentation

Every public item needs rustdoc. Include `# Example` sections where practical:

```rust
/// Brief one-line description.
///
/// More detail if needed.
///
/// # Example
///
/// ```rust,ignore
/// let result = my_function("input")?;
/// ```
pub fn my_function(input: &str) -> Result<Output> { ... }
```

### Commit Messages

Use conventional commits:

```
feat(gateway): add session timeout configuration
fix(security): correct rate limiter reset logic
docs: update CONTRIBUTING.md
refactor(plugins): extract PII patterns into constants
test(pairing): add lockout property tests
```

## Pull Request Checklist

### Quality Gates (all required)

- [ ] `cargo fmt --all` — code is formatted
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings
- [ ] `cargo test --workspace` — all tests pass
- [ ] Builds clean: `cargo build --workspace`

### Code Quality

- [ ] New code has tests (unit, integration, or property tests as appropriate)
- [ ] Public APIs have rustdoc comments
- [ ] No `println!`/`eprintln!` in library code (use `tracing` instead)
- [ ] No hardcoded secrets, API keys, or local paths

### Hygiene

- [ ] No local development artifacts (`.env`, `.DS_Store`, IDE configs, build dirs)
- [ ] No unrelated changes mixed in
- [ ] Commit messages follow conventional format (`feat:`, `fix:`, `docs:`, etc.)
- [ ] PR targets `main` branch
- [ ] PR references an issue (`Fixes #___`)

### Documentation (if applicable)

- [ ] CHANGELOG.md updated for user-facing changes
- [ ] README updated if capabilities changed
- [ ] Examples added or updated for new features

## Architecture Notes

### Adding a New Plugin

1. Implement the plugin function in `liveclaw-app/src/plugins.rs`
2. Register it in `build_plugin_manager()`
3. Add configuration toggle in `LiveClawConfig` if needed
4. Write unit tests and property tests if applicable

### Adding a New Gateway Message

1. Add the variant to `GatewayMessage` and/or `GatewayResponse` in `liveclaw-gateway/src/protocol.rs`
2. Add the handler in `liveclaw-gateway/src/server.rs`
3. Add proptest `Arbitrary` implementation for the new variant
4. Update the protocol documentation in the README

### Modifying Configuration

1. Add the field to the appropriate sub-config struct in `liveclaw-app/src/config.rs`
2. Add a default value function if needed
3. Update the dev config template in `dev/liveclaw.dev.toml`
4. Add a test case in the config tests

## Getting Help

- [GitHub Issues](https://github.com/zavora-ai/liveclaw/issues) — bug reports and feature requests
- [GitHub Discussions](https://github.com/zavora-ai/liveclaw/discussions) — questions and ideas

## License

By contributing, you agree that your contributions will be licensed under the [Apache 2.0 License](LICENSE).
