# Fast.io CLI — Agent Guide

Use this guide when interacting with the Fast.io platform via the `fastio` CLI
(and its built-in MCP server, `fastio mcp`).

## Offload to Ripley first

Fast.io ships a delegated subagent — **Ripley**, Fast.io's AI agent. Before
firing a long chain of low-level primitives, consider asking Ripley to find or
do the multi-step work for you. Ripley acts **on the user's behalf** (it carries
the caller's JWT and runs in a sandbox), so it can search, read, summarize, and
chain operations across a workspace or share that would otherwise take many
round-trips.

```bash
# Ask Ripley a question and get the answer (creates a chat, waits for the result)
fastio ripley ask --workspace WS_ID "Which contracts mention auto-renewal?"

# Same, scoped to a share
fastio ripley ask --share SHARE_ID "Summarize the latest revision of the proposal"
```

Heuristics for agents:

- **Prefer one `ripley ask` over many primitives** when the task is "find/answer/
  summarize across content." Reserve the raw `files`/`storage`/`search` commands
  for deterministic, single-shot operations where you already know the IDs.
- **Poll activity, not detail.** For anything that runs asynchronously (a Ripley
  answer, a metadata extraction job), watch the activity/state endpoint with a
  bounded wait — do **not** tight-loop a `--detail full` read. The `ask` and
  `metadata extract --wait` paths already do bounded activity-polling for you.
- **Pick the reply effort per message.** `--effort normal` (default) or
  `--effort high` on `ripley ask` / `ripley chat` (MCP: `effort` on `ask`,
  `chat-create`, `message-send`). `high` reasons more deeply with no per-reply
  step limit, but is slower and uses more credits. Effort applies to one
  message — pass it again on each follow-up that should use it.
- **`turn_limit_reached: true` means "continue", not "failed".** A `normal`
  reply that used up its per-reply step budget wraps up early and may be
  incomplete; the chat is intact. Send an ordinary follow-up such as
  `Continue` in the same chat (`fastio ripley chat --workspace WS_ID
  --chat-id CHAT_ID "Continue"`, or MCP `message-send` with
  `query_text="Continue"`) and the new reply picks up where it left off.

`ripley` is the former `ai` group — **`ai` still works as a hidden alias** (CLI
and MCP) for backward compatibility, but new code should use `ripley`.

> **Deferred / pending.** The Ripley delegated-**job** lifecycle
> (`ripley delegate` / `run` / `status` / `logs` / `cancel-job`) is **not yet
> available** — it is pending the server-side delegation contract, which has not
> been finalized. Those verbs are hidden stubs that call no endpoint and exit
> with a "not yet available" message. Until the contract ships, delegate work via
> `ripley ask` / `ripley chat`, which run today.

## Authentication

Authenticate before using any command. Two methods:

```bash
# Option 1: API key (best for agents/automation)
fastio --token YOUR_API_KEY auth check

# Option 2: browser login (interactive)
fastio auth login
# Opens a URL → sign in (including 2FA, if enrolled) in the browser → done

# No browser available? Print the URL, sign in elsewhere, paste the code back
fastio auth login --no-browser
```

For automation, pass `--token` on every command or set `FASTIO_TOKEN`:

```bash
export FASTIO_TOKEN=your_api_key
fastio org list
```

### Scopes and access modes

Every credential carries a list of **entity scopes**, each written
`entity_type:entity_id:access_mode` — for example `org:1234567890123456789:rw`,
`workspace:9876543210987654321:r`, or `user:*:rwa` for the whole account. The
access mode is one of:

- `r` — read only
- `rw` — read and write (the default mode once an entity is selected)
- `rwa` — read, write, and **administrative** operations on that entity

`rwa` is an access mode, not a role. Holding it does not change who the human
is, and it never grants more than the account already has — the user's own
permissions still apply on top of it.

### Requesting admin as a ceiling

`--admin` asks for the `rwa` access mode. It is a **ceiling, not a grant** — it
is the most the resulting credential may be given. What happens to a request
that reaches for more than it may have depends on where it is issued, and the
two rules are different:

- **`fastio auth login` — consent may narrow it.** The ceiling is what the
  browser session asks for; the consent page may approve less, and the
  credential ends up with whatever was approved. So the result can be smaller
  than the request.
- **`api-key create` / `api-key update` / `oauth narrow` — the issuer contains
  it.** The request is sent exactly as written, and the server **rejects** it
  when it is wider than the credential doing the issuing. It is never quietly
  trimmed: the call either fails, or returns precisely the scopes you asked for.

Either way, confirm what you actually got:

```bash
fastio auth login --admin    # request an admin ceiling, approve it in the browser
fastio auth scopes           # live view of what the credential may now do
```

`--read-only` is the same idea in the other direction: it asks for `r`. The two
are mutually exclusive, and both apply to `auth login` (browser/PKCE login
only), `auth api-key create`, `auth api-key update`, and `auth oauth narrow`.

