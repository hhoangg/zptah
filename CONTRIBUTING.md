# Contributing

MonoCode is early and I’m the only maintainer, so small and focused lands much faster than large and ambitious. Past that, the door is open - bug reports and fixes are genuinely welcome.

Please don’t open PRs that add a new provider right now. The existing harnesses still need to agree on a few patterns, and a new adapter would copy whatever is there today. See [New providers](#new-providers).

## PR policy during the GPUI preview

The Rust + GPUI app is developed on `main`. The React/Tauri app was removed from this repository.

- Bug fixes, security fixes and documentation improvements are welcome on `main`.
- Target GPUI contributions at `main`. See [the native guide](docs/native/README.md) for setup and checks.

Before starting a larger change, open an issue so we can agree on the scope and target branch and avoid duplicated work. Existing PRs will be reviewed individually.

## Get it running

You need Node.js 20+, a current stable Rust toolchain, and at least one provider CLI installed and logged in. See [provider setup](docs/providers.md) for installation and login instructions.

macOS, Linux, and Windows are supported targets. See [building from source](docs/building.md) for platform prerequisites and packaging commands.

```bash
npm install
npm run native:dev
```

## Where things live

- `crates/monocode-core/` - the app logic shared by the native app: providers, sessions, storage, git, remote
- `crates/monocode-gpui/` - the GPUI app
- `crates/mobile-link/` - the pairing and link protocol (used by `crates/monocode-core` and the web link protocol)
- `host/` - the remote Host (TypeScript), packaged with `npm run host:package`
- `src/` - pruned to what `host/`, the `scripts/native-editor` and `scripts/native-settings` generators and the `crates/monocode-core` provider/kind.rs tests use, plus code kept alongside it (harness sources are in `src/integrations/harness/`). It is not an app any more and not yet the exact import closure; a follow-up should prune it and decouple `host/`.
- `web/` - the web app, Worker and relay protocol
- `assets/native/` - icons, entitlements and the updater public key used to package the native app

`src/integrations/harness/` holds the provider protocol modules (for example `claudeProtocol.ts`). They are pure functions with unit tests beside them (`npm run test:harness`). Please don’t add a new provider yet.

## Before you push

```bash
npm run native:check
```

That runs the node script tests, `cargo fmt`, `cargo clippy` and the native test suites; CI runs most of them (see [the native guide](docs/native/README.md)). If you touched `host/` or `src/`, also run `npm run host:build`, `npm run test:host` and `npm run test:harness`.

## New providers

I’m pausing new harnesses until the current ones share the same patterns - session lifecycle, catalog probes, usage, approvals, and how slash commands and skills are wired. A PR that adds another provider will be closed for now, even if the work is good. Fixes, tests, and protocol bugs on Claude, Codex, Cursor, Grok, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent, and Devin are still the best kind of contribution.

When the pause lifts, this section goes away.

## Pull requests

Keep a PR to one thing, and say what changed and why. The [PR template](.github/pull_request_template.md) covers the rest. Branch names, commit messages and PR titles follow [the git conventions](docs/git-conventions.md). If it changes the UI, a before/after screenshot helps a lot.

For anything that moves product direction - a new surface, new provider behavior, a refactor that changes the shape of the app - open an issue first. That’s not gatekeeping, I’d just rather you hear “I’m already halfway through that” before you write it than after. New providers are the exception: don’t send the adapter, even from an issue, until the pause above is gone.

I might close a PR, ask you to shrink it, or end up implementing the idea differently. That’s a call about scope and timing, not about you or the quality of your work.

Be kind: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Security reports: [SECURITY.md](SECURITY.md).
