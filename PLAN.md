# Squads CLI - Microsoft Teams CLI for AI Agents

## 1. Project Analysis: How Squads Works

### 1.1 Architecture Overview

Squads is an alternative Microsoft Teams client written in Rust using the **iced** GUI framework. It reverse-engineers the Microsoft Teams web APIs to provide a lightweight, minimalist client.

### 1.2 Authentication Flow

```
┌─────────────────┐    ┌────────────────────────┐    ┌─────────────────┐
│  Device Code    │ -> │ User visits URL and    │ -> │  Refresh Token  │
│  Generation     │    │ enters code            │    │  Obtained       │
└─────────────────┘    └────────────────────────┘    └─────────────────┘
                                                              │
                                                              v
┌─────────────────┐    ┌────────────────────────┐    ┌─────────────────┐
│  Access Tokens  │ <- │ Token Generation for   │ <- │  Token Refresh  │
│  (Multiple)     │    │ different scopes       │    │  as needed      │
└─────────────────┘    └────────────────────────┘    └─────────────────┘
```

**Key Authentication Components:**
- **Device Code Flow**: OAuth 2.0 device authorization for headless login
- **Multiple Tokens**: Different scopes for different APIs
  - `https://ic3.teams.office.com/.default` - Chat service
  - `https://chatsvcagg.teams.microsoft.com/.default` - Teams/Chat aggregation
  - `https://graph.microsoft.com/.default` - Microsoft Graph
  - `https://api.spaces.skype.com/Authorization.ReadWrite` - Real-time messaging
- **Skype Token**: Required for real-time features (WebSockets)
- **Token Caching**: Tokens cached in `~/.cache/ianterzo/squads/`

### 1.3 API Endpoints Used

| API | Base URL | Purpose |
|-----|----------|---------|
| Microsoft Graph | `graph.microsoft.com/v1.0` | User profiles, organization data |
| Teams Chat Service | `teams.microsoft.com/api/chatsvc/emea/v1` | Messages, conversations |
| Teams User API | `teams.microsoft.com/api/csa/emea/api/v2` | Teams, channels, chats |
| Teams MT | `teams.microsoft.com/api/mt/part/emea-02/beta` | Profiles, pictures |
| Trouter | `go.trouter.teams.microsoft.com/v4` | Real-time WebSocket |
| SharePoint | `{tenant}.sharepoint.com` | File operations |

### 1.4 Data Models

**Core Entities:**
- `Team` - Organization teams with channels
- `Channel` - Channels within teams
- `Chat` - Direct/group conversations
- `Message` - Individual messages with content, reactions, files
- `Profile` - User information
- `Presence` - Online/offline status

### 1.5 Real-time Communication

The WebSocket connection (`websockets.rs`) uses Microsoft Trouter:
1. Initialize connection via `teams_trouter_start()`
2. Register for notifications
3. Maintain WebSocket with 40-second pings
4. Receive real-time messages, presence updates, typing indicators

---

## 2. CLI Implementation Proposal

### 2.1 Project Structure

```
squads-cli/
├── Cargo.toml
├── src/
│   ├── main.rs                 # Entry point, argument parsing
│   ├── lib.rs                  # Library exports
│   ├── api/
│   │   ├── mod.rs
│   │   ├── auth.rs             # Authentication (reused from Squads)
│   │   ├── client.rs           # HTTP client wrapper
│   │   ├── teams.rs            # Teams/Channels API
│   │   ├── chat.rs             # Chat/Messages API
│   │   ├── users.rs            # Users/Profiles API
│   │   └── websocket.rs        # Real-time connection
│   ├── types/
│   │   ├── mod.rs
│   │   ├── team.rs
│   │   ├── chat.rs
│   │   ├── message.rs
│   │   └── user.rs
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── commands.rs         # CLI command definitions
│   │   └── output.rs           # Output formatting (JSON, table, plain)
│   ├── tui/
│   │   ├── mod.rs
│   │   ├── app.rs              # TUI application state
│   │   ├── ui.rs               # UI rendering
│   │   ├── events.rs           # Event handling
│   │   └── components/
│   │       ├── chat_list.rs
│   │       ├── message_view.rs
│   │       └── input.rs
│   ├── config.rs               # Configuration management
│   └── cache.rs                # Token and data caching
├── README.md
└── PLAN.md
```