**Selectors choose *what* the scopes cover; the access mode chooses *how much*.**
`--org`, `--workspace` and `--share` each take an entity id and may be repeated;
`--all` selects the whole account (`user:*`). Once at least one selector is
given, the access mode defaults to `rw` unless `--admin` or `--read-only` says
otherwise, and it applies to every entity in that request. **Give no selector at
all** — no `--org` / `--workspace` / `--share` / `--all`, no `--account-settings`
and no raw `--scopes` — and no scope list is sent: the credential is
**unscoped**, covering the whole account with read and write but **not**
administrative access. `rw` is the default *mode*, not what an unscoped
credential is written as.

### Account settings are a separate scope

Changing the account itself — password, email address, enrolling a second
factor (`auth 2fa setup` / `auth 2fa verify-setup`), and invalidating every
session — needs the `userdetails:*:rw` scope, requested with
`--account-settings`. **`rwa` does not grant it.** Administrative access to
organizations and account-settings access are orthogonal; ask for each one
explicitly. 2FA during `auth login` itself happens on the browser sign-in
page, not through the CLI, so it is unaffected by this scope; `auth 2fa send`
/ `auth 2fa verify` manage a pending 2FA code outside of login and are also
not gated by it.

### Issuing and narrowing credentials

```bash
# A key that administers one org (structured flags share one access mode)
fastio auth api-key create --name "release bot" \
  --org 1234567890123456789 --admin

# Mixed access modes need the raw form: admin on one org, read on one workspace
fastio auth api-key create --name "release bot" \
  --scopes '["org:1234567890123456789:rwa","workspace:9876543210987654321:r"]'

# Give up authority on an existing login session
fastio auth oauth narrow SESSION_ID --org 1234567890123456789 --read-only
```

Points that bite agents:

- **An `api-key update` REPLACES the key's entire scope set.** There is no
  "add one scope" call. Run `fastio auth api-key get <key-id>` first, then
  re-state every scope the key should keep alongside the new one.
- **Key changes apply immediately; OAuth narrowing applies at the session's
  next token refresh.** An access token already issued keeps its original
  scopes until then.
- **`auth oauth narrow` can only give up authority**, never add it — the server
  refuses anything wider than the session already holds. To widen, sign in
  again with the ceiling you need.
- **Raw `--scopes '<json array>'` still works** for anything the structured
  flags do not express, and is mutually exclusive with them.
- **Keys issued before scopes existed** (no scope list at all) behave as
  `user:*:rw`: full read and write, **not** administrative, and **not** able to
  change account settings. Re-issue them with the access they need.
- **`full_access` does not imply admin.** It means the credential covers the
  whole account with write access rather than being narrowed to particular
  entities. The access mode is reported separately — read it there.

### Seeing what a credential may do

- `fastio auth scopes` asks the server. Authoritative and current — use this.
- `fastio auth status` shows the scopes cached locally when the profile signed
  in, so it can be stale if the grant has been narrowed since.

### When a call is refused for scope

Scope refusals are HTTP `403` carrying a machine-readable reason, and the CLI
prints a specific remedy for each. Repeating the *same* request with the same
credential never helps — something has to change: either the credential is
re-issued or re-consented with the access it lacks, or the request itself is
narrowed to something that credential can already satisfy.

- **`scope_admin_required`** — the credential covers the entity but without
  `rwa`. For an API key, read it with `api-key get`, then run `api-key update`
  re-stating its scopes plus `--admin`, from a credential that already holds
  that access. If those scopes do not all share one access mode, re-state them
  with the raw `--scopes '[...]'` form instead: `--admin` applies to every
  entity in the request, so a blanket flag would escalate a read-only scope
  along with the one you meant to raise. For a login or a web session, run
  `fastio auth login --admin` and approve admin access on the consent page.
- **`scope_exceeds_issuer`** / **`access_mode_exceeds_initiate`** — you asked
  to mint something broader than the credential doing the minting. Narrow the
  request, or issue it from a credential that already holds the access you
  asked for: a web session, or a fresh login with the ceiling you need.
- **`userdetails_scope_required`** — an account-settings operation from a
  credential without `userdetails:*:rw`. Run
  `fastio auth login --account-settings`, or add `--account-settings` when
  re-issuing the key (remembering that a key update replaces the whole scope
  set). `--admin` is not the fix here.
- **`scope_write_required`** — every access mode on the credential is `r`, so
  it is read-only and cannot perform account operations (creating an org,
  updating the user, revoking sessions, signing out). Sign in again *without*
  `--read-only`, or re-issue the key read-write (again: an update replaces the
  whole scope set, so read it back with `api-key get` first). Write is not
  admin — `--admin` asks for a higher ceiling than this needs and is not the
  fix. Sign-out is the one special case: a read-only credential cannot revoke
  its own session, so `fastio auth signout` discards the credential locally,
  reports `server_signout_completed: false`, and exits `0`.

