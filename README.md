# Squads CLI

A command-line interface for Microsoft Teams, designed for AI agents (Claude Code, Codex, OpenCode) and terminal users.

> **Note**: This client only works with organization accounts (school/work). Personal Microsoft accounts are not supported due to API differences.

## Features

- **Global search**: Search across your Mail and Calendar with a single command
- **Full chat support**: List, read, and send messages (with Markdown support)
- **Outlook Mail integration**: Full support for listing, reading, sending, drafting, and managing emails
- **Calendar management**: View schedules, check availability (Free/Busy), and manage events (including shared calendars)
- **Interactive TUI**: A terminal user interface for a more visual experience
- **CLI-first design**: JSON output format optimized for AI agents
- **Teams support**: Browse teams and channels
- **User management**: Search and view user profiles
- **Activity feed**: View notifications and mentions

## Installation

### From source

```bash
git clone https://github.com/aymericcousaert/squads-cli
cd squads-cli
cargo build --release
./target/release/squads-cli install
```

The binary will be installed to `~/.local/bin/squads-cli`. Ensure this directory is in your `PATH`.

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

### Global Search

Search across Mail and Calendar simultaneously.

```bash
# Search for a keyword
squads-cli search "keyword"

# Limit results and choose format
squads-cli search "project" --limit 10 --format json
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

# Send a message with Markdown support
squads-cli chats send <chat-id> "**Bold** and _italic_" --markdown

# Reply to a message (with citation fallback for 1:1 chats)
squads-cli chats reply <chat-id> --message-id <msg-id> "My reply"

### Outlook Mail

```bash
# List emails
squads-cli mail list --limit 10

# Read an email
squads-cli mail read <msg-id>

# Search emails specifically
squads-cli mail search "invoice"

# Send an email
squads-cli mail send --to "user@example.com" --subject "Hello" "Email body"

# Create a draft
squads-cli mail draft --to "user@example.com" --subject "Draft" "Content"

# Manage emails
squads-cli mail reply <msg-id> "My reply"
squads-cli mail forward <msg-id> --to "other@example.com"
squads-cli mail mark <msg-id> --read
squads-cli mail delete <msg-id>

# Attachments
squads-cli mail attachments <msg-id>
squads-cli mail download <msg-id> <attachment-id> --output "file.pdf"
```

### Calendar

```bash
# View today's events
squads-cli calendar today

# View events for the next 7 days
squads-cli calendar week

# List events in a specific range
squads-cli calendar list --start 2024-01-01 --end 2024-01-31

# List all accessible calendars (including shared and groups)
squads-cli calendar calendars

# Check availability (Free/Busy) for a contact
squads-cli calendar free-busy --users "aymeric@example.com"

# View shared calendar
squads-cli calendar today --user-id <user-id-or-email>

# Manage events
squads-cli calendar show <event-id>
squads-cli calendar rsvp <event-id> accept --comment "I'll be there"
squads-cli calendar delete <event-id>
```

### Interactive TUI

```bash
# Launch the terminal UI
squads-cli tui
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

### Shell Completions

Generate completion scripts for your shell.

```bash
# For Zsh
squads-cli completions zsh > ~/.zfunc/_squads-cli
echo "fpath+=~/.zfunc" >> ~/.zshrc

# For Bash
squads-cli completions bash > squads-cli.bash
source squads-cli.bash

# For Fish
squads-cli completions fish > ~/.config/fish/completions/squads-cli.fish
```

## Output Formats

Use `--format` to control output:

- `--format json` - JSON output (best for AI agents)
- `--format table` - Table output (default, best for humans)
- `--format plain` - Pipe-delimited output (for scripting)

## AI Agent Integration

To make `squads-cli` capabilities available globally to your AI agent (like Claude Code or OpenCode), you can symlink the `SKILL.md` file:

```bash
mkdir -p ~/.claude/skills/squads-cli
ln -s $(pwd)/SKILL.md ~/.claude/skills/squads-cli/SKILL.md
```

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
