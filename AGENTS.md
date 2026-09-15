# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 Cargo workspace. Shared clipboard models, configuration, IPC, themes, and media helpers live in `crates/little-clip-core`. Keep hardware and USB-display code in `little-clip-driver`, rendering in `little-clip-render`, and transformations and pipeline operations in `little-clip-ops`. The `little-clipd`, `clipctl`, and `clip-workbench` crates provide the daemon, CLI, and egui workbench binaries.

Root `themes/` contains SVG themes, `scripts/` contains supporting Python tools, `config.toml` holds runtime defaults, and `launch.sh` manages local builds and services.

## Build, Test, and Development Commands

Run commands from the repository root:

```bash
cargo build                              # Build the workspace in debug mode
cargo build --release                    # Build optimized binaries
cargo test --workspace                   # Run all unit tests
cargo test -p little-clip-ops            # Test one crate
cargo fmt --check                        # Verify Rust formatting
cargo clippy --workspace --all-targets -- -D warnings
./launch.sh build                        # Release build through the helper
./launch.sh start                        # Start the local daemon
./launch.sh ui                           # Open the workbench
./launch.sh test                         # Exercise the USB display directly
```

`./launch.sh test` can stop a running daemon and access attached hardware; use it only when that is intended.

## Coding Style & Naming Conventions

Use standard `rustfmt` output (four-space indentation). Name modules, functions, and variables in `snake_case`; use `PascalCase` for types and traits and `SCREAMING_SNAKE_CASE` for constants. Keep modules focused on one domain. For fallible operations, return explicit `Result` values and use the project’s error types rather than panicking in normal runtime paths.

## Testing Guidelines

Keep unit tests beside the code in `#[cfg(test)]` modules. Use descriptive `snake_case` test names that state the expected behavior, such as `parses_valid_theme_colors`. Add targeted tests for new transformations, parsing or configuration behavior, rendering, and testable UI logic. Run the affected crate’s tests during development and `cargo test --workspace` before submitting broader changes.

## Commit & Pull Request Guidelines

Follow the existing Conventional Commit pattern with a concise scope: `feat(image): add film simulation controls`, `refactor(ui): extract shared toolbar`, or `feat(theme): improve contrast`. Use imperative summaries. Pull requests should explain the behavior change, list validation commands and results, link relevant issues when available, and include screenshots or GIFs for workbench or theme changes.

## Runtime Configuration & Hardware

Do not commit personal display, compositor, or desktop settings. Keep `config.toml` changes reloadable, and preserve virtual-mode behavior when no supported USB display is attached.