Changing a credential's scopes is a compare-and-swap: `auth api-key update` and
`auth oauth narrow` write against the set they read, so a concurrent change
returns HTTP `409`. Do not retry the same request — re-read the credential
(`auth api-key get <key-id>` or `auth oauth details <session-id>`) and resend
the full intended set, because an update replaces the whole set and a narrow is
measured against what the session holds now.

Full endpoint reference: <https://api.fast.io/current/llms/full/>.

## Output Format and verbosity

Two orthogonal knobs control output:

- `--format` controls **client-side rendering**: `table` (human), `json`
  (programmatic), `csv` (spreadsheets), `markdown`/`md` (GitHub-flavored).
  Default is `table` for a TTY and `markdown` for pipes (changed from `json` on
  2026-04-15 to benefit LLM consumers; pass `--format json` for the old shape).
- `--detail terse|standard|full` controls **server-side verbosity** by passing
  `?output=<detail>` on supported endpoints — i.e. how much data the API returns.
  It is independent of `--format`. Omitting it uses the server's `full` shape.

```bash
fastio org list --format json
fastio files list --workspace WS_ID --detail terse --format json
```

Use `--quiet` to suppress output (useful for write operations where you only
care about the exit code).

### Viewing markdown

`fastio view <workspace_id> <node_id>` renders a markdown note (or a `.md`
file) in the terminal. It always emits rendered markdown — or verbatim with
`--raw`, when piped, or with `--no-color` — and ignores `--format`/`--fields`.
Only note nodes and markdown files are supported; other file types are rejected.
Add `--include-suggestions` (notes only, not combinable with `--raw`) to also
get the note's pending suggested edits. With `--format json` or `markdown` the
note `content` and its `suggestions` come back together as structured output.
With `table`, the note is printed as plain `view` prints it, then the
suggestions table. With `csv`, only the suggestion rows are emitted (a stderr
note points to json/markdown for the note text).

### Inspecting identifiers (offline)

`fastio id info <ID>...` classifies one or more Fast.io `OpaqueId`s **offline**
(no auth, no network) so you know what an id refers to before acting on it —
handy when an id arrives in a webhook, event, or payload. It reads the
self-describing length and type prefix: 1-char-type ids are 29 chars; 2-char-type
ids are 30 chars (this includes the legacy workflow family under `w`, retired
2026-07 but still decodable for ids minted before the sunset). It reports the
`entity_type`, `family`, `surfacing` tier, and a `recognized` flag. Output is
always an array of records, so `--format json|table|csv|markdown` and `--fields`
all apply.

```bash
fastio id info 2abcd-efghj-kmnpq-rstuv-wxyz3-4567 NODE_ID2    # → StorageNode (hyphens OK)
```

Over MCP this is the `id` tool (`action: "info"`, params `id` or `ids`). A
29-char id whose 1-char code is unmapped is reported `unknown` (it may be a
transitional code pending reassignment), never guessed.

## Important: Deep Indexing Setting

Workspaces have a Deep Indexing toggle (`--deep-indexing`). When **ON**,
documents are indexed with embeddings for AI-powered search, chat, and
summarization. When **OFF** the workspace is pure storage. A new workspace takes
the platform default (on) unless you pass `--deep-indexing false`. Ingestion is
expensive (per-page cost), so turn it off on workspaces you do not intend to
query.

```bash
# Storage-only
fastio workspace create "File Storage" --org ORG_ID --deep-indexing false

# AI/RAG use case (platform default)
fastio workspace create "Knowledge Base" --org ORG_ID

# Toggle Deep Indexing on an existing workspace
fastio workspace update WS_ID --deep-indexing true

# Keep Deep Indexing but stop extracting metadata from new uploads
fastio workspace update WS_ID --metadata-extraction false
```

`share create` and `share update` accept the same `--deep-indexing` flag;
`share create` sends `false` when it is omitted.

`--metadata-extraction` is a separate opt-OUT layered under Deep Indexing: it
can withhold automatic extraction from newly uploaded files, but it can never
enable extraction where the Deep Indexing setting or the plan does not allow it.
It defaults to on, it deletes nothing, and explicit per-file extraction requests
are unaffected. It is accepted on `workspace create`, `workspace update` and
`org create-workspace`.


## Core Workflows

### Organizations, workspaces, files

```bash
fastio org list --format json
fastio workspace list --org ORG_ID --format json

fastio files list --workspace WS_ID --folder NODE_ID --format json
fastio files search --workspace WS_ID --limit 25 "query" --format json
fastio files create-folder --workspace WS_ID --parent NODE_ID --name "My Folder"
fastio files delete --workspace WS_ID NODE_ID        # → trash
fastio files purge  --workspace WS_ID NODE_ID        # permanent

# A file's EXTRACTED TEXT (what search and AI indexed), not its raw bytes
fastio files content --workspace WS_ID NODE_ID --query "termination clause"
fastio files content --workspace WS_ID NODE_ID --chunk-from 40 --chunk-to 42
```

### Upload / download

