# Squads CLI

A command-line interface for Microsoft Teams, designed for AI agents (Claude Code, Codex, OpenCode) and terminal users.

> **Note**: This client only works with organization accounts (school/work). Personal Microsoft accounts are not supported due to API differences.

## Features

- **CLI-first design**: JSON output format optimized for AI agents
- **Full chat support**: List, read, and send messages
- **Teams support**: Browse teams and channels
- **User management**: Search and view user profiles
- **Activity feed**: View notifications and mentions

## Installation

### From source

```bash
git clone https://github.com/aymericcousaert/squads-cli
cd squads-cli
cargo build --release
```

The binary will be at `target/release/squads-cli`.

## Usage

### Authentication

```bash
# Login (opens device code flow)
squads-cli auth login

# Check auth status
squads-cli auth status

# Logout
squads-cli auth logout
```

### Chats

```bash
# List all chats
squads-cli chats list

# Get chat messages
squads-cli chats messages <chat-id>

# Send a message
squads-cli chats send <chat-id> "Hello, World!"

# Send from stdin (useful for AI agents)
echo "Hello" | squads-cli chats send <chat-id> --stdin

# Send from file
squads-cli chats send <chat-id> --file message.txt
```

### Teams

```bash
# List all teams
squads-cli teams list

# List channels in a team
squads-cli teams channels <team-id>

# Get channel messages
squads-cli teams messages <team-id> <channel-id>
```

### Users

```bash
# List users
squads-cli users list

# Search users
squads-cli users list --search "John"

# Show current user
squads-cli users me
```

### Activity

```bash
# View activity feed
squads-cli activity list
```

## Output Formats

Use `--format` to control output:

- `--format json` - JSON output (best for AI agents)
- `--format table` - Table output (default, best for humans)
- `--format plain` - Pipe-delimited output (for scripting)

## AI Agent Integration

Example workflow for an AI agent:

```bash
# 1. Check authentication
squads-cli auth status --format json

# 2. List chats
CHATS=$(squads-cli chats list --format json)

# 3. Get messages from a chat
MESSAGES=$(squads-cli chats messages "19:abc@thread.v2" --format json)

# 4. Send a response
squads-cli chats send "19:abc@thread.v2" "Your response here"
```

## Configuration

Config file: `~/.config/squads-cli/config.toml`

```toml
[auth]
tenant = "organizations"  # or specific tenant ID

[output]
default_format = "table"
color = true

[api]
region = "emea"
timeout = 30
```

## Credits

Based on the [Squads](https://github.com/IanTerzo/Squads) project by IanTerzo.

## License

GPL-3.0
