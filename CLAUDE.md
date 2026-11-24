# zptah

This repository is **zptah**, a native desktop app (Rust + GPUI) plus a remote Host and a web
companion. It originated from [`hardbeat920/monocode`](https://github.com/hardbeat920/monocode)
via the Monoptah fork. It tracks upstream and merges from it regularly, while carrying
fork-local changes that are never offered back upstream.

## Git conventions

Full rules and examples: [`docs/git-conventions.md`](docs/git-conventions.md).

- Types (branch, commit, PR): `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.
- Branch: `<type>/<issue#>-<short-slug>` or `<type>/<short-slug>`, lowercase, about 40 chars. Rename an app-made `mc/...` branch with `git branch -m` before the first push.
- Commit: `<type>(<scope>): <summary>` (imperative, at most 72 chars, no period), English. A body (why and what) is required for `feat` and `fix`; add `Refs: #N` or `Fixes: #N` when there is an issue. One buildable change per commit.
- PR titles become native release-note lines (`feat`, `fix`, `perf` are listed; label a PR `user-visible` to include `docs`, `chore`, `ci`, `test`, `build`, `refactor`), so write them for users.
- PR: title in the commit format; the body fills every section of `.github/pull_request_template.md`, keeping its headings. Merge with squash, except upstream-sync PRs, which use a merge commit.
- Never commit to `main`, never force-push a branch with an open PR, never commit secrets or realistic-looking fake tokens.

## After every merge from upstream

Upstream keeps adding links, menu items, dialog copy and docs that say "MonoCode" and
point at `hardbeat920/monocode`. Every merge can therefore quietly reintroduce the
upstream identity into a build called zptah. **Re-check after each merge** — do not
rely on remembering which files were fixed last time, because the next merge will touch
different ones.

Run this and resolve every hit before the merge is considered done:

```bash
./scripts/check-fork-branding.sh
```

It exits non-zero while anything remains, so it can gate a merge. It looks only at
string literals and links, so an identifier like `MonoCodeMark` does not trip it, and
it carries an ALLOW list of deliberate keeps with the reason for each. When you decide
a new hit should stay, add it to that list **with its reason** — never loosen the
pattern, because a checker that passes by looking away is worse than no checker.

For each hit, decide:

- **A link to upstream** → point it at this fork (`hhoangg/zptah`), except the
  Website menu item, which goes to `https://zptah.app`.
- **Text a user can read** (dialog titles, tray tooltip, window title, button labels,
  "What's new in …", error copy) → say zptah.
- **A test asserting old copy** → update the assertion. A test that still passes only
  because it asserts "MonoCode" is a failure, not coverage.

The Tauri app (`src-tauri/`, most of `src/`, `mobile/`, root `index.html`, `vite.config.ts`
and the Tauri workflows) was removed from this fork. When a merge reports a modify/delete
conflict on one of those paths, keep the deletion. `src/` was pruned to what `host/`, the
generators in `scripts/native-editor` and `scripts/native-settings`, and the
`crates/monocode-core` provider/kind.rs tests use, plus code kept alongside it. It is not
the exact import closure; a follow-up should prune it to that closure and decouple `host/`. Native assets that used to live in `src-tauri/` now live in
`assets/native/` (icons, `Entitlements.plist`, `Assets.car`, `updater.pub`); confirm that
`assets/native/updater.pub` still holds the fork's updater public key after a merge.

If upstream changed `host/`, bump `host/VERSION` and cut a Host release (see "Releasing the
Host"). Give any new upstream Host workflow job the
`if: github.repository == 'hardbeat920/monocode'` gate.

## Never rename these

They are identifiers, not branding. Changing them breaks working installs:

| Identifier | Why it stays |
|---|---|
| `monocode.*` localStorage key prefix | Renaming loses every saved preference on existing installs |
| npm package `monocode-desktop` (root `package.json` name) | `scripts/check-web-isolation.mjs` flags any import of that package name from `web/` |
| `zptah-host` CLI name, its data dir `~/.zptah-host`, `app.zptah.host` launchd label, `zptah-host.service`, the `zptah Host-<SID>` Windows task, default port 3784 | Installed remote Hosts, the release asset names (`zptah-host-<target>`) and the app's bootstrap depend on them. They are distinct from Monoptah's `monocode-host`, `~/.monocode-host`, `com.monocode.host` and port 3774 on purpose: both Hosts run side by side on one machine and share nothing. Never stop, upgrade, uninstall or delete `monocode-host` or `~/.monocode-host` from this app: it may belong to the installed Monoptah |
| `INTERRUPT_MESSAGE` in `src/features/sessions/model/inFlight.ts` | Compared with `===` against text already stored in saved sessions; changing it makes old interrupt notices unrecognised and duplicated |

## Verifying

For the remote Host: `npm run host:build` (typecheck and bundle) and `npm run test:host`. For the
harness sources the Host imports: `npm run test:harness`. The native app is verified with
`npm run native:check` (see "Developing the native app").

Read the exit status directly - never through a pipe. `npm run native:check | tail` reports
`tail`'s status, which is always 0, so a failing suite looks green. Use
`npm run native:check > log 2>&1; echo $?` or `${PIPESTATUS[0]}`.

## Seeing a change live

For the native app, run Native Dev from a clean worktree that matches origin, so what you
see is what was pushed:

```bash
npm run native:dev -- --detach
```

`--detach` keeps the app alive when the task that started it ends (a background task
would otherwise take it down) and writes its stdout and stderr to a log file whose path it
prints. Native Dev keeps its data in `~/Library/Application Support/zptah Development`
(`DEV_DATA_DIR` in `crates/monocode-core/src/brand.rs`), separate from the installed app.

## Developing the native app

These rules are team-wide and come from measurement: one session spent about 468 minutes
on about 240 cargo test runs because it re-ran tests after every small edit. Each run
relinks the large `monocode-gpui` test binary (median 1.4 min, p90 4.1 min).

- **Test in one pass per task.** Make all the edits for a task first. Then run `cargo fmt`,
  clippy and only the related tests, filtered to the touched modules, once, in a single
  pass. Fix and rerun only what failed. Do not run tests after each small edit. Run the full
  `npm run native:check` once per batch (before a release or a merge to main), not per task:
  CI runs the GPUI and core suites on Linux for every PR (see "What CI checks").
- **Seed the build cache in a new worktree.** Before the first cargo command, clone an
  existing target dir from the main checkout (or another worktree that has one) so crates
  are not rebuilt from scratch. The native workspace (`monocode-gpui` and `monocode-core`)
  builds into `crates/monocode-gpui/target`; the root workspace (only
  `crates/mobile-link`) builds into `target`. On macOS `cp -c -R` makes an APFS clone,
  which is instant and takes no extra space:

  ```bash
  cp -c -R <checkout>/crates/monocode-gpui/target crates/monocode-gpui/target
  ```

  Elsewhere use plain `cp -R` (a real copy).
- **Always build the native workspace with the `native-dev` profile.** Plain `cargo test`,
  `build`, `check` or `clippy` uses the `dev` profile and fills a second `target/debug` (several
  GB) that nothing else reuses. Cargo has no setting for a default profile and ignores an alias
  that shadows a built-in command, so use the aliases in `crates/monocode-gpui/.cargo/config.toml`
  from inside `crates/monocode-gpui`: `cargo ntest`, `cargo nbuild`, `cargo ncheck`,
  `cargo nclippy`, `cargo nrun`. From the repo root pass
  `--manifest-path crates/monocode-gpui/Cargo.toml --profile native-dev`, as the npm scripts do.
  In `native-dev` the `monocode-gpui` crate is `opt-level = 0` and dependencies are `opt-level = 1`.
  The `monocode-gpui` and `monocode-core` crates keep `debug = "line-tables-only"`, so panic
  backtraces show file and line. Do not add `split-debuginfo` to the profiles: `unpacked` is
  rejected on windows-msvc and `packed` runs dsymutil on every macOS link (about +9s).
  `npm run worktrees:clean -- --prune-target` removes a leftover `target/debug` and stale
  incremental data (see the `cleanup-worktree` skill).
- **Keep tests Linux-portable.** GPUI tests run on Ubuntu in CI. Use `secondary-...`
  keystrokes (cmd on macOS, ctrl elsewhere), never `cmd-...`. Do not assume macOS paths,
  Keychain, WKWebView, AVPlayer or Metal. A test that truly runs only on macOS gets
  `#[cfg_attr(not(target_os = "macos"), ignore = "<specific reason>")]` or
  `#[cfg(target_os = "macos")]`. Never weaken an assertion to make a test pass.
- **Fix at the root, in the shared component.** Prefer the shared pieces (the modal dialog in
  `crates/monocode-gpui/src/modal.rs`, the popover menus and choice selects in
  `crates/monocode-gpui/src/popover.rs`, the one `sbx` runner in
  `crates/monocode-core/src/sandboxes.rs`) so a fix reaches every caller. The first fix for the dialog
  input barrier went into individual dialogs and covered 2 of about 35.
- **Review once per request.** Agents do one review per user request, not one per step. The
  implementer hands over a checklist that maps each point of the request to its change
  (`file:line`) and its test. The reviewer gets the original request, marks each point done
  or missing with evidence, then reviews the diff. Reviewers read code; they build or run
  tests only to prove a specific finding. High-risk changes (tokens and secrets, deleting
  data, migrations, billable cloud lifecycle) get their own review.
- **No realistic secrets in tests.** GitHub push protection blocks realistic-looking tokens
  such as `dckr_pat_...` in test sources. Build fake tokens with `concat!` or use obviously
  fake values.

## Cleaning up worktrees

When a session created a PR and that PR is merged, the agent cleans up after itself: its
worktree, local and remote branch, and any temp worktrees, using the `cleanup-worktree` skill
(`npm run worktrees:clean`). It refuses anything unmerged, dirty, unpushed, running, or the
worktree Native Dev runs from, and never forces. It never removes `release-main` (the
build-cache source and release checkout), not even when named explicitly, and `--merged` keeps
detached-HEAD worktrees (name one explicitly to remove it). Do not delete worktrees by hand.

For the user: `npm run worktrees:clean -- --merged --dry-run` lists what would go and why the
rest stays, then run it without `--dry-run` to delete (it asks first; non-TTY needs `--yes`).

## Releasing

Releases are native only, see "Releasing the native app". The Tauri app and its release
workflow were removed. The updater signing key (minisign, used by `tauri signer sign` from the
`@tauri-apps/cli` devDependency) lives at `~/.config/zptah/updater.key`, with its password in
`~/.config/zptah/updater.key.password`, and is **not** in this repo; its public half
(minisign key id `437A3F6DE4A767DD`) is `assets/native/updater.pub`, the only copy: the workflow and the local
release script both read that file. The same key and password are the repository secrets
`UPDATER_SIGNING_KEY` and `UPDATER_SIGNING_KEY_PASSWORD` on `hhoangg/zptah`; only the `publish`
job maps them to `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, which is
what the signer reads. The feed is
`https://github.com/hhoangg/zptah/releases/download/native-latest/latest.json`. Losing the private key means no further
updates can be published to installed apps, because they only trust that one key, so a
replacement key would strand them. `@tauri-apps/cli` stays in `package.json` only as that signer: the native feed signing and the
signing tests in `native:test:dev` need it, so keep it until the signer is replaced.
(`@tauri-apps/api` and `plugin-dialog` stay too, only because the Host imports the harness
sources that use them.)

### Releasing the Host

The fork publishes its own remote Host (`host/`), separately from both apps. Its version lives
in `host/VERSION` as `X.Y.Z+zptah` (semver build metadata marks the fork flavor); the root
`package.json` version is not used. `zptah-host --version` prints it and
`environment.describe` reports `host: {flavor: "zptah", version: "X.Y.Z"}`.

1. `npm run host:release -- X.Y.Z` (plain `X.Y.Z`): needs a clean tree, writes `host/VERSION`,
   commits "Release Monoptah Host X.Y.Z" and creates the tag `host-vX.Y.Z`. It never pushes; it
   prints the push commands for you to review and run. To keep the git conventions (no commits
   to main, squash merges), set `host/VERSION` to `X.Y.Z+zptah` in a PR titled
   `chore(release): Monoptah Host X.Y.Z` instead, then run the script on the up-to-date main:
   with the version already recorded it only tags HEAD. Push only the tag, not HEAD.
2. Pushing the tag starts `.github/workflows/host-release-monoptah.yml`. It checks the tag
   against `host/VERSION`, refuses when the release already exists, builds every target of
   `npm run host:package -- --all` (Linux and macOS `.tar.gz`, Windows `.zip`, asset names are
   `zptah-host-<target>`), and publishes release `host-vX.Y.Z` with `--latest=false` (never "latest":
   the installed Tauri app's updater reads `releases/latest`). It uploads each package with its `.sha256` and
   `host-release.json` (`{version, assets: {target: sha256}}`, also a workflow artifact with a
   build-provenance attestation).
3. Never re-upload a version: the app pins the sha256 of every asset. To redo a failed release,
   bump the version.
4. The app pins the Host version, URL base and hashes (H2a, `host_release.rs`, written by
   `scripts/pin-host-release.mjs` from the run's `host-release.json`). Release the Host before
   the app that pins it. The pin also carries `HOST_PACKAGE_NAME` (the asset prefix). The
   1.2.1 pin still says `monocode-host`; do not ship an app with that pin to users, because that
   package installs under the old names. Re-pin to the first `zptah-host` release (1.4.0) first.

### Releasing the native app

zptah (GPUI) ships for macOS arm64 (signed with a stable self-signed certificate, not notarized), Linux x86_64 and
Windows x86_64 (unsigned installer). It has its own release flow. No macOS job
runs on GitHub Actions (hosted macOS runners are not being acquired): the macOS app is checked,
built and packaged on the maintainer's Mac, everything else is on CI.

What CI checks (`native.yml`): `monocode-core` clippy and tests plus `native:test:dev` on Linux
and Windows (the Windows core tests run in 3 shards from one nextest archive), GPUI `fmt` and clippy `-D warnings` on Linux, GPUI tests on Linux (built once with
`cargo nextest archive`, run in 3 shards, one process per test), and GPUI `cargo check
--all-targets` on Windows. The workflow runs on every PR (a `changes` job skips the heavy jobs when
no native path changed) and the `CI passed` job is the single check to require for merging. During development the local remainder is `npm run native:check:mac`
(clippy of the macOS-only code that Linux and Windows never compile); when you change macOS-only
code or tests gated to macOS, also run the tests you touched on macOS. `npm run native:check`
(the full gate, including the GPUI tests on macOS) still runs locally in `native:release:mac`
before every release. How often to run which check while developing is in "Developing the
native app".

The macOS app is signed with a stable self-signed certificate so the designated requirement
(and macOS permission grants such as Full Disk Access) survive updates. One-time setup, backup
and recovery: "Stable signing identity" in [`docs/native/release-flow.md`](docs/native/release-flow.md).
The release needs `MONOPTAH_NATIVE_SIGNING_IDENTITY` (the certificate SHA-1) and fails early without it.

How to release (timeline, notes rules, recovery and stuck runs: [`docs/native/release-flow.md`](docs/native/release-flow.md)):

0. **First zptah release only.** Run from a clone whose `origin` is `hhoangg/zptah` (the script
   refuses any other origin), with no Monoptah `native-v*` or `host-v*` tags present locally. With
   no earlier `native-v*` tag pass `--since <first commit>` and optionally `--notes FILE`. See
   "The first zptah release" in `docs/native/release-flow.md`.
1. **No notes PR.** Release notes are generated from the titles of the PRs merged since the
   previous `native-v*` tag (see `docs/git-conventions.md`, "Releases"). Optional highlights:
   `--notes FILE`, or `docs/native/monoptah-release-notes.md` with first line
   `# zptah X.Y.Z` for this version. Pick X.Y.Z by semver: `feat` is minor, `fix` and
   `perf` are patch, a `BREAKING CHANGE` is major.
2. **One command**, from an up-to-date `main` on the Mac, right after the last PR merges:
   `npm run native:release:mac -- X.Y.Z` (plain `X.Y.Z`; options `--since REF`, `--notes FILE`,
   `--recheck`, `--ci-timeout MINUTES`). Preconditions and the notes are checked first.
3. **In parallel.** The script dispatches `native-release-monoptah.yml` for the commit (the
   early run), so Linux, Windows and rpm build while the Mac runs `npm run native:check` (about
   20 minutes, remembered per commit and `rustc -V` in
   `crates/monocode-gpui/target/.native-check-stamp.json`), builds and packages. It waits for a
   green `native.yml` on main for HEAD only right before tagging (default 60 minutes). Any
   failure before the tag leaves nothing tagged or published and cancels the early run: fix it
   and run the script again.
4. **Publishing.** The script tags, pushes `native-vX.Y.Z` and creates the release with the three
   macOS files and the notes as body. The early run (or a fresh one) then signs every updater
   package in one `publish` job on Ubuntu (the signing key exists on CI only, the local script
   never reads it), verifies, publishes, and bumps the cask.
5. **`--dry-run` is optional**: a real run is safe on its own because every check happens before
   the tag. Use it to test changes to the script.
6. **If something fails** the script prints the exact commands (`gh run cancel ID`, and
   `gh workflow run native-release-monoptah.yml --ref main -f tag=native-vX.Y.Z`). Never re-run
   `native:release:mac` for an existing tag: it refuses. A run queued with no job for 10 minutes
   is cancelled and replaced by the script.

The release page and the macOS zip are public as soon as the script creates the release, and
they stay macOS-only if the workflow fails. The in-app feed and the cask change only at the
end, when `publish` and `homebrew` succeed. The workflow publishes:

- Release `native-vX.Y.Z` (never "latest": Tauri's updater reads `releases/latest`) with, for
  each updater package, the file and its `.sig`:
  - macOS arm64: `zptah_X.Y.Z_aarch64.app.tar.gz`, plus the `.zip` and `.zip.sha256`
    the cask pins.
  - Linux x86_64: `zptah_X.Y.Z_amd64.AppImage`, `zptah_X.Y.Z_amd64.deb`
    and `zptah-X.Y.Z-0.preview.0.el10.x86_64.rpm` (the release string comes from
    upstream's spec, which is written for previews).
  - Windows x86_64: `zptah_X.Y.Z_x64-setup.exe` (NSIS).
- `latest.json` on the rolling pre-release `native-latest`, the feed compiled into the app.
  It lists `darwin-aarch64-app`, `linux-x86_64-appimage`, `linux-x86_64-deb`,
  `linux-x86_64-rpm` and `windows-x86_64-nsis`, and it is never replaced by an older version.

Every platform is required: if a CI package job fails, nothing more is published. Re-run the
failed jobs or dispatch the workflow again with the tag only. Published assets are never
replaced: a platform whose files are all on the release is not rebuilt (its files are
downloaded and signed again) and one with no files is built, but never macOS. A platform with
only some files published fails the `plan` job: delete that platform's published assets (files
and `.sig`) by hand, then re-run. The signed files and feed are also kept as a workflow
artifact. The Linux and Windows packages use upstream's `scripts/package-native.mjs`.

The `homebrew` job only bumps `Casks/zptah.rb` in the tap `hhoangg/homebrew-zptah` (macOS only;
Linux and Windows have no package-manager entry), it does not create it: push
`packaging/homebrew/zptah.rb` to the tap as `Casks/zptah.rb` once before the first release
(overwriting any older file there), or the job fails. It runs after `publish` when
`HOMEBREW_TAP_TOKEN` is set. While it is unset, bump the cask by hand with
`scripts/update-homebrew-cask.sh` and the sha from the `zptah_<version>_aarch64.zip.sha256`
asset. The fork scripts are `scripts/native-release-macos-local.mjs`,
`scripts/native-release-notes.mjs`, `scripts/package-native-monoptah.mjs` (also the asset table
for every platform) and `scripts/native-release-monoptah.mjs`.

The release script removes the built `zptah.app` from `target/.../release` after packaging (also on failure and in `--dry-run`) and marks that directory `.metadata_never_index`. Agents must not leave built `.app` bundles behind: remove any you build outside the release flow when done (Native Dev's `~/Applications/zptah Dev.app` is the exception).

Things to keep true after merging from upstream or changing repo settings:

- Upstream's `native-release.yml` (and `scripts/native-release.mjs`) and the Tauri `ci.yml` and
  `release.yml` are deleted in this fork. If a merge brings one back (modify/delete conflict),
  keep the deletion: a restored workflow would run ungated here and fail without Apple and R2
  secrets.
- The rolling feed is replaced with `gh release upload --clobber`. Do not enable GitHub
  immutable releases for this repository, or the feed can never be updated and CI cannot upload
  to the release the local script created.
- `native.yml` and `native-release-monoptah.yml` must not use a macOS runner. The
  `MONOPTAH_MACOS_RUNNER` repository variable is unused now and can be deleted.
- Every job in `native.yml` and `native-release-monoptah.yml` keeps a `timeout-minutes`.
- `native-release-monoptah.yml` keeps its per-tag concurrency group (`native-monoptah-release-<tag>`)
  and the `native-monoptah-publish` group on the `publish` job. Do not turn on `cancel-in-progress`
  there, and do not go back to one global group: the early run waits up to 2 hours for the tag.
- The workflow runs its helper scripts from the `release-tools/` checkout of the dispatched ref, not from the built commit, so the tag-only recovery works for older tags.
- Only the `publish` job may see the signing key or have `contents: write`.