```bash
fastio upload file --workspace WS_ID --folder FOLDER_NODE_ID ./path/to/file
fastio upload text --workspace WS_ID --name "notes.txt" "File content here"
fastio upload url  --workspace WS_ID --url "https://example.com/file.pdf"

fastio download file   --workspace WS_ID --node-id NODE_ID
fastio download folder --workspace WS_ID --node-id FOLDER_NODE_ID   # ZIP
```

### Shares (data rooms)

```bash
fastio share list --format json
fastio share create "Share Name" --workspace WS_ID
fastio share info SHARE_ID --format json
fastio share guest-auth SHARE_ID
```

### Ripley (AI agent)

Requires Deep Indexing enabled on the workspace for content-aware queries.

```bash
fastio ripley ask --workspace WS_ID "your question"      # headline verb
fastio ripley ask --workspace WS_ID --effort high "a harder question"
fastio ripley chat --workspace WS_ID --chat-id CHAT_ID "Continue"   # follow-up
fastio ripley list --workspace WS_ID --kind all          # chats
fastio ripley details CHAT_ID --workspace WS_ID
fastio ripley messages CHAT_ID --workspace WS_ID
fastio ripley summary --workspace WS_ID NODE_ID1 NODE_ID2     # AI share
```

### Unified search

`fastio search` runs one query across grouped result buckets (files, metadata,
comments) for a workspace or share:

```bash
fastio search workspace WS_ID "query" --format json
fastio search share SHARE_ID "query" --only files,comments
```

`fastio files search` remains the targeted file-only search. Add `--details` to
either form to attach extracted metadata facts to file and note hits in the
files bucket (workspace only; a share accepts the flag and returns no facts).

### Watching an org for storage changes

`fastio event changes` reads one page of the org-wide storage change feed: one
call and one cursor cover every workspace and share of the org you can read.
It is a single call — loop it yourself:

```bash
# 1. Bootstrap: no cursor. Returns no changes, a head cursor, and profiles.items.
fastio event changes --org-id ORG_ID --format json
# 2. Immediately list every workspace/share in profiles.items in full.
# 3. Follow: send the returned cursor back unchanged.
fastio event changes --org-id ORG_ID --cursor CURSOR --format json
```

- `has_more: true` → call again now. `has_more: true` with the SAME cursor you
  sent → the newest changes are still settling; wait ~10 seconds first.
  `has_more: false` → caught up; poll again later. An empty page with
  `has_more: true` is not the end.
- Dedupe by `event_id`; a row can be redelivered and the latest copy wins.
- `profiles.version` changed → re-list `profiles.items`. An expired or invalid
  cursor → re-bootstrap and re-list.
- Reconcile with a periodic full listing: the feed has no purge events, reports
  a rename as `_updated`, can miss some share-only operations and very late
  commits, and never covers user-owned shares.

Over MCP this is the `event` tool's `changes` action (`org_id` required).

### Reading a document's text (locate and quote)

`fastio files read` returns a file's raw bytes. `fastio files content` returns
its **extracted text** as ordered chunks — the same text the platform indexed
for search and AI — so it is what actually reads a PDF's words. Each chunk
carries a `position`, which is its address.

To find a passage and quote it accurately:

```bash
# 1. Locate it. The hit carries best_chunk.position and best_chunk.indexed_version_id.
fastio files search --workspace WS_ID "termination clause" --detail standard --format json

# 2. Read that passage in context (P-1 .. P+1 around best_chunk.position P).
fastio files content --workspace WS_ID NODE_ID --chunk-from 39 --chunk-to 41 --format json

# 3. Confirm the response's indexed_version_id MATCHES the search hit's before
#    quoting. If it differs the file was re-indexed — re-run the search.
#    A hit whose best_chunk.position is null has no address — use --query
#    on that file instead of a chunk range.
```

When `best_chunk.same_as_snippet` is true its `text` is null at
`--detail terse`/`standard`; read `content_snippet` on the hit instead.
`--detail terse` on `files content` returns the chunk map with no text.

```bash
# Score up to 10 candidate files against one question in a single request
# (workspace only; scores are comparable only within one file, never across files).
fastio files content --workspace WS_ID --nodes NODE_A,NODE_B,NODE_C --query "renewal terms"

# Walk a whole file: pass the previous response's next_cursor back verbatim,
# and stop when it is null.
fastio files content --workspace WS_ID NODE_ID --cursor "NEXT_CURSOR"
```

`indexed: false` is a normal answer, not a failure — that version has no
extracted text yet, or carries none at all.

## Comment threads and suggested edits

A root comment and its replies form a **thread**, which can be resolved,
reopened, and assigned to one person. `--entity-type` is `workspace`, `share`,
or `fileshare`; `--entity-id` is that workspace or share id.

```bash
# Open threads assigned to you or mentioning you (--for-me=agents: assigned through one of your agents)
fastio comment list-all --entity-type workspace --entity-id WS_ID --status open --for-me --threads

fastio comment resolve COMMENT_ID [--if-state-version N]   # root comments only; repeating is harmless
fastio comment reopen  COMMENT_ID [--if-state-version N]
fastio comment assign  COMMENT_ID --self                   # or --ripley, or --user-id USER_ID
fastio comment unassign COMMENT_ID
fastio comment reply COMMENT_ID "Done" --node-id NODE_ID \
  --entity-type workspace --entity-id WS_ID --reopen       # --reopen also reopens a resolved thread
```

