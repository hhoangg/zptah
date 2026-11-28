# Deploy runbook (official host)

How the official Monoptah web host (`zptah.app`) is deployed. Self-hosters follow architecture.md section 9 instead.

## Who runs it

Only the maintainer, from their own Mac, with the wrangler OAuth login that exists only on that Mac (security.md
section 9). Agents, sandboxes and CI never run it, never hold Cloudflare credentials and never run
`wrangler deploy`, `wrangler d1 migrations apply --remote`, `wrangler secret put` or `wrangler login`. CI
(`web-ci.yml`) builds and tests only and holds no Cloudflare deploy token. The script refuses when `CI` is set and,
for a real run, when the OS is not macOS.

## Prerequisites (once)

- `wrangler login` done on the Mac, with 2FA on the Cloudflare account.
- The D1 database exists and its real `database_id` is in `web/wrangler.jsonc` (not the all-zero placeholder).
- `VAPID_PUBLIC_KEY` in `wrangler.jsonc` `vars` is the real key (not the `REPLACE_WITH_...` placeholder).
- The three secrets are set with `wrangler secret put`: `VAPID_PRIVATE_KEY`, `RECOVERY_PEPPER`, `AUDIT_IP_SALT`.
  Keep a copy in the sealed operator vault (losing `RECOVERY_PEPPER` invalidates stored recovery codes).
- The file transfer bucket exists, is private, and has its 1 day lifecycle rule (next section). `wrangler deploy` fails
  when the `TRANSFERS` binding names a bucket that does not exist. File transfers need no secret.

## File transfer bucket (once, and when moving to another account)

Files a device opens from the desktop travel as ciphertext through R2 (security.md "File transfers"). The Worker binding
`TRANSFERS` (`wrangler.jsonc` `r2_buckets`) names the bucket `monoptah-transfers`. Self-hosters create their own bucket
and change `bucket_name`. Run once, from `web/`, after `wrangler login`:

```bash
wrangler r2 bucket create monoptah-transfers
# Cleanup is ONLY this rule: R2 deletes every object under `transfers/` one day after it was written.
wrangler r2 bucket lifecycle add monoptah-transfers expire-transfers transfers/ --expire-days 1 --force
wrangler r2 bucket lifecycle list monoptah-transfers     # check: one rule, prefix transfers/, expire after 1 day
```

- Keep the bucket private: no custom domain, no `r2.dev` public access (`wrangler r2 bucket domain list monoptah-transfers` and
  `wrangler r2 bucket dev-url get monoptah-transfers` must show none). Only the Worker reads and writes it.
- There is no cron and no early delete. R2 applies the rule once a day, so an object can stay up to about two days; the
  desktop treats a key as stale after three days and uploads again when an object is missing (it checks with `HEAD` on every
  open), so a late or early deletion changes nothing the user sees.
- The Worker never overwrites an object and never lists the bucket. Free tier: the bucket stays small because of the rule
  (a day of previews, at most 64 MiB each); watch R2 storage in the dashboard after the first weeks.
- Local development and the tests need nothing: `wrangler dev` and the Worker tests simulate the bucket in
  Miniflare (`--local`), including the 1 day rule's absence (objects are simply kept in the local state).
- Rollback of this feature is deploying the previous Worker. The bucket may stay: its rule empties it within two days.

## Deploy

From a clean checkout, in `web/`:

```bash
pnpm deploy:official --dry-run   # preflight for real, prints the plan, runs no remote command
pnpm deploy:official             # the real deploy
```

Order, enforced by the script (`web/scripts/deploy-official.mjs`):

1. Preflight, stopping at the first failure: `CI` not set; macOS (real run only); tree clean (untracked files
   count); branch is `main`; `main` equals `origin/main` after `git fetch origin main`; `wrangler.jsonc` has no
   placeholder `database_id` or VAPID key; `pnpm install --frozen-lockfile` passes (the installed dependencies
   match the lockfile); `pnpm check` passes; `scripts/check-fork-branding.sh` passes.
2. Build the app: `pnpm --filter @monoptah/app build`.
3. Migrations FIRST: `wrangler d1 migrations apply monoptah --remote -c wrangler.jsonc`. A failure stops the run
   before the deploy.
4. Deploy: `wrangler deploy -c wrangler.jsonc`. Every deploy disconnects all relay sockets; clients reconnect.
5. Audit: one `audit_log` row (`actor_kind` `system`, action `system.deploy`, meta with only the 40-character commit sha
   and the app version, both validated before the SQL is built). If this step fails the deploy has already happened: insert the row by hand.
6. The script prints one line. Append it to the deploy log below in the same commit that records the deploy
   (a PR titled `docs(web): record deploy <sha>` or the next change).

### Post-deploy check (first internal deploy)

Batches rely on SQLite `changes()` carrying from one statement to the next inside a D1 batch (security.md 3.5), which
the Cloudflare D1 docs do not state. On the first internal deploy, check on the real D1 that:

1. One passkey login succeeds and leaves one session row and one `account.login` audit row.
2. One session revoke (`DELETE /api/sessions/:id` of another session) deletes that row and leaves one
   `session.revoke` audit row.

