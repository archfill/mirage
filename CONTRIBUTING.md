# Contributing to Mirage

Thank you for your interest in contributing to Mirage!

## Development Setup

### Prerequisites

- Rust (stable, latest)
- System libraries:
  - `libfuse3-dev` (Ubuntu/Debian) or `fuse3` (Arch)
  - `libdbus-1-dev` (Ubuntu/Debian) or `dbus` (Arch)
  - `cmake`
  - `extra-cmake-modules`
  - `qt6-base-dev` (Ubuntu/Debian) or `qt6-base` (Arch)
  - `kio-dev` (Ubuntu/Debian) or `kio` (Arch) — required for Dolphin plugins

### Build & Test

```bash
cargo build          # Build
cargo test           # Run tests
cargo clippy         # Lint (must have zero warnings)
cargo fmt -- --check # Check formatting
```

A `Makefile` is also provided for common tasks:

```bash
make build    # Build release binary
make test     # Run tests + clippy + fmt check
make install  # Install to system (copies binary, completions, desktop files, etc.)
```

For Arch Linux packaging:

```bash
cd dist/
makepkg -si   # Build and install the package
```

## How to Contribute

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make your changes
4. Ensure `cargo test`, `cargo clippy`, and `cargo fmt -- --check` all pass
5. Commit using [Conventional Commits](https://www.conventionalcommits.org/) format
6. Open a Pull Request

### Commit Message Format

```
type(scope): short description

feat:     new feature
fix:      bug fix
refactor: code change that neither fixes a bug nor adds a feature
docs:     documentation only
test:     adding or updating tests
chore:    maintenance tasks
ci:       CI/CD changes
```

## Code Guidelines

- No `unwrap()` / `expect()` in production code — use `Result` / `?`
- No `unsafe` unless absolutely necessary (document the reason)
- Keep `cargo clippy` warnings at zero
- Use `tracing` crate for logging (not `println!`)
- FUSE callbacks must not perform network I/O directly

## Reporting Issues

- Use [GitHub Issues](https://github.com/archfill/mirage/issues)
- For security vulnerabilities, see [SECURITY.md](SECURITY.md)

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