- **List filters** (`comment list` and `comment list-all`): `--status
  open|resolved|all`, `--for-me` (or `--for-me=agents`), `--kind
  comment|suggestion|all`, `--include-suggestions` (suggestion threads are
  hidden unless asked for), and `--threads` (each root with its replies
  embedded, plus `counts` and `limits`). With `--format table|csv` the
  `counts`/`limits` summary goes to one stderr line; json and markdown show it
  inline.
- **`assign`** takes exactly one of `--self`, `--ripley`, or `--user-id`
  (optionally `--agent-name NAME --credential-type oauth|api_key|platform
  [--credential-id ID]` to assign through one of that user's agents). Tip: an
  agent using its own credential can simply use `--self` — the server records
  the agent.
- `--if-state-version` is a compare-and-swap: if the thread changed since you
  read it, the command exits `3`. Re-read the thread and decide again.

### Anchoring to a quoted passage

`comment create` and `suggestion create` anchor to a passage of a note with
`--quote "<exact text>"`, written as it reads (not as markdown). The CLI reads
the note and derives a short prefix/suffix context for you; `--prefix` /
`--suffix` give it explicitly, and `--occurrence N` (1-based) picks one match.

```bash
fastio comment create NODE_ID "Is this date right?" \
  --entity-type workspace --entity-id WS_ID --quote "March 3, 2026"
```

- If prefix + quote + suffix occurs **more than once** in the note, the command
  refuses — add context or `--occurrence N`.
- If it occurs **zero** times the anchor is still sent, with a stderr warning
  (the stored note text can lag the live note).
- Limits: quote ≤ 500 characters, prefix and suffix ≤ 100 each, whole anchor
  ≤ 2048 bytes. On `comment create`, `--quote` and `--reference` are mutually
  exclusive.

### Suggested edits

A suggestion proposes replacing or deleting a quoted passage of a note; someone
with edit rights accepts or rejects it.

```bash
fastio suggestion create NODE_ID --entity-type workspace --entity-id WS_ID \
  --quote "net 30" --replacement "net 45" --note "Matches the signed terms"
fastio suggestion create NODE_ID --entity-type workspace --entity-id WS_ID --quote "draft" --delete
fastio suggestion list [NODE_ID] --entity-type workspace --entity-id WS_ID --status open
fastio suggestion reply SUGGESTION_ID "Updated" --node-id NODE_ID --entity-type workspace --entity-id WS_ID
fastio suggestion edit SUGGESTION_ID --replacement "net 60"     # your own pending suggestion
fastio suggestion withdraw SUGGESTION_ID                        # your own; cannot be undone here
fastio suggestion accept SUGGESTION_ID                          # rewrites the note text
fastio suggestion reject SUGGESTION_ID                          # the note is not changed
```

- `suggestion list` without a node lists across the whole workspace or share.
- `accept` applies the edit without the note open in an editor. It sends the
  request once; it is retried once automatically (after 10 seconds) only on a
  409 `suggestion_needs_editor` with `retryable: true`, or a 503
  `suggestion_apply_unknown` (a repeat accept never applies twice).
- `accept` exits `5` when the suggestion cannot be accepted with the note
  closed. The hint says which case it is: open the note in the editor to
  accept it (hand it to a person); or the note is busy, so try again shortly;
  or an earlier accept's outcome is unknown, so re-read the note and the
  suggestion's state before doing anything else.
- `accept` and `withdraw` are destructive.

### Tips for agents working on threads

- When you pick up an assigned thread, consider publishing an intent with
  topic `comment:<root_comment_id>` so peers can see it:

  ```bash
  fastio intents allocate --workspace WS_ID --node-id NODE_ID     # returns id and version
  fastio intents fill INTENT_ID --workspace WS_ID --version VERSION --topic "comment:COMMENT_ID"
  ```

- Reply with a short summary of what you did, and leave resolving the thread
  to a person.
- For a review request, prefer proposing a suggestion over editing the note
  directly.
- Leave accepting your own suggestions to someone else.

Over MCP these are `comment` tool actions: `resolve`, `reopen`, `assign`,
`unassign`, `suggestion-add`, `suggestion-list`, `suggestion-edit`,
`suggestion-withdraw`, `suggestion-accept`, `suggestion-reject`. `list` and
`list-all` take `status`, `for_me`, `kind`, `include_suggestions`, and
`group=threads`; `add` is an alias for `create` and takes the quote params
`exact`, `prefix`, `suffix`, `occurrence`; `reply` takes `reopen`. The
`workspace` tool's `read-note` takes `include_suggestions`.

Unlike the CLI, MCP `suggestion-accept` sends the accept ONCE and never
retries it. On a retryable `suggestion_needs_editor` or a
`suggestion_apply_unknown`, re-read the note and the suggestion's state before
accepting again (a repeat accept never applies twice).

## Exporting notes

`fastio export` converts a note to `pdf`, `docx`, `odt`, `html`, `txt`, or `md`
(`--to`; the global `--format` still controls how the command's own output is
rendered).

```bash
# Download the finished file
fastio export NODE_ID --workspace WS_ID --to pdf --comments margin --output ./note.pdf

# Save the export into the workspace instead (next to the note, or into --folder)
fastio export NODE_ID --workspace WS_ID --to docx --comments native --tracked-changes --save-copy

# Start it and return at once, then collect it later
fastio export NODE_ID --workspace WS_ID --to odt --no-wait
fastio export status EXPORT_ID --output ./note.odt
```

Options: `--comments none|margin|native|endnotes`, `--include-resolved`,
`--tracked-changes`, `--version VERSION_ID`, `--page-size letter|a4`,
`--margins narrow|normal|wide`, `--name FILE_NAME`, `--folder FOLDER_NODE_ID`
(with `--save-copy`), `--no-wait`, `--timeout SECONDS`.

- Defaults: no comments, letter page size, normal margins, resolved threads
  excluded. Page size and margins matter for the paged formats (pdf, docx,
  odt).
- Comment modes: `margin` (PDF margin column and annotations), `native`
  (DOCX/ODT native comment threads), `endnotes` (an appendix; the reliable mode
  for complete thread text), `none`.
- `--tracked-changes` exports pending suggestions as native tracked changes;
  docx and odt only.
- Exactly one of `--output PATH` or `--save-copy` is required unless
  `--no-wait`. `--output` writes atomically — an existing file is only replaced
  by a complete download. `--save-copy` prints the new node's id; with
  `--to md` the copy is a new note, not a file.
- By default the command waits, polling with backoff (1 s growing to 10 s) up
  to `--timeout` (default 600 s, at most 86400). On timeout it exits non-zero
  and prints the export id with a `fastio export status EXPORT_ID` hint.
  `export status --timeout 0` does a single check, which can still take up to
  30 seconds if the server is slow.
- A failed job exits non-zero with a hint for its reason and is never
  re-submitted automatically. If an export was too large and the server says
  dropping comments would fit, the hint suggests retrying with `--comments
  none` and without `--tracked-changes`.
- The export `report` (pages for PDF, comment threads or suggestions that could
  not be placed, missing images, dropped items, a truncated flag) is always
  shown: inline for json/markdown, as one stderr line for table/csv.
- Each run sends one idempotency key, reused on automatic network retries, so
  a retried request never starts a second export.
- The file is downloaded with your credentials directly from the API, and the
  download never follows a redirect. Exported files expire on the server after
  24 hours.

Over MCP, the `workspace` tool's `export-note` action (`workspace_id`,
`node_id`, `format`, `comments`, `include_resolved`, `suggestions none|native`,
`version_id`, `page_size`, `margins`, `file_name`, `destination
download|workspace` — default `workspace`; an `md` copy is a new note — and
`parent_id`) starts the job and, once the server has accepted it, waits up to
about 25 seconds. It returns `status`, `export_id`, `output`, and `report`, or
`queued`/`running` with the `export_id` if it is not done yet. If checking the
job fails, the error still carries the `export_id`: call `export-status` with
it rather than starting another export. `export-status` (`export_id`, optional
local `output_path`) is a single check that returns the status and report;
given `output_path` and a finished `download`-destination export, it saves the
file locally. A failed export is returned as an error result, with its record
and hint. Neither action returns credentials or a download URL.

