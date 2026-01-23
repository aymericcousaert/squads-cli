---
name: squads-cli
description: Expert guidance for using squads-cli to manage Microsoft Teams and Outlook operations. Use this skill when the user needs to interact with Teams chats, Outlook mail, or Calendar resources.
metadata:
  short-description: Manage Teams and Outlook via squads-cli
---

# squads-cli Agent Skill

Use this skill to efficiently manage Microsoft Teams and Outlook operations using the `squads-cli` tool.

## Core Capabilities

### 1. Global Search
Search across both Mail and Calendar simultaneously.
- **Command**: `squads-cli search "<query>"`
- **Options**: `--limit <N>`, `--format [json|table|plain]`

### 2. Teams Chats
- **List Chats**: `squads-cli chats list`
- **View Messages**: `squads-cli chats messages <chat-id>`
- **Send Message**: `squads-cli chats send <chat-id> "<content>"`
  - Support for `--markdown` and `--stdin`.
- **Reply**: `squads-cli chats reply <chat-id> --message-id <msg-id> "<content>"`

### 3. Outlook Mail
- **List/Search**: `squads-cli mail list` or `squads-cli mail search "<query>"`
- **Read**: `squads-cli mail read <msg-id>`
- **Send/Draft**: `squads-cli mail send --to <email> --subject <sub...> "<body>"`
- **Management**: `squads-cli mail mark <msg-id> --read`, `squads-cli mail delete <msg-id>`

### 4. Calendar & Availability
- **View Events**: `squads-cli calendar today` or `squads-cli calendar week`
- **Check Availability**: `squads-cli calendar free-busy --users "email1,email2" --date YYYY-MM-DD`
  - *Note*: Times are automatically localized to the system timezone.
- **Calendars**: `squads-cli calendar calendars` (lists all accessible calendars)

## Best Practices for Agents

1. **Structured Output**: Always use `--format json` when you need to parse results programmatically (e.g., extracting `chat-id` or `msg-id`).
2. **Context Discovery**: Start by listing chats or mail to find relevant IDs before performing actions.
3. **Availability Checks**: When scheduling, use `free-busy` first to find common slots.
4. **Markdown**: Prefer `--markdown` for Teams messages to ensure rich formatting (bold, links, code blocks) is preserved.

## Installation / Setup
If the tool is not in the path, it can be installed via:
`squads-cli install` (copies to `~/.local/bin/`)
