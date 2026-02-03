# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

squads-cli is a Rust CLI for Microsoft Teams and Outlook, designed for AI agents and terminal users. It only works with organization accounts (school/work), not personal Microsoft accounts.

## Build Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo build --features tui     # Build with TUI support
cargo test                     # Run all tests
cargo test --lib               # Unit tests only
cargo test --test cli_tests    # Integration tests only
cargo test <test_name>         # Run specific test
cargo fmt                      # Format code
cargo clippy                   # Lint code
```

**Before committing**: Always run `cargo fmt && cargo clippy` to ensure code is formatted and passes lints.

## Architecture

### Module Structure

```
src/
├── main.rs          # Entry point: CLI parsing, config loading, command dispatch
├── api/             # Microsoft API integration
│   ├── auth.rs      # OAuth device code flow, token generation/renewal
│   ├── client.rs    # TeamsClient - manages multi-scope tokens, HTTP requests
│   └── emoji.rs     # Teams emoji name-to-character mapping
├── cli/             # Command implementations (one file per subcommand)
│   ├── mod.rs       # Cli struct, Commands enum, OutputFormat
│   └── output.rs    # JSON/table/plain formatting
├── types/           # Data models with serde (de)serialization
│   └── mod.rs       # Custom deserializers (strip_url, string_to_i64, etc.)
├── config.rs        # TOML config loading (~/.config/squads-cli/config.toml)
├── cache.rs         # JSON cache manager (~/.cache/squads-cli/)
└── tui/             # Optional TUI (feature-gated)
```

### Key Patterns

**Authentication**: Device code OAuth flow with multi-scope token management. Tokens are cached in `~/.cache/squads-cli/tokens.json` with separate tokens for IC3, CHATSVC, Graph, and Spaces APIs.

**Command Pattern**: Each subcommand module exports an `execute()` function that takes the parsed command, config, and optional format. Commands use `TeamsClient` for API calls.

**Output Formatting**: Three formats via `--format` flag:
- `json` - For AI agents (structured parsing)
- `table` - Default (human-readable with `tabled` crate)
- `plain` - Pipe-delimited for scripting

**Error Handling**: Uses `anyhow::Result` with `Context` for error messages. Pattern: `operation().context("what failed")?`

### Microsoft API Scopes

| Scope | Purpose |
|-------|---------|
| `ic3.teams.office.com` | Chat messages, reactions |
| `chatsvcagg.teams.microsoft.com` | Teams, channels, user details |
| `graph.microsoft.com` | User profiles, mail, calendar |
| `api.spaces.skype.com` | Real-time communication |

## Testing

Integration tests in `tests/cli_tests.rs` use `assert_cmd` and `predicates` to test CLI argument parsing and subcommand availability. Unit tests are in-module (e.g., `api::emoji::tests`).

## Configuration

Optional config file: `~/.config/squads-cli/config.toml` (defaults work without it)
```toml
[auth]
tenant = "organizations"  # Azure AD tenant (only change if using specific tenant)

[update]
auto_check = true         # Check for updates on startup
check_interval_hours = 24
```

## Feature Flags

- `tui` - Enables terminal UI with ratatui/crossterm (disabled by default)

## Releasing New Versions

1. Bump version in `Cargo.toml`
2. Build release binary: `cargo build --release`
3. Commit and push changes
4. Create GitHub release with binary:
   ```bash
   cp target/release/squads-cli target/release/squads-cli-linux-amd64
   gh release create vX.Y.Z \
     --repo aymericcousaert/squads-cli \
     --title "vX.Y.Z" \
     --notes "Release notes here" \
     target/release/squads-cli-linux-amd64
   ```

The update mechanism (`squads-cli update`) fetches releases from GitHub API and downloads the appropriate binary for the user's platform. Asset naming convention:
- `squads-cli-linux-amd64`
- `squads-cli-macos-amd64`
- `squads-cli-macos-arm64`
- `squads-cli-windows-amd64.exe`