## E-signature

`fastio sign` and the `sign` MCP tool drive **`SignEnvelopes`** — PDFs sent to
recipients for electronic signature. Every envelope is parented to a
**workspace**, so each subcommand takes a required `--workspace <id>`.

Signing is available on **every plan**. The org resource exposes
`capabilities.signing` (boolean) to confirm availability, and the server rejects
calls with a feature-disabled error if an org's plan does not grant it — a `403`
with error code `1670` ("Signing is not enabled for this organization"). Access
also requires workspace membership.

Over MCP the `sign` tool exposes reads, reversible draft-drive actions, and the
idempotent recovery actions. The outward-facing and terminal actions — `send`
(which emails real recipients) and `void` — are **CLI-binary-only**
(`fastio sign envelope send|void …`), as are `template create|update|delete`.
Envelopes are voided, never deleted.

## File Shares

`fastio fileshare` creates **durable, link-shareable views of a SINGLE workspace
file** — the replacement for the retired **QuickShare** (the legacy QuickShare
surface has been fully removed from this CLI). A File Share binds one file node
and serves it via a stable link with an optional password, an access option, an
expiry, and per-user grants (`view < download < edit`).

```bash
fastio fileshare create --workspace WS_ID --node NODE_ID --title "Q3 Report" --access-option anyone_with_link
fastio fileshare list   --workspace WS_ID
fastio fileshare info   FS_ID                       # details + effective_capability (anonymous-capable)
fastio fileshare update FS_ID --title "..." --clear-password --clear-expires
fastio fileshare grants list  FS_ID
fastio fileshare grants add   FS_ID --user USER_ID --capability download   # exactly one of --user / --email
fastio fileshare grants remove FS_ID --email someone@example.com --yes
fastio fileshare download FS_ID -o ./file.pdf [--version VID] [--password PW]
fastio fileshare versions FS_ID
fastio fileshare preview  FS_ID --type thumbnail -o ./thumb.jpg            # PRIMARY asset only
fastio fileshare upload   FS_ID ./new-version.pdf [--if-version VID] --yes  # write-back: NEW VERSION
fastio fileshare activity FS_ID                     # single activity poll (members only)
fastio fileshare ws-token FS_ID --token-file ./ws.token                    # realtime token (0600)
fastio fileshare delete   FS_ID --yes
```

