<h1 align="center">zptah</h1>

<p align="center">
  <strong>A desktop app for running and managing your AI coding agent sessions.</strong>
</p>

zptah is a native desktop app (Rust + GPUI) for macOS, Linux and Windows. It does not sell tokens: it drives the coding agents you already have installed and signed in to, such as Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent and Devin. Tabs are sessions, and the composer is the input.

## Features

- Run several agent sessions as tabs, and split panes to work with sessions side by side.
- Keep conversations organized by project.
- Use separate Git worktrees for parallel tasks.
- Edit files and review changes in the app.
- Start a message with `/operator` to let an agent open sessions and manage worktrees and notes. See [agent access](docs/agent-access.md).
- Monos: experimental agents that work across your projects, keep a memory and run scheduled tasks you set up with them.
- Remote sessions (experimental) and answering questions from Telegram. See [remote access](docs/remote-access.md) and [Telegram](docs/telegram.md).

Install and sign in to at least one supported agent first. See [provider setup](docs/providers.md).

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

## Web companion

The browser client at [zptah.app](https://zptah.app) lets you reach your sessions from a browser, including on a phone. See [web/README.md](web/README.md) for how it is built and run.

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
