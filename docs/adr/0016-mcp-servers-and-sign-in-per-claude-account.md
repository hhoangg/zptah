# 16. MCP servers and sign-in are managed per Claude account

Date: 2026-11-27
Status: Accepted

## Context

Settings > MCP ran `claude mcp list` and `claude mcp login` with no account environment, so it
always read the CLI login (`~/.claude.json`, `~/.claude`). Sessions run Claude with
`CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` set to
`<data>/provider-accounts/claude/<account-id>/` for every non-default account. Everything the CLI
keeps for an MCP server is per config dir:

- Servers: user servers (`mcpServers`) and local servers (`projects[<path>].mcpServers`) live in
  `<config dir>/.claude.json`. Local scope is per project path.
- OAuth tokens: the secure storage blob (`mcpOAuth`, `mcpOAuthClientConfig`, next to the account's
  own login). macOS keychain service `Claude Code-credentials` (default) or
  `Claude Code-credentials-<first 8 hex of sha256(config dir)>`; on Linux
  `<config dir>/.credentials.json`.
- Project `.mcp.json` approval: `enabledMcpjsonServers` in `<project>/.claude/settings.local.json`
  is ignored unless the folder is trusted for that config dir
  (`projects[<path>].hasTrustDialogAccepted` in `<config dir>/.claude.json`). The home folder is
  never persistently trusted. No CLI command approves a server.
- Header auth: a server with an `Authorization` header (or `headersHelper`, `bearerTokenEnvVar`)
  has no login; `claude mcp login` exits 1 with "authenticates with the `Authorization` header".

So a sign-in under the default config never reached a session on another account: that session
still said "Pending approval" or "needs authentication". Servers added to the CLI login were absent
from every account. Every http server showed "Connected" and "Sign in" together because the button
only looked at the transport.

## Decision

- The MCP page works on one Claude account at a time: the account new sessions in the project use
  (`Book::selected`), with a selector when there is more than one account. Health (`claude mcp
  list`), sign-in (`claude mcp login`), remove and add run with that account's environment. The
  default account clears `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR`, because the app
  may itself have been started from a Claude session. Health results are keyed by project and
  account.
- "Sign in" appears only when the server needs it (status "Needs authentication"). A connected OAuth
  server shows "Signed in" and "Sign in again". Servers that use a key in their config and local
  servers never offer sign-in. "Sign in for all accounts" reads each account's own status first and
  signs in one account at a time, only where it is needed.
- A project server "Pending approval" gets an explicit Approve action per account, run on click only,
  after a confirmation that says it also trusts the folder for that account. It adds the server to
  `enabledMcpjsonServers` and sets `hasTrustDialogAccepted` in the account's config, keeping every
  other key. It refuses the home folder. A rejected server can be approved too (it is removed from
  `disabledMcpjsonServers`).
- Approve writes for the canonical git root of the project (the main working tree when the project
  is a git worktree), because Claude resolves local settings and the trust lookup from that root.
  Running Claude sessions of that account must be restarted to pick the approval up; the confirm
  says so.
- Writing a Claude config is careful: symlinks are resolved first (a linked config stays a link),
  an existing file keeps its permissions (a new one is 0600), the advisory lock lives in the temp
  dir and never next to the user's files, the only file created beside the target is the temp file
  of the atomic rename (removed on every failure), and if the file changed between the read and the
  rename (a running Claude writes it often) the edit is applied again to the new content, a few
  times, then fails with a plain error. serde_json rewrites the file with its keys sorted; the
  content is otherwise unchanged.
- No automatic sync of user-level servers or auth into account dirs. Accounts are isolated on
  purpose (another organisation or billing), Claude stores OAuth tokens per config dir by design and
  the app never reads or copies a token, and a silent copy of a header-auth config would duplicate a
  secret and leave it stale when it is rotated. Servers that exist in the CLI login (user scope, or
  local scope for this project) but not in the chosen account are listed as "Only in CLI login" with
  an explicit "Add to <account>" action. It runs `claude mcp add-json <name> - --scope <scope>` as
  that account, with the server JSON on stdin, never in the arguments or a log. A server with a key
  asks first. The user then signs in for that account if the server uses OAuth.
- Known CLI failures are shown in plain words ("signs in with the Authorization header in its
  config", "<account> does not have <name> configured for this project"); the cleaned output stays
  behind "Show details".

## Consequences

- Signing in for the default account still does not sign in the other accounts; that is the CLI's
  design, and "Sign in for all accounts" saves the repeated clicks.
- Approving trusts the folder for that account, the same as accepting Claude's trust prompt.
- Servers are added to an account once; later changes to the CLI login are not mirrored.
