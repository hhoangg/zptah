// Builds the beta target and deploys the separate `zptah-landing-beta` Worker. Run from web/ on the maintainer Mac
// (`pnpm landing:deploy:beta`, `--dry-run` for a plan that deploys nothing). It never touches wrangler.jsonc, the
// production Worker, D1 or secrets.
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from './build.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const webDir = path.join(here, '..');
const dry = process.argv.includes('--dry-run');

if (process.env.CI) throw new Error('landing:deploy:beta is a manual deploy: it does not run in CI');

const res = await build({ target: 'beta', out: path.join(here, 'dist', 'beta') });
console.log(`built ${res.landing}`);
// The wrangler of the worker package, run from web/ like the official deploy.
const wrangler = path.join(webDir, 'worker', 'node_modules', '.bin', 'wrangler');
const args = ['deploy', '-c', 'landing/wrangler.beta.jsonc', ...(dry ? ['--dry-run'] : [])];
const run = spawnSync(wrangler, args, { cwd: webDir, stdio: 'inherit' });
process.exit(run.status ?? 1);