- **Anonymous consumption.** `info` / `download` / `versions` / `preview` may be
  used **without auth** on a public (`anyone_with_link`) share. A protected link
  needs the password (next bullet). An **expired stored-profile** credential
  falls back to anonymous for these reads (with one stderr warning); an explicit
  `--token`/env token that fails stays a hard error; management / `upload` /
  `ws-token` / `activity` always require auth.
- **Password discipline.** A link password comes from `--password` **or** the
  `FASTIO_FILESHARE_PASSWORD` env var (the flag wins; prefer the env var so the
  value stays out of `ps` and shell history). It travels **only** in the
  `x-ve-password` header and is never logged. On `update`, pass
  `--clear-password` to remove it (don't also pass `--password`). A `1650`/`401`
  on a consumption read means the link password is missing or wrong (not an
  account-login problem).
- **Write-back (CAS).** `fileshare upload` pushes a **new version** of the bound
  file (the previous version is retained in history) and needs an `edit` grant.
  Pass `--if-version VID` for optimistic concurrency: the precondition is
  **server-enforced** — when the server detects a version conflict it reports
  `CONFLICT_VERSION_MISMATCH:{vid}` and the CLI surfaces it as a version-conflict
  error. On that conflict, re-download the current bytes, re-apply your change,
  and retry with `--if-version {vid}`. Files ≤ 4 MB go single-shot; larger files
  chunk + complete + poll.
- **`ws-token`** mints a realtime WebSocket token; it is **redacted from stdout**
  and only written (0600) to `--token-file <path>`. There is no in-CLI WebSocket
  client (token mint only).

### Over MCP

The `fileshare` MCP tool exposes **read + drive** actions: `create`, `list`,
`info`, `update`, `grants-list`, `grants-add`, `versions`, `download`, `preview`,
`activity`, `describe`. The four LINK-ACCESS reads (`info`, `download`,
`versions`, `preview`) run **anonymously** when the server holds no token — the
same anonymous-consumption path as the CLI (a `named_people` / `any_registered`
share then returns the uniform unavailable/auth error). The other actions
require auth. The destructive actions are **confirm-gated**: `delete` requires
`confirm_delete=true` and `grants-remove` requires `confirm_revoke=true`
(rejected **before auth and arg extraction**, so even an unauthenticated /
arg-less probe gets the gate message, mirroring the CLI `--yes`). The
`password` arg must be a **string**; a non-string value (or `password` together
with `clear_password=true` on `update`) is rejected explicitly. Two actions are
**CLI-binary-only** and NOT routable over MCP:

- **`upload`** (write-back) — it needs the local file bytes and is destructive;
  run `fastio fileshare upload …`.
- **`ws-token`** (realtime mint) — the token is a long-lived secret that must not
  be parked in an MCP transcript (the CLI redacts it and writes 0600); run
  `fastio fileshare ws-token … --token-file <path>`.

`download` / `preview` write bytes to the agent's local filesystem (default under
`.fastio/downloads/`, created 0700) and return a path + byte count. The
`password` arg authorizes a protected link (x-ve-password; never echoed).

## Service accounts

A service account is a non-human member of ONE workspace that signs in only with
API keys — for CI jobs, scripts, and agents that should not borrow a person's
account.

Roles: `viewer` (read, download, search, AI chat) or `editor` (viewer plus upload, edit, move, rename, trash, metadata, comments).

**Managing them is CLI-only and needs a person's credential** — a workspace
admin or owner, signed in as themselves. There is no MCP tool for it, and a
service account key cannot manage service accounts or mint keys.

```bash
fastio service-account list --workspace WS_ID            # alias: fastio sa …
fastio sa create --workspace WS_ID --name ci-bot --role editor [--key-expires 1w|30d|90d|1y|never] [--key-memo TEXT]
fastio sa info SA_ID --workspace WS_ID                   # the account and its keys
fastio sa update SA_ID --workspace WS_ID [--name NEW] [--role viewer|editor]
fastio sa disable SA_ID --workspace WS_ID                # every key stops at once; reversible
fastio sa enable SA_ID --workspace WS_ID
fastio sa delete SA_ID --workspace WS_ID --yes           # deletes its keys and closes it
fastio sa key list SA_ID --workspace WS_ID
fastio sa key create SA_ID --workspace WS_ID [--expires 1w|30d|90d|1y|never] [--memo TEXT]
fastio sa key revoke SA_ID KEY_ID --workspace WS_ID --yes
```