If either row is missing or doubled, treat the deploy as bad and roll back.

## Landing page and its beta host

The public landing page lives in `web/landing/` (plain HTML templates in `src/html`, CSS in `src/styles`, vanilla ES modules in
`src/js`, no dependencies; `node landing/build.mjs --target prod|beta`). Every `vite build` of the app runs it
(`app/vite-plugins/landing.ts`) and writes it to `app/dist/client/landing/`, plus `robots.txt` and `sitemap.xml` at the root of
`app/dist/client`, so it ships inside the production Worker's assets however the app is built (the app build script, the e2e
harness, CI, the official deploy). Canonical, `og:*` and the sitemap use the `RP_ORIGIN` of `wrangler.jsonc`.

How production serves it (`worker/src/landing.ts`, `run_worker_first` of `wrangler.jsonc` lists `/`, `/home`, `/home/`, `/privacy`, `/privacy/`):

- `GET /` without a `__Host-mc_session` cookie: the landing page. With the cookie: the SPA, exactly as before. Presence check only: no
  D1 call, no validation (a stale cookie gets the SPA, whose auth guard sends it to `/login`). The cookie is `SameSite=Strict`, so a
  signed-in user who arrives from another site (a link in chat or search) sees the landing page once and uses "Sign in" or "Open web app".
- `GET /privacy`: the privacy policy of the Mono Browser (Ardia) (`landing/privacy/`), for everyone, no cookie variance.
- `GET /home`: always the landing page. Both answers carry `Vary: Cookie` and `Cache-Control: no-store`; HEAD behaves the same.
- The PWA `start_url` is `/app` (redirects to the last place, or to `/login` when signed out); `scope` and `id` stay `/`.
- Service worker: `/`, `/home`, `/landing/` and `/privacy` are on the navigation denylist (queries included) and precaching has
  `directoryIndex: ""`, so a signed-out visitor with the new service worker gets the landing page. Navigations to exactly `/` go
  network first, and fall back to the precached shell when offline, so an app installed with the old `start_url` `/` still opens
  offline. Online, such an installed app is sent from the landing page to `/app` by `landing/src/js/boot.js` (standalone display mode).
- Transition after the first deploy: the service worker already on users' devices has the old routing (default `directoryIndex`,
  `registerType: "prompt"`: a new worker waits until the user taps Reload or every tab is closed). Until the new worker activates, a
  returning visitor with the old worker gets the cached app shell at `/` (which sends a signed-out user to `/login`) instead of the
  landing page. New visitors and visitors without a service worker see the landing page at once. This is expected, not a bug.
- `robots.txt` and `sitemap.xml` are generated from `landing/src/robots.txt` and the build host. robots disallows `/admin`, `/api/`,
  `/app` (exactly and `/app/`, not `/apple-touch-icon.png`), `/d/`, `/desktops`, `/pair`, `/relay/`, `/settings`, and the login,
  register and recover forms (nothing to index). It keeps `/install` (the public install guide) and `/landing/` (the page's CSS and
  JS) crawlable. The sitemap lists `https://zptah.app/` only; `/home` is the same page and the canonical link points at `/`.
- The page runs under the same CSP as the app (no inline script or style, no HTML string sinks because of Trusted Types);
  `landing/test/build.test.mjs` and `app/scripts/check-dist.mjs` enforce that.

Nothing of this is live until the Worker is deployed with the normal procedure above (`pnpm deploy:official` after the PR is merged).
`vite dev` does not build the landing page (`/landing/` falls back to the SPA there); use `wrangler dev` on a built app to see it.

### Landing beta (`beta.zptah.app`)

A separate, assets-only Worker `zptah-landing-beta` (`landing/wrangler.beta.jsonc`) serves the beta build for review before the
production Worker is deployed. It binds nothing (no D1, R2, Durable Object, var or secret) and has no script, so it cannot reach
production data. The beta build differs from the production build only by flags: Sign in, Get started and Open web app point to the
absolute `https://zptah.app/login` and `/register`, the page has `<meta name="robots" content="noindex, nofollow">`, `robots.txt` is
`Disallow: /`, and `_headers` carries the production CSP plus `X-Robots-Tag: noindex, nofollow`.

```bash
cd web
pnpm landing:deploy:beta --dry-run   # builds landing/dist/beta, wrangler dry run
pnpm landing:deploy:beta             # real deploy (maintainer Mac, wrangler login; refuses when CI is set)
```

The first deploy creates the custom domain `beta.zptah.app` on the `zptah.app` zone (DNS and certificate are created by wrangler).
Remove it with `wrangler delete --name zptah-landing-beta` (from `web/worker`); it never touches `monoptah-web`.
When the landing page is approved: merge the PR, deploy production as usual, and delete the beta Worker if it is no longer needed.

## VAPID key rotation

Only during a rotation, the Worker also reads a previous key pair, so subscriptions made with the old key keep
receiving pushes until their browsers re-subscribe. The full procedure (why, who, drain check, staging drill) is
security.md section 9.1; this is the configuration it needs:

| Name | Where | Value |
|---|---|---|
| `VAPID_PREVIOUS_PUBLIC_KEY` | `wrangler.jsonc` `vars` (public) | The old public key, base64url |
| `VAPID_PREVIOUS_KEY_ID` | `wrangler.jsonc` `vars` | The old key id (`push_subscriptions.vapid_key_id` of its rows) |
| `VAPID_PREVIOUS_PRIVATE_KEY` | `wrangler secret put` | The old private key. A secret, never in `vars` |

All three are unset normally and are declared optional in `worker/src/cloudflare-env.d.ts`. While they are set,
`VAPID_PUBLIC_KEY`, `VAPID_KEY_ID` and `VAPID_PRIVATE_KEY` hold the new pair; a push is signed with the key whose
id matches the subscription, and new subscriptions are accepted only on the current id. When the old key drains (or
leaked: leave the previous slot empty), delete the two vars and run `wrangler secret delete VAPID_PREVIOUS_PRIVATE_KEY`,
then deploy. A half-set trio is ignored by the Worker (the previous key stays off).

## Rollback

Migrations are forward-only and backward compatible for one release (architecture.md section 9), so the previous
Worker version still runs on the new schema. To roll back a bad Worker:

```bash
wrangler rollback -c wrangler.jsonc    # from web/, on the maintainer Mac
```

Never roll back a migration, and never edit an applied one: fix forward with a new migration. If a release shipped a
migration that is not backward compatible, a Worker rollback is not safe: fix forward instead. Record the rollback
in the log below with `(rollback)` after the operator name. Point-in-time restore of D1 (Time Travel) is the
last resort for data damage (security.md section 9).

## Deploy log

Append the printed line (date, commit, app version, operator) at the end.

| Date | Commit | App version | Operator |
|---|---|---|---|
| 2026-11-19 | d1e9a7e6d2fa22a25e3fd66fa567dc4e687b676e | 0.1.0 | hhoangg |
| 2026-11-19 | 2717dabe036eb82252b09e23dc6391e2e2411bfd | 0.1.0 | hhoangg |
| 2026-11-19 | 775f5e550d415999a070b7f4c36478eff3a5d17d | 0.1.0 | hhoangg |
| 2026-11-19 | 27aab66b02441ab1c0870ab74269d6041b711774 | 0.1.0 | hhoangg |
| 2026-11-19 | 799f29b52e24caa8679befb8b8e3a1168e6313cf | 0.1.0 | hhoangg |
| 2026-11-20 | c52337c55eb7c8aba66a13c3e55badcbd570ce5e | 0.1.0 | hhoangg |
| 2026-11-20 | 242e974c31c07e5cc06deac6c89b552d730cec06 | 0.1.0 | hhoangg |
| 2026-11-20 | 2ada231eafceb0f7e745d672ec3dcc321f3156c0 | 0.1.0 | hhoangg |
| 2026-11-20 | f47945205cfd3c80946a55028e846de940698f18 | 0.1.0 | hhoangg |
| 2026-11-21 | 4c02da189c90aabd8fef49cd65860a2b2d1da77c | 0.1.0 | hhoangg |
| 2026-11-21 | 0079c5da099b410cddffe018e036538415aea881 | 0.1.0 | hhoangg |
| 2026-11-21 | f1ed4a2030e2f645221a786f85ff6b9fb54d2ce7 | 0.1.0 | hhoangg |
| 2026-11-23 | ee77a523e51d5ff58bda0b8ac38b61c17bea706c | 0.1.0 | hhoangg |
| 2026-11-23 | 50996678e144a8e02f7ac0c066fd274d59dc7491 | 0.1.0 | hhoangg |
| 2026-11-24 | 2f275e1a6310ae3650e4bf32bb6aeb7221c25322 | 0.1.0 | hhoangg |
| 2026-11-24 | 739ce646ea809569289b695fffc5c80c77e6feb1 | 0.1.0 | hhoangg |
| 2026-11-24 | ed462f9f765e4394d37c71aec4806502615f08ef | 0.1.0 | hhoangg |
| 2026-11-24 | 835f4b4b8baa237c6ea79065e3ee800089873c3d | 0.1.0 | hhoangg |
| 2026-11-25 | 2381f8b5cac5c3eb3bb9b34881805bf51c4059d4 | 0.1.0 | hhoangg |
| 2026-11-25 | ce03dda93748cbf9a59a4b39c0bef7f67cc54042 | 0.1.0 | hhoangg |
| 2026-11-25 | afa1d93308d24f73a0e2f2883f482d4eef165333 | 0.1.0 | hhoangg |
| 2026-11-26 | c558aed5c13de3f1f6671ff6b366abfaf8346843 | 0.1.0 | hhoangg |
| 2026-11-26 | a3e483fb51baa210ea9d118e487029bfdfc16eee | 0.1.0 | hhoangg |
| 2026-11-26 | 4d2f5dd3511bbb6f4588c26038617ef813daf58e | 0.1.0 | hhoangg |
