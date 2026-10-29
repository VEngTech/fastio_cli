# fastio

A command-line interface for the [Fastio](https://fast.io) cloud storage platform. Built in Rust for speed, reliability, and cross-platform support.

## Installation

### npm (recommended)

```bash
npm install -g @vividengine/fastio-cli
```

Or run without installing:

```bash
npx @vividengine/fastio-cli --help
```

### Shell script

```bash
curl -fsSL https://raw.githubusercontent.com/VEngTech/fastio_cli/main/install.sh | sh
```

### Binary download

Pre-built binaries for macOS, Linux, and Windows are available on the [Releases](../../releases) page.

### From source

Requires Rust 1.88+ (edition 2024):

```bash
cargo install --path .
```

## Quick Start

```bash
# Log in (opens browser for secure authentication)
fastio auth login

# Check auth status
fastio auth status

# List organizations
fastio org list

# Create a workspace
fastio workspace create --org <org_id> "My Workspace"

# Upload a file
fastio upload file --workspace <workspace_id> ./document.pdf

# Download a file
fastio download file --workspace <workspace_id> <node_id> --output ./downloads/

# Ask the AI about your workspace
fastio ai chat --workspace <workspace_id> "What files do I have?"

# Log out
fastio auth logout
```

## Authentication

The CLI supports multiple authentication methods, checked in this order:

1. `--token` flag (one-off bearer token)
2. `FASTIO_TOKEN` environment variable
3. `FASTIO_API_KEY` environment variable
4. Stored credentials from `--profile` (or default profile)

### Browser Login (Recommended)

```bash
fastio auth login

# Request an admin (`rwa`) access mode, or a read-only one
fastio auth login --admin
fastio auth login --read-only

# Also request permission to change account settings (`userdetails:*:rw`)
fastio auth login --account-settings
```

`auth login` opens your browser to Fastio's sign-in page (including 2FA, if enabled on your account)
and receives the result on a short-lived local callback. Tokens are stored locally and automatically
refreshed.

`--admin` and `--read-only` request a **ceiling**, not a role: the consent page may grant less than you asked for. Run `fastio auth scopes` afterwards for the live view of what the credential may do — `fastio auth status` only shows the scopes cached when the profile signed in.

No browser on this machine? Use `--no-browser`:

```bash
fastio auth login --no-browser
```

This prints the sign-in URL instead of opening it. Open it on any device, sign in, and paste the
code it displays back into the CLI prompt.

Password sign-in from the CLI has been removed — it's handled on the browser sign-in page above. For
unattended or CI use, create an API key instead (below).

### API Key

```bash
# Create an API key
fastio auth api-key create --name "CI pipeline"

# Create one scoped to a single organization, with admin access
fastio auth api-key create --name "release bot" --org 1234567890123456789 --admin

# Use it for subsequent commands
export FASTIO_API_KEY=your-key-here
fastio org list
```

Scopes are `entity_type:entity_id:access_mode` strings, where the access mode is `r`, `rw`, or `rwa` (admin). `--org` / `--workspace` / `--share` (repeatable) and `--all` select the entities, `--admin` / `--read-only` set the access mode, and `--account-settings` adds `userdetails:*:rw` for password, email, and 2FA-enrolment changes — `rwa` alone does not grant those. An `api-key update` **replaces** the key's entire scope set, so run `fastio auth api-key get <key-id>` first and re-state every scope it should keep. If those scopes do not all use the same access mode, re-state them with the raw `--scopes '[...]'` form: `--admin` and `--read-only` apply to every entity in the request, so neither can express a mixed set. A blanket `--admin` escalates a read-only scope to admin along with the one you meant to raise; a blanket `--read-only` downgrades a writable or admin scope along with the one you meant to narrow. `fastio auth oauth narrow <session-id>` gives up authority on a login session and takes effect at its next token refresh.

### Service Account Keys

For automation that should not act as a person, a workspace admin or owner can create a **service account** — a non-human member of one workspace with a `viewer` or `editor` role — and give the job its key:

```bash
# As the admin: create the account and its first key (the secret is printed once)
fastio service-account create --workspace 1234567890123456789 --name ci-bot --role editor

# In the job: use the secret like any API key
export FASTIO_API_KEY=fio_sa_...
fastio auth status
```

A service account key is limited to its one workspace.

Roles: `viewer` (read, download, search, AI chat) or `editor` (viewer plus upload, edit, move, rename, trash, metadata, comments).

Management and sharing operations (members, invitations, creating shares, File Shares, sign, imports, org changes, permanently purging files or emptying the trash, key/account management) return HTTP `403` with reason `service_account_not_permitted`. Identity reads (`auth status`, `auth check`, `workspace list`) work, and public share links behave as if the request were anonymous.

The secret cannot be retrieved again; if it is lost, `fastio sa key create` a new one and `fastio sa key revoke` the old one.

### Two-Factor Authentication

```bash
# Check 2FA status
fastio auth 2fa status

# Enable 2FA
fastio auth 2fa setup --channel totp

# Verify a pending 2FA code
fastio auth 2fa verify --code <CODE>
```

2FA during `auth login` is handled on the browser sign-in page, not in the CLI — these commands are
for managing your 2FA settings (enrollment, status, disabling).

## Output Formats

All commands support `--format` to control output:

```bash
# Table format (default for terminals)
fastio org list

# Markdown format (default when piped; optimized for LLM consumers)
fastio org list | cat

# JSON format (for scripts that parse structured output)
fastio org list --format json

# CSV format
fastio org list --format csv

# Explicit markdown (GitHub-flavored, byte-equivalent to the server's ?output=markdown)
fastio org list --format markdown
# `--format md` is accepted as an alias

# Filter specific fields
fastio org list --fields name,id,description
```

Markdown replaced JSON as the non-TTY default on 2026-04-15. The
rendered output is **byte-equivalent** to what the Fastio API produces
for `?output=markdown`: a `**Result:** success|failure` preamble,
object-valued errors promoted to a leading `# Error` section, and each
remaining top-level key as an H1 section in insertion order. Arrays of
associative records render as GFM pipe tables (union of keys in
insertion order); scalar lists render as bulleted lists.

Note on escaping: bullet values and heading text are **not** escaped —
the renderer takes a light-touch approach matching the server contract,
because the output is meant to be read or rendered, not embedded into
other markdown. **Downstream consumers that render the output to HTML
MUST sanitize.** The renderer does strip Unicode bidi, zero-width, and
C0/C1 control characters as a Trojan-Source defense; table cells escape
`|`, `\`, `` ` ``, and newlines; HTML-like cell content is wrapped in
inline-code fences.

Pipelines that need machine-parseable output can opt back in with
`--format json`.

## Commands

### Authentication & User

| Group | Description |
|-------|-------------|
| `auth` | Login, logout, 2FA, API keys, OAuth sessions |
| `user` | User profile, search, assets, invitations |
| `configure` | CLI profiles and settings |

### Organizations & Workspaces

| Group | Description |
|-------|-------------|
| `org` | Org CRUD, billing, members, transfer tokens, discovery, assets |
| `workspace` | Workspace CRUD, search, limits, `jobs-status` (poll async jobs) |
| `member` | Workspace/share member management |
| `service-account` | Workspace service accounts — non-human members that use API keys (alias `sa`): list, create, info, update, disable, enable, delete, and `key list/create/revoke`. Admin/owner only; secrets are shown once |
| `invitation` | List, accept, decline, delete invitations; `join` accepts or declines one by its email key |
| `dashboard` | Per-workspace dashboard card feed: get, dismiss, undismiss |

### Files & Storage

| Group | Description |
|-------|-------------|
| `search` | Unified search — one query across a workspace or share, grouped into `files` / `metadata` / `comments` buckets. Start here for any "find X" |
| `files` | List, create folders, move, copy, rename, delete, trash, versions, search (flat file list, `--workspace` or `--share`), `content` (a file's extracted text as addressable chunks — relevance `--query`, `--page`, `--chunk-from/--chunk-to`, or `--nodes` to score up to 10 files at once), lock |
| `upload` | File upload (chunked with progress), text upload, URL import, session management |
| `download` | File download (streaming with progress), folder ZIP, batch |
| `lock` | Acquire, check status, heartbeat, release file locks |
| `metadata` | Metadata extraction and search: `eligible`, `details`, `search`, and async single-file `extract` (SPENDS AI CREDITS — needs `--confirm-ai-spend`; `--wait` polls to a terminal state) |

### Shares & Collaboration

| Group | Description |
|-------|-------------|
| `share` | Share CRUD, files, members, password auth |
| `fileshare` | File Shares — durable single-file link shares (replaces the retired QuickShare): create/list/info/update/delete, grants, download/versions/preview, upload write-back, activity, ws-token |
| `comment` | Comments, replies, reactions, attachments; thread `resolve` / `reopen` / `assign` / `unassign`; list filters `--status`, `--for-me`, `--kind`, `--include-suggestions`, `--threads`; `--quote` anchors to a passage of a note |
| `suggestion` | Suggested edits on notes: `create` (`--quote` with `--replacement` or `--delete`), `list`, `reply`, `edit`, `withdraw`, `accept`, `reject`. `accept` rewrites the note and exits `5` when it can only be accepted in the editor |
| `export` | Export a note to pdf, docx, odt, html, txt, or md (`--to`), with optional comments and tracked changes; download it (`--output`) or save a copy to the workspace (`--save-copy`); `export status` resumes a job |
| `event` | Activity events, search, polling, org-wide storage change feed (`event changes`) |
| `preview` | File preview URLs and transforms |
| `asset` | Org/workspace/user asset management |

### AI

| Group | Description |
|-------|-------------|
| `ripley` | Fastio's delegated AI agent — ask, chat, history, message management, `summary`, `transactions` (`ai` is a hidden alias). `--effort normal\|high` picks the reply effort per message. Searching is not a Ripley verb — use `search` |

### Platform

| Group | Description |
|-------|-------------|
| `apps` | App listing, details, launching |
| `import` | Cloud import providers, identities, sources, jobs, writebacks |
| `intents` | Agent Intents — announce what you are doing so peers see it before they collide: `allocate`, `list`, `fill`, `get`, `release` |
| `mcp` | Start built-in MCP server for AI agents |
| `completions` | Generate shell completions (bash, zsh, fish, PowerShell) |

### Utilities

| Group | Description |
|-------|-------------|
| `how-to` | Grounded "how do I…" answers about Fastio itself (alias `howto`) |
| `id` | Inspect Fastio identifiers offline (no auth, no network): classify an id's entity type and family |
| `view` | Render a single note or `.md` file node in the terminal; `--include-suggestions` also returns a note's pending suggested edits |
| `skill` | Print the built-in agent skill guide |
| `system` | API health checks: `ping`, `status` (no auth required) |

## MCP Server Mode

The CLI includes a built-in [Model Context Protocol](https://modelcontextprotocol.io) server for AI agent integration. Run it as a subprocess:

```bash
fastio mcp
```

This exposes the CLI's action-routed tool surface as MCP tools over stdio, compatible with Claude Desktop, VS Code, and other MCP-compatible clients. A few high-risk or local-byte operations (sign send/void, fileshare upload write-back and ws-token, and service account management, which mints secrets) remain CLI-only.

Tool responses are rendered as GitHub-flavored Markdown by default,
byte-equivalent to the Fastio API's `?output=markdown` output.
Markdown is substantially more token-efficient for LLMs than
pretty-printed JSON, so the MCP server uses it for every read and
status response.

```json
{
  "mcpServers": {
    "fastio": {
      "command": "fastio",
      "args": ["mcp"]
    }
  }
}
```

Filter which tools are available:

```bash
fastio mcp --tools auth,org,workspace,files,upload,download
```

Authentication and backend follow the standard CLI rules: the server honors the
global `--api-base`, `--token`, and `--profile` flags (and the `FASTIO_TOKEN` /
`FASTIO_API_KEY` env vars), so you can point it at a non-default backend or
profile — e.g. `fastio --profile staging mcp` or
`fastio --api-base https://api.example/current mcp`.

## Shell Completions

Generate shell completions for your shell:

```bash
# Bash
fastio completions bash > ~/.bash_completion.d/fastio

# Zsh
fastio completions zsh > ~/.zfunc/_fastio

# Fish
fastio completions fish > ~/.config/fish/completions/fastio.fish

# PowerShell
fastio completions powershell > _fastio.ps1
```

## Profiles

Manage multiple accounts with named profiles:

```bash
# Set up a profile interactively
fastio configure init

# Log in to a specific profile
fastio auth login --profile work

# Use a profile for a command
fastio org list --profile work

# Set default profile
fastio configure set-default work

# List all profiles
fastio configure list
```

## Global Options

| Flag | Description |
|------|-------------|
| `--format json\|table\|csv\|markdown` | Output format (markdown is the default when piped) |
| `--fields name,id,...` | Filter output fields |
| `--detail terse\|standard\|full` | Server-side response verbosity via `?output=<detail>`; when omitted the server returns its `full` shape |
| `--no-color` | Disable colored output |
| `--quiet` / `-q` | Suppress all output |
| `--verbose` / `-v` | Increase verbosity; repeatable (`-v` info, `-vv` debug, `-vvv` trace API calls) |
| `--profile <name>` | Use named profile |
| `--token <jwt>` | One-off bearer token |
| `--api-base <url>` | Override API base URL |
| `--version` / `-V` | Print version (root command only, e.g. `fastio --version`) |

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error (details on stderr) |
| `2` | Invalid arguments |
| `3` | Compare-and-swap conflict — the version or thread state you sent is no longer current; re-read and try again |
| `5` | The suggestion could not be accepted with the note closed — open the note to accept it, or follow the hint (try again shortly when the note is busy; re-read first when an earlier attempt's outcome is unknown) |

## Configuration

Configuration files are stored in the platform config directory, under a `fastio-cli` folder: `$XDG_CONFIG_HOME/fastio-cli` (typically `~/.config/fastio-cli`) on Linux, `~/Library/Application Support/fastio-cli` on macOS, and `%APPDATA%\fastio-cli` on Windows:

| File | Purpose |
|------|---------|
| `config.json` | Profile settings and API base URL |
| `credentials.json` | Stored authentication tokens |

## License

Apache License 2.0. See [LICENSE](LICENSE) for details.
