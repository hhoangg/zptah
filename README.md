<h1 align="center">zptah</h1>

<p align="center">
  <strong>A desktop app for running and managing your AI coding agent sessions.</strong>
</p>

zptah is a native desktop app (Rust + GPUI) for macOS, Linux and Windows. It does not sell tokens: it drives the coding agents you already have installed and signed in to, such as Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent and Devin. Tabs are sessions, and the composer is the input.

## Features

Install and sign in to at least one supported agent first. See [provider setup](docs/providers.md).

### Sessions and providers

- Run many sessions as tabs, split panes side by side, group sessions in folders and pin tabs.
- Supported agents: Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent and Devin. Missing CLIs are detected at startup and disabled with an install hint.
- Several accounts per provider (Claude, Codex, Devin), usage and remaining-limit footer, and a resource pressure view to stop heavy agents.
- Per provider, open sessions as an app chat or in the provider's own terminal UI. See [terminal sessions](docs/tui-sessions.md).
- Attach files and images, queue follow-ups, answer agent questions, rewind turns, and use slash commands and skills.
- MCP servers and skills managed in Settings.
- Start a message with `/operator` to let an agent open sessions, split panes and manage worktrees and notes. See [agent access](docs/agent-access.md).
- Orchestration: a lead agent plans work and dispatches worker sessions.

### Projects, worktrees and code

- Projects in a rail with quick open, project search and per-project settings and themes.
- Git worktrees for parallel tasks, a worktree switcher and worktree management in Settings.
- Built-in file editor with syntax highlighting, file explorer, Markdown preview and Mermaid diagrams ([Mermaid](docs/native/mermaid.md)).
- Diff view, per-turn change recordings and session review, git status, git graph and branch picker, and git credential management.
- Embedded terminal.
- Contributors page per project: stats, leaderboard and identity merging.

### Inbox

- GitHub, GitLab, Linear, Jira, ClickUp and Azure DevOps sources, connected in Settings > Inbox. See [Jira](docs/jira.md) and [ClickUp](docs/clickup.md).
- Browse issues and tasks with comments, ask an agent about a ticket, or start work on it in a local project.
- Workflows that run on a schedule, by hand or when an inbox item appears (a new PR, issue or ticket).
- Ticket Contributors with AI ticket scoring for ClickUp.

### Monos

- Agents that work across your projects with a memory, a personality, a chosen provider and their own sessions.
- Habits: scheduled tasks (hourly, daily, weekdays, weekly, every N hours), shareable as YAML.
- Workflows: graphs of session, shell, ask-a-Mono, condition, for-each and loop steps, drawn as live run graphs, exported as YAML, and an imported file is reviewed before it runs.
- Browser: zptah Browser, a Chrome extension that lets a Mono work in your real Chrome profile. Several Monos can share one profile, each in its own tab group. Risky actions (payments, sending, deleting, credentials) ask for your approval, in English and Vietnamese, and a Stop button on each tab takes it back at once. See [zptah Browser](docs/browser/zptah-browser.md).
- Dashboard, overview, notifications, Today view and notes.

### Visuals

- Agents publish interactive charts, diagrams and calculators into the conversation, rendered natively. Use `/visualize`. See [session visuals](docs/native/visuals.md).

### Cloud sessions

- Run a session in a GitHub Codespace or a Docker Sandbox and move it between your computer and the cloud with Continue in cloud and Continue locally.
- Setup kits for the cloud workspace, auto-stop, and a footer chip and drawer to stop or delete workspaces. See [cloud workspaces](docs/native/cloud-workspaces.md) and [cloud dev environment](docs/native/cloud-dev-environment.md).

### Remote machines

- Run sessions on another Windows, Linux or macOS machine over SSH with `zptah-host`, installed and paired from Settings > Connections. Host sessions keep running when you close the desktop. See [remote access](docs/remote-access.md).
- Pair the zptah web app (zptah.app) in Settings > Connections.

### App

- Auto-update with signed updates, notifications and sounds, themes and keybindings, a quick composer, and Settings > Archive.

## Install

### macOS (Apple Silicon)

```bash
brew install --cask hhoangg/zptah/zptah
```

Or download the `.zip` from [GitHub Releases](https://github.com/hhoangg/zptah/releases).

### Linux (x86_64)

Download the AppImage, `.deb` or `.rpm` from [GitHub Releases](https://github.com/hhoangg/zptah/releases). See [Linux installation](docs/install.md) for dependencies and package commands.

### Windows (x86_64)

Download and run the installer (`.exe`) from [GitHub Releases](https://github.com/hhoangg/zptah/releases).

## Coming from Monoptah?

Open Settings > Import from Monoptah to copy your conversations, settings, saved sign-ins, Monos and files into zptah. Monoptah keeps its own copy and is not changed. Quit Monoptah first.

## Web companion

The browser client at [zptah.app](https://zptah.app) lets you reach your sessions from a browser, including on a phone, over an end-to-end encrypted channel after pairing. Remote push in Git, cloud start and tool approvals are separate opt-in permissions. See [web/README.md](web/README.md) for how it is built and run.

## Build from source

You need Node.js 22+ and a stable Rust toolchain (plus Xcode on macOS). See [build setup](docs/building.md) and the [native guide](docs/native/README.md) for prerequisites, build output and checks.

```bash
npm run native:dev     # launch with a separate development database
npm run native:build   # create an optimized build
npm run native:check   # full check suite
```

The native commands do not need `npm install`. Type `r` then Enter in the development terminal to rebuild and restart; there is no hot reload. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Credits

zptah originated from [hardbeat920/monocode](https://github.com/hardbeat920/monocode) via the Monoptah fork. Thanks to the upstream author and contributors.

## License

[MIT](LICENSE). Provider names and logos are trademarks of their owners. See [NOTICE](NOTICE).
