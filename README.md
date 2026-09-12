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
- **Real-time watch**: Stream messages, edits, typing and read receipts as JSON lines
- **Personal Notes**: Shortcut command to manage your personal notes chat

## Installation

### From source

```bash
git clone https://github.com/aymericcousaert/squads-cli
cd squads-cli
cargo build --release --features tui
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
# List all chats (default limit: 50)
squads-cli chats list --limit 20

# Search chats by member names or title (all words must match, any order)
squads-cli chats list --search "john"
squads-cli chats list --search "john alice"  # finds "John Doe & Alice Smith"

# Add each chat's other members, with their IDs, to the JSON output
squads-cli chats list --format json --with-members

# Get chat messages
squads-cli chats messages <chat-id>

# Add system messages to the JSON output (members added, topic renames, calls)
squads-cli chats messages <chat-id> --format json --types text,thread_activity
squads-cli chats messages <chat-id> --format json --types all

# Mark a chat as read (clears its unread state in Teams, everywhere)
squads-cli chats read <chat-id>
# Read up to a message you already have, which saves the lookup
squads-cli chats read <chat-id> --message-id <msg-id>

# Send a message
squads-cli chats send <chat-id> "Hello, World!"

# Send from stdin (useful for AI agents)
echo "Hello" | squads-cli chats send <chat-id> --stdin

# Send a message with Markdown support
squads-cli chats send <chat-id> "**Bold** and _italic_" --markdown

# Reply to a message (with citation fallback for 1:1 chats)
squads-cli chats reply <chat-id> --message-id <msg-id> "My reply"

# React to a message (full support for Teams emojis by name or character)
squads-cli chats react <chat-id> --message-id <msg-id> unicornhead
squads-cli chats react <chat-id> --message-id <msg-id> 🦄

# Download a file (supports piping to stdout)
squads-cli chats download-file <chat-id> <file-url> --output "file.docx"
# We recommend using piping for AI agents to process files without saving to disk
squads-cli chats download-file <chat-id> <file-url> -o - | textutil -convert txt -stdin -stdout
```

`chats read` moves the chat's read watermark to now, which is what Teams
derives `unread` from. Opening a chat in a client of your own changes nothing
until you call it. Without `--message-id` it looks the newest message up first,
so it costs one extra fetch.

`chats messages` returns human messages only, so existing scripts see no change.
`--types` adds the other kinds to the `--format json` output:

| `--types` value | What it adds | Wire `messagetype` |
|---|---|---|
| `text` | messages someone typed (the default) | `RichText/Html`, `Text` |
| `thread_activity` | members added or removed, topic renames | `ThreadActivity/*` |
| `event` | call records | `Event/*` |
| `all` | every message the chat returned | any |

The table and plain output always show human messages only: a terminal listing does
not want call records.

A message that fails to decode is skipped, not fatal. The count and the message id
go to stderr, so the rest of the conversation still loads.

### Watch (real-time)

```bash
# Follow new messages in the terminal
squads-cli watch --push

# One JSON line per new message, for scripts and agents
squads-cli watch --json

# Put other real-time events on the same stream
squads-cli watch --json --events message,typing,read
squads-cli watch --json --events all

# Only one chat
squads-cli watch --json --chat "19:abc@thread.v2"

# Also stream what you sent yourself, from this or any other device
squads-cli watch --json --include-self
```

`--json` prints one JSON object per line. Every line carries `event`, `time` and
`source`. Only `message` is sent by default, so existing consumers see no change.

| `event` | Meaning | Fields on top of `event`, `time`, `source` |
|---|---|---|
| `message` | a new chat message | `chat_id`, `message_id`, `from`, `from_mri`, `content` |
| `message_update` | an edit, or a reaction landing on a message | same as `message` |
| `typing` | someone is typing | `chat_id`, `from` (usually empty) |
| `read` | someone moved their read marker | `chat_id` |
| `message_loss` | events were dropped, so resync | none |
| `presence` | a user's availability changed | `user_id`, `availability` |

How the filters apply:

- `--chat` filters the chat events: `message`, `message_update`, `typing` and `read`. `message_loss` and `presence` are account-wide and always pass
- your own messages and your own edits are dropped, unless you pass `--include-self`
- `message` is de-duplicated by `message_id`. `message_update` is not, because an edit reuses the id
- the terminal output (`--push` without `--json`) shows messages only, whatever `--events` says
- `presence` is parsed and emitted, but nothing subscribes to presence yet, so the stream is quiet until it does

`--include-self` is for a chat client: a message you send from your phone belongs in the
thread, and moves that chat to the top of the list. A notifier wants the default, where
your own traffic is noise. The flag covers `message` and `message_update` on both the
push and the polling path, so your own edits arrive too.

### Personal Notes

Shortcut to your personal "Notes" chat.

```bash
# Add a new note
squads-cli notes add "Remember to check the crawler-batch"

# List recent notes
squads-cli notes list

# Delete a note
squads-cli notes delete <message-id>
```

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

# Send with markdown formatting (bold, lists, etc.)
squads-cli mail send --to "user@example.com" --subject "Hello" --markdown "**Bold** and *italic*"

# Create a draft
squads-cli mail draft --to "user@example.com" --subject "Draft" "Content"

# Create a draft with markdown formatting
squads-cli mail draft --to "user@example.com" --subject "Draft" --markdown "**Bold title**"

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

# Download someone's profile photo
squads-cli users photo <user-id> --output avatar.jpg
squads-cli users photo alice@example.com --output avatar.jpg

# A team's photo, by the group ID in `teams show`
squads-cli users photo <group-id> --group --output team.jpg

# Straight to stdout
squads-cli users photo <user-id> -o - | open -f -a Preview
```

Most people never set a photo. `users photo` says so on stderr and exits **3**,
which is not the **1** a real failure exits with, so a caller can remember
"nobody set one" instead of retrying. With `--format json` it prints
`{"id": "...", "found": false}` and still exits 3.

`--with-members` on `chats list` adds a `people` array to each chat: the other
members, in the order Teams lists them, each with the object ID `users photo`
takes and a display name for the fallback. It costs no extra request, and the
table output is unchanged.

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

Optional config file: `~/.config/squads-cli/config.toml` (defaults work without it)

```toml
[auth]
tenant = "organizations"  # or specific tenant ID

[update]
auto_check = true         # check for updates on startup
check_interval_hours = 24
```

## Credits

Based on the [Squads](https://github.com/IanTerzo/Squads) project by IanTerzo.

## License

GPL-3.0