- `create` (which also issues the first key) and `key create` print the key's
  **secret once**; it can never be retrieved again. Both refuse `--quiet` and
  `--fields`, which would discard it. Key expiry defaults to 90 days.
- If a create fails with a network or 5xx error, do NOT just re-run it: the
  account or key may already exist. Check with `list` / `key list`, revoke any
  key whose secret you do not hold, and create a new one.
- A role change applies to every existing key of the account. `disable` is the
  reversible stop; `delete` and `key revoke` are permanent.
- An account's activity: `fastio event list --workspace WS_ID --calling-user-id SA_ID`.

**Running the CLI as a service account** — use the secret like any API key:

```bash
export FASTIO_API_KEY=fio_sa_...      # or --token, or a profile
fastio auth status                    # shows account_type: service_account, its name, workspace, role
```

- The key is always limited to its one workspace (`workspace:<id>:r` for a
  viewer, `:rw` for an editor) and cannot be widened with `auth api-key update`.
- It works over plain REST only (no realtime/websocket tokens); poll for changes.
  The built-in `fastio mcp` server works with it for everything its role
  allows.
- Management and sharing operations (members, invitations, creating shares, File Shares, sign, imports, org changes, permanently purging files or emptying the trash, key/account management) return HTTP `403` with reason `service_account_not_permitted`. Identity reads (`auth status`, `auth check`, `workspace list`) work, and public share links behave as if the request were anonymous.
  Repeating a refused call with the same key never helps — it needs a person's
  credential.
- **Never paste a service account secret into an MCP tool argument** or any
  other transcript; set it in the environment of the process that needs it.

## Billing

Org billing lives under `fastio org billing`:

```bash
fastio org billing plans
fastio org billing subscribe ORG_ID --plan PLAN_ID
fastio org billing reactivate ORG_ID         # re-enable a scheduled cancel
fastio org billing cancel ORG_ID --yes       # schedule cancel at period end
fastio org billing usage ORG_ID              # credit usage
fastio org billing meters ORG_ID --meter METER [--workspace-id ID | --share-id ID]
fastio org billing invoices ORG_ID [--starting-after CURSOR]
fastio org billing details ORG_ID
fastio org billing members ORG_ID
```

A `402` / billing error surfaces an actionable hint pointing at
`fastio org billing plans` / `subscribe`.

## ID Formats

- **Organization / Workspace / Share IDs**: 19-digit numeric strings
  (e.g. `1234567890123456789`)
- **Node IDs** (files/folders): opaque alphanumeric with hyphens
  (e.g. `2abcd-efghj-kmnpq-rstuv-wxyz3-4567`)
- **Root folder**: the literal string `root`
- **Trash**: the literal string `trash`

## Pagination

Storage endpoints (files) use cursor-based pagination:

```bash
fastio files list --workspace WS_ID --page-size 100 --cursor NEXT_CURSOR
```

Other endpoints use offset-based pagination (and a few — e.g. invoices — use
cursor / `--starting-after`):

```bash
fastio org members list ORG_ID --limit 50 --offset 0
```

## Error Handling

- Exit code `0` = success
- Exit code `1` = error (check stderr)
- Exit code `2` = invalid arguments (clap parsing error)
- Exit code `3` = a compare-and-swap conflict (the version or thread state you
  sent is no longer current) — re-read, then decide again
- Exit code `5` = the suggestion could not be accepted with the note closed —
  open the note to accept it, or follow the hint (try again shortly when the
  note is busy; re-read first when an earlier attempt's outcome is unknown)

Common errors:

- `authentication required` → set `--token` or run `fastio auth login`
- `workspace ID must not be empty` → missing required ID
- `invalid page size` → must be 100, 250, or 500
- a `402` billing error → run `fastio org billing plans` / `subscribe`

## MCP Server

The CLI includes a built-in MCP server for direct agent integration:

```bash
fastio mcp
```

It speaks MCP over stdio and exposes the CLI's operations as action-routed tools
(`ripley`, `fileshare`, `files`, `org`, `workspace`, `sign`, …). The `files`
tool mirrors the three ways to reach a file's words: `read` for raw bytes,
`content` for one file's extracted text as addressed chunks, and `content-many`
to score up to 10 files against one question in a single call. Tool results are
rendered as GitHub-flavored Markdown for compact, high-signal consumption. This
same guide is available as the `skill://guide` MCP resource and via
`fastio skill`.

Auth and backend follow the standard CLI precedence: the server honors the
global `--token` / `--profile` / `--api-base` flags (and `FASTIO_TOKEN` /
`FASTIO_API_KEY` env), so `fastio --profile staging mcp` or
`fastio --api-base <url> mcp` point it at a non-default backend.