### 2.2 Command-Line Interface Design

#### 2.2.1 Authentication Commands

```bash
# Login using device code flow (interactive)
squads-cli auth login

# Login with specific tenant
squads-cli auth login --tenant "contoso.onmicrosoft.com"

# Check authentication status
squads-cli auth status

# Logout and clear tokens
squads-cli auth logout

# Refresh tokens manually
squads-cli auth refresh
```

#### 2.2.2 Teams Commands

```bash
# List all teams
squads-cli teams list [--format json|table|plain]

# Show team details
squads-cli teams show <team-id>

# List channels in a team
squads-cli teams channels <team-id>

# Get channel messages
squads-cli teams messages <team-id> <channel-id> [--limit 50]
```

#### 2.2.3 Chat Commands

```bash
# List all chats
squads-cli chats list [--format json|table|plain]

# Show chat details
squads-cli chats show <chat-id>

# Get chat messages
squads-cli chats messages <chat-id> [--limit 50] [--since "2024-01-01"]

# Send message to chat
squads-cli chats send <chat-id> "Hello, World!"

# Send message from stdin (useful for AI agents)
echo "Hello" | squads-cli chats send <chat-id> --stdin

# Send message from file
squads-cli chats send <chat-id> --file message.txt

# Start new chat with user(s)
squads-cli chats new <user-id> [<user-id>...] --message "Hello!"

# Watch chat for new messages (real-time)
squads-cli chats watch <chat-id>
```

#### 2.2.4 User Commands

```bash
# List users in organization
squads-cli users list [--search "John"]

# Show user profile
squads-cli users show <user-id>

# Show current user
squads-cli users me

# Get user presence/status
squads-cli users presence <user-id>
```

#### 2.2.5 Activity Commands

```bash
# List activity feed
squads-cli activity list [--limit 20]

# Watch activity (real-time notifications)
squads-cli activity watch
```

#### 2.2.6 Interactive TUI Mode

```bash
# Launch interactive TUI
squads-cli tui

# TUI with specific chat pre-selected
squads-cli tui --chat <chat-id>
```

### 2.3 Output Formats

**JSON Output (for AI agents):**
```bash
squads-cli chats list --format json
```
```json
{
  "chats": [
    {
      "id": "19:xxx@thread.v2",
      "title": "Project Discussion",
      "members": ["user1@example.com", "user2@example.com"],
      "lastMessage": {
        "content": "Hello!",
        "from": "user1@example.com",
        "timestamp": "2024-01-15T10:30:00Z"
      },
      "isRead": true
    }
  ]
}
```

**Table Output (for humans):**
```
┌────────────────────┬─────────────────────┬───────────────┬────────┐
│ Chat ID            │ Title               │ Last Message  │ Unread │
├────────────────────┼─────────────────────┼───────────────┼────────┤
│ 19:abc@thread.v2   │ Project Discussion  │ Hello!        │ No     │
│ 19:def@thread.v2   │ Team Standup        │ Good morning  │ Yes    │
└────────────────────┴─────────────────────┴───────────────┴────────┘
```

**Plain Output (minimal, scriptable):**
```
19:abc@thread.v2|Project Discussion|Hello!|false
19:def@thread.v2|Team Standup|Good morning|true
```

### 2.4 Configuration

**Config file:** `~/.config/squads-cli/config.toml`

