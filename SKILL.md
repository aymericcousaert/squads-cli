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
- **List Chats**: `squads-cli chats list` (supports `--limit <N>`)
- **View Messages**: `squads-cli chats messages <chat-id>` (includes reactions column)
- **Send Message**: `squads-cli chats send <chat-id> "<content>"`
  - Support for `--markdown` and `--stdin`.
- **Reply**: `squads-cli chats reply <chat-id> --message-id <msg-id> "<content>"`
  - Support for `--markdown`.
- **React**: `squads-cli chats react <chat-id> --message-id <msg-id> <reaction>` (Supports all Teams emojis by name like `unicornhead`, `meltingface`, or characters like `🦄`)
- **View Reactions**: `squads-cli chats reactions <chat-id> --message-id <msg-id>` (see who reacted to a message)
- **View Mentions**: `squads-cli chats mentions` (find messages where you are @mentioned)
- **List Files**: `squads-cli chats files <chat-id>` (list files shared in a chat)
- **Download File**: `squads-cli chats download-file <chat-id> <file-id> --output ./file.pdf`
  - **Piping**: Use `-o -` to pipe content: `squads-cli chats download-file ... -o - | cat`
- **List Images**: `squads-cli chats images <chat-id>` (list images in chat messages)
- **Download Image**: `squads-cli chats download-image <url> --output ./image.png`

### 3. Outlook Mail
- **List/Search**: `squads-cli mail list` or `squads-cli mail search "<query>"`
- **Read**: `squads-cli mail read <msg-id>`
- **Send/Draft**: `squads-cli mail send --to <email> --subject <sub...> "<body>"`
  - Support for `--markdown` (convert Markdown to HTML) and `--html` (raw HTML).
- **Management**: `squads-cli mail mark <msg-id> --read`, `squads-cli mail delete <msg-id>`

### 4. Calendar & Availability
- **View Events**: `squads-cli calendar today` or `squads-cli calendar week`
- **Check Availability**: `squads-cli calendar free-busy --users "email1,email2" --date YYYY-MM-DD`
  - *Note*: Times are automatically localized to the system timezone.
- **Calendars**: `squads-cli calendar calendars` (lists all accessible calendars)

### 5. User Operations
- **Search Users**: `squads-cli users search "<name or email>"` (find users by name or email)
- **Check Presence**: `squads-cli users presence` (your own presence)
- **Check User Presence**: `squads-cli users presence --user "<email>"` (specific user)
- **Check Multiple Users**: `squads-cli users presence --users "email1,email2"` (multiple users)

### 6. Teams Channels
- **List Teams**: `squads-cli teams list`
- **List Channels**: `squads-cli teams channels <team-id>`
- **View Messages**: `squads-cli teams messages <team-id> <channel-id>` (includes reactions column)
- **Post to Channel**: `squads-cli teams post <team-id> <channel-id> "<message>"`
  - Support for `--subject`, `--markdown`, and `--stdin`.
- **Reply in Channel**: `squads-cli teams reply <team-id> <channel-id> --message-id <id> "<reply>"`
  - Support for `--markdown` and `--html`.
- **React in Channel**: `squads-cli teams react <team-id> <channel-id> --message-id <id> <reaction>`
  - Reactions: like, heart, laugh, surprised, sad, angry, skull, hourglass
  - Use `--remove` to remove a reaction
- **List Images**: `squads-cli teams images <team-id> <channel-id>` (list images in channel messages)
- **Download Image**: `squads-cli teams download-image <url> --output ./image.png`

### 7. Unified Feed
- **View All Activity**: `squads-cli feed` (combined view of chats and emails)
- **Filter by Mentions**: `squads-cli feed --mentions-only` (only items where you are @mentioned)
- **Filter Unread**: `squads-cli feed --unread`

### 8. Personal Notes
Shortcut to manage your personal "Notes" chat.
- **List Notes**: `squads-cli notes list`
- **Add Note**: `squads-cli notes add "<content>"` (Supports `--markdown` and `--stdin`)
- **Delete Note**: `squads-cli notes delete <msg-id>`

## Best Practices for Agents

1. **Structured Output**: Always use `--format json` when you need to parse results programmatically (e.g., extracting `chat-id` or `msg-id`).
2. **Context Discovery**: Start by listing chats or mail to find relevant IDs before performing actions.
3. **Availability Checks**: When scheduling, use `free-busy` first to find common slots.
4. **Markdown**: **ALWAYS** use `--markdown` when your message contains formatting characters like `**bold**`, `` `code` ``, or ` ``` ` code blocks. Without this flag, these characters are sent as literal text and won't render properly in Teams or Outlook emails.
5. **Check Presence Before Reaching Out**: Use `squads-cli users presence --user "<email>"` to check if someone is Available/Busy/Away before messaging.
6. **Find Users by Name**: Use `squads-cli users search "John"` to find user email/ID for messaging.
7. **Monitor Mentions**: Use `squads-cli chats mentions` or `squads-cli feed --mentions-only` to find messages that need your attention.
8. **Access Shared Content**: Use `squads-cli chats images` and `squads-cli chats files` to list and download content shared in chats.
9. **Monitor Reactions for Feedback**: Use `squads-cli chats messages` to see reactions summary, or `squads-cli chats reactions` for detailed info on who reacted. Reactions like thumbs up indicate approval/acknowledgment.
10. **Writing Style**: Refer to `WRITING_STYLE.md` in this directory to understand and mimic the user's communication style (tone, vocabulary, formatting) when sending messages or replies.
11. **Piping Files**: When needing to read file content (like .txt, .md, .csv, .json) from a chat, prefer using piping (`-o -`) to process it directly in memory rather than saving to a temporary file. Example: `squads-cli chats download-file ... -o - | cat`.

## Authentication

- **Login**: `squads-cli auth login`
  - Automatically opens browser for authentication
  - `-c, --copy-code`: Copy the auth code to clipboard automatically
  - `--no-browser`: Disable automatic browser opening
- **Check Status**: `squads-cli auth status`
- **Refresh Tokens**: `squads-cli auth refresh`
- **Logout**: `squads-cli auth logout`

## Installation / Setup
- **Install**: `squads-cli install` (copies to `~/.local/bin/`)
- **Update**: `squads-cli update` (downloads latest release from GitHub)
  - Automatically detects platform (Linux/macOS/Windows)
  - Downloads pre-built binary - no build tools required