```toml
[auth]
tenant = "organizations"  # or specific tenant

[output]
default_format = "json"  # json, table, plain
color = true

[cache]
directory = "~/.cache/squads-cli"
token_file = "tokens.json"

[tui]
theme = "dark"
refresh_interval = 5  # seconds

[api]
region = "emea"  # emea, amer, apac
timeout = 30  # seconds
```

### 2.5 TUI Design (using Ratatui)

```
┌─────────────────────────────────────────────────────────────────────┐
│ Squads CLI                                      [j/k: nav] [q: quit]│
├──────────────────┬──────────────────────────────────────────────────┤
│ Chats            │ Project Discussion                               │
│ ──────────────── │ ─────────────────────────────────────────────── │
│ > Project Disc.  │ John Doe                              10:30 AM  │
│   Team Standup   │ Hello everyone! How is the sprint going?        │
│   Marketing      │                                                  │
│   Dev Channel    │ Jane Smith                            10:32 AM  │
│                  │ Going well! Almost done with the feature.       │
│ Teams            │                                                  │
│ ──────────────── │ John Doe                              10:35 AM  │
│   Engineering    │ Great! Let me know if you need any help.        │
│   Design         │                                                  │
│                  │                                                  │
│                  │                                                  │
│                  │                                                  │
├──────────────────┴──────────────────────────────────────────────────┤
│ > Type message...                                          [Enter] │
└─────────────────────────────────────────────────────────────────────┘
```

**TUI Features:**
- Navigation: vim-style (j/k/h/l) or arrow keys
- Chat list on left, messages on right
- Real-time message updates via WebSocket
- Markdown rendering for messages
- Status bar with keyboard shortcuts
- Search functionality (/)
- Quick reply (Enter)

---

## 3. Implementation Plan

### Phase 1: Core API Library
1. Extract and refactor API code from Squads
2. Create proper error handling with `thiserror`
3. Implement token management
4. Add async HTTP client with `reqwest`

### Phase 2: CLI Commands
1. Set up `clap` for argument parsing
2. Implement authentication commands
3. Implement listing commands (teams, chats, users)
4. Implement message commands (send, read)
5. Add output formatters

### Phase 3: Real-time Features
1. Implement WebSocket connection
2. Add `watch` commands for real-time updates
3. Implement typing indicators

### Phase 4: TUI Mode
1. Set up `ratatui` application structure
2. Implement chat list component
3. Implement message view component
4. Add input handling
5. Connect real-time updates

### Phase 5: Polish
1. Add comprehensive error messages
2. Improve documentation
3. Add shell completions
4. Performance optimization

---

## 4. Dependencies

```toml
[dependencies]
# CLI
clap = { version = "4", features = ["derive"] }

# TUI
ratatui = "0.28"
crossterm = "0.28"

# Async runtime
tokio = { version = "1", features = ["full"] }

# HTTP
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }

# WebSocket
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
futures = "0.3"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Error handling
thiserror = "1"
anyhow = "1"

# Utilities
directories = "5"
chrono = "0.4"
urlencoding = "2"
base64 = "0.22"
uuid = { version = "1", features = ["v4"] }

# Output formatting
tabled = "0.15"  # for table output
colored = "2"     # for colored output
```

---

## 5. Pros and Cons Analysis

### 5.1 CLI-Only Approach (Commands Only)

**Pros:**
- Simple to implement
- Perfect for AI agents and automation
- Easy to integrate with scripts and pipelines
- Low resource usage
- Works over SSH and headless systems
- Easy to test and debug

**Cons:**
- No real-time updates without `watch` commands
- Less intuitive for interactive use
- Multiple commands needed for common workflows
- No visual context for conversations

**Best for:** AI agents (Claude Code, Codex), automation scripts, CI/CD pipelines

### 5.2 TUI Approach (Ratatui)

**Pros:**
- Interactive experience in terminal
- Real-time message updates
- Visual conversation context
- Keyboard-driven efficiency
- Single interface for all operations
- Works over SSH

**Cons:**
- More complex to implement
- Harder to automate (need expect/pexpect)
- Resource overhead from constant updates
- Accessibility challenges
- Terminal compatibility issues

**Best for:** Developers preferring terminal, SSH access, low-resource environments

### 5.3 Hybrid Approach (Recommended)

**Pros:**
- Best of both worlds
- CLI for automation, TUI for interaction
- Shared core library
- Flexible for different use cases
- Can use CLI commands within TUI

**Cons:**
- More development effort
- Larger binary size
- Two interfaces to maintain

---

## 6. AI Agent Integration

### 6.1 Design Principles for AI Agents

1. **Structured Output**: JSON by default for easy parsing
2. **Exit Codes**: Clear error codes for different failures
3. **Stdin Support**: Accept message content from stdin
4. **Idempotent Operations**: Safe to retry commands
5. **Pagination**: Support for large result sets
6. **Quiet Mode**: Suppress progress indicators

### 6.2 Example AI Agent Workflow

```bash
# 1. Check authentication
if ! squads-cli auth status --quiet; then
    echo "Need to re-authenticate"
    exit 1
fi

# 2. Find the relevant chat
CHAT_ID=$(squads-cli chats list --format json | \
    jq -r '.chats[] | select(.title | contains("Project")) | .id' | head -1)

# 3. Get recent messages
MESSAGES=$(squads-cli chats messages "$CHAT_ID" --limit 10 --format json)

# 4. Process with AI and generate response
RESPONSE=$(echo "$MESSAGES" | ai-process-command)

# 5. Send response
squads-cli chats send "$CHAT_ID" "$RESPONSE"
```

### 6.3 MCP (Model Context Protocol) Integration

For deeper AI integration, consider implementing MCP tools:

```json
{
  "tools": [
    {
      "name": "teams_list_chats",
      "description": "List all Microsoft Teams chats",
      "parameters": {}
    },
    {
      "name": "teams_send_message",
      "description": "Send a message to a Teams chat",
      "parameters": {
        "chat_id": "string",
        "content": "string"
      }
    },
    {
      "name": "teams_get_messages",
      "description": "Get messages from a Teams chat",
      "parameters": {
        "chat_id": "string",
        "limit": "number"
      }
    }
  ]
}
```

---

## 7. Important Considerations

### 7.1 Limitations

1. **Organization Accounts Only**: Personal Microsoft accounts not supported (API differences)
2. **Region-Specific**: API endpoints vary by region (EMEA, AMER, APAC)
3. **Unofficial API**: Microsoft may change APIs without notice
4. **No Calling/Video**: Only messaging features
5. **Rate Limits**: Microsoft may throttle requests

### 7.2 Security Considerations

1. **Token Storage**: Tokens stored securely with appropriate permissions
2. **No Plaintext Credentials**: Only OAuth tokens cached
3. **Tenant Isolation**: Tokens scoped to specific tenant
4. **Session Management**: Support for multiple accounts

### 7.3 Legal/Terms of Service

Using unofficial APIs may violate Microsoft's Terms of Service. This tool should be used for personal/educational purposes only.

---

## 8. Implementation Recommendation

For AI agent usage (Claude Code, Codex, OpenCode), I recommend:

1. **Start with CLI-only**: Implement all core commands first
2. **JSON-first design**: Optimize for machine parsing
3. **Add TUI later**: As an optional feature for human users
4. **Prioritize**:
   - Authentication (device code flow)
   - Chat listing and message retrieval
   - Message sending
   - Real-time watching (WebSocket)

This approach delivers maximum value for AI agents quickly while maintaining extensibility for future TUI features.

---

## 9. Next Steps

1. [ ] Set up project structure
2. [ ] Port authentication code from Squads
3. [ ] Implement core API client
4. [ ] Add CLI argument parsing
5. [ ] Implement basic commands
6. [ ] Add real-time features
7. [ ] (Optional) Implement TUI
8. [ ] Documentation and testing
