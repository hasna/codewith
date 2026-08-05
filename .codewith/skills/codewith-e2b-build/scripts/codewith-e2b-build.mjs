#!/usr/bin/env bun
// codewith-e2b-build: build/test a Codewith Rust crate on a remote E2B sandbox.
//
// Source of truth is git: the box does `git fetch origin <branch> && checkout`,
// so nothing is lost when a box expires. The box is ephemeral compute only.
//
// Usage:
//   codewith-e2b-build --branch <git-branch> --crate <crate> [--crate <crate> ...] [opts]
//
// See ../SKILL.md for full docs. Run with --help for flags.

import { spawnSync } from 'node:child_process';
import { mkdirSync, createWriteStream } from 'node:fs';
import { homedir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

// --- Per-template environment (only codewith-pr-drain is validated end-to-end) ---
// The E2B command runner executes as unprivileged `user` with a minimal PATH and
// never inherits the Docker image ENV or rustup's default toolchain, so we pass the
// toolchain env explicitly and run as root (root also owns /opt/codewith for git).
const TEMPLATES = {
  'codewith-pr-drain': {
    repo: '/opt/codewith',
    target: '/opt/codewith-target',
    envs: {
      CARGO_HOME: '/opt/rust/cargo',
      RUSTUP_HOME: '/opt/rust/rustup',
      PATH: '/opt/rust/cargo/bin:/root/.bun/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
      RUST_MIN_STACK: '8388608',
    },
  },
};
const DEFAULT_TEMPLATE = 'codewith-pr-drain';

function parseArgs(argv) {
  const a = {
    crates: [], template: DEFAULT_TEMPLATE, timeoutMin: 90, extra: [],
    mode: 'test', // test | check | full
    keep: false, kill: false, pause: false, all: false, json: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const v = argv[i];
    const next = () => argv[++i];
    if (v === '--branch') a.branch = next();
    else if (v === '--sha') a.sha = next();
    else if (v === '--crate' || v === '-p') a.crates.push(next());
    else if (v === '--all') a.all = true;
    else if (v === '--check') a.mode = 'check';
    else if (v === '--full') a.mode = 'full';
    else if (v === '--template') a.template = next();
    else if (v === '--sandbox') a.sandbox = next();
    else if (v === '--worktree') a.worktree = next();
    else if (v === '--repo') a.repo = next();
    else if (v === '--target') a.target = next();
    else if (v === '--timeout-min') a.timeoutMin = parseInt(next(), 10);
    else if (v === '--keep') a.keep = true;
    else if (v === '--kill') a.kill = true;
    else if (v === '--pause') a.pause = true;
    else if (v === '--json') a.json = true;
    else if (v === '-h' || v === '--help') a.help = true;
    else if (v === '--') { a.extra.push(...argv.slice(i + 1)); break; }
    else a.extra.push(v);
  }
  return a;
}

const HELP = `codewith-e2b-build — remote Codewith Rust build/test on E2B

Required:
  --branch <b>        git branch pushed to origin (source of truth)
  --crate <c>         crate to build/test, e.g. codex-core (repeatable; or -p)

Build mode (default: test):
  --check             compile-only (just check-fast), fastest signal
  --full              official gate (just test, includes bench-smoke)
  -- <args>           extra args passed to the just recipe (e.g. --test <bin>)

Sandbox lifecycle:
  --sandbox <id>      reuse a running box (keeps target-dir cache warm)
  --keep              leave a fresh box running after build (prints reuse cmd)
  --pause             snapshot+pause the box after build (warm resume later)
  --kill              force-kill even a reused box
  --timeout-min <n>   box lifetime / keepalive window (default 90)
  --template <t>      E2B template (default codewith-pr-drain)

Other:
  --sha <sha>         build an exact commit instead of branch tip
  --worktree <path>   apply local 'git diff HEAD' from this checkout over the branch
  --json              print a final machine-readable JSON result line

Auth: reads E2B_API_KEY, else 'secrets get hasnaxyz/e2b/live/api_key --raw'.
Default cleanup: a fresh box is killed after the build unless --keep/--pause.
A reused (--sandbox) box is left running unless --kill.`;

function getApiKey() {
  if (process.env.E2B_API_KEY) return process.env.E2B_API_KEY;
  const r = spawnSync('secrets', ['get', 'hasnaxyz/e2b/live/api_key', '--raw'], { encoding: 'utf8' });
  if (r.status === 0 && r.stdout.trim()) return r.stdout.trim();
  console.error('ERROR: no E2B_API_KEY and `secrets get` failed. Export E2B_API_KEY.');
  process.exit(2);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) { console.log(HELP); return 0; }
  if (!args.branch) { console.error('ERROR: --branch is required (source of truth = git).\n'); console.log(HELP); return 2; }
  if (!args.crates.length && !args.all) { console.error('ERROR: pass at least one --crate (or --all to build the whole workspace).\n'); console.log(HELP); return 2; }

  const tpl = TEMPLATES[args.template] || TEMPLATES[DEFAULT_TEMPLATE];
  const repo = args.repo || tpl.repo;
  const target = args.target || tpl.target;
  const envs = { ...tpl.envs, CARGO_TARGET_DIR: target };
  const apiKey = getApiKey();
  const timeoutMs = args.timeoutMin * 60 * 1000;

  const { Sandbox } = await import('e2b').catch(async (e) => {
    console.error('e2b SDK not found; installing into skill scripts dir…');
    const r = spawnSync('bun', ['install'], { cwd: HERE, stdio: 'inherit' });
    if (r.status !== 0) { console.error('bun install failed', String(e)); process.exit(2); }
    return import('e2b');
  });

  // log file
  const logDir = join(homedir(), '.cache', 'codewith-e2b-build', 'logs');
  mkdirSync(logDir, { recursive: true });
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const logPath = join(logDir, `${stamp}-${args.branch.replace(/[\/]/g, '_')}.log`);
  const logStream = createWriteStream(logPath);
  const tail = [];
  const sink = (d) => { process.stdout.write(d); logStream.write(d); tail.push(d); if (tail.length > 400) tail.shift(); };

  const t0 = Date.now();
  let sbx, reused = false;
  if (args.sandbox) {
    sbx = await Sandbox.connect(args.sandbox, { apiKey });
    reused = true;
    console.log(`## reusing sandbox ${sbx.sandboxId} (${args.template})`);
  } else {
    sbx = await Sandbox.create(args.template, { apiKey, timeoutMs });
    console.log(`## created sandbox ${sbx.sandboxId} (${args.template}) in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  }

  // keepalive: keep extending the box lifetime so long builds never expire mid-run
  const keepalive = setInterval(() => { sbx.setTimeout(timeoutMs).catch(() => {}); }, 30_000);

  const runOpts = { user: 'root', envs, cwd: repo, timeoutMs, onStdout: sink, onStderr: sink };
  const run = async (label, cmd, o = {}) => {
    console.log(`\n=== ${label} ===`);
    const s = Date.now();
    const r = await sbx.commands.run(cmd, { ...runOpts, ...o }).catch((e) => e.result || { exitCode: 1, error: String(e) });
    console.log(`\n-- ${label}: exit=${r.exitCode} (${((Date.now() - s) / 1000).toFixed(1)}s)`);
    return r;
  };

  let result = { ok: false, exit: 1 };
  try {
    // 1. Sync source of truth from git (branch or exact SHA). reset --hard makes it reproducible.
    const ref = args.sha || `origin/${args.branch}`;
    const fetchRef = args.sha ? args.sha : args.branch;
    const gitCmd =
      `git config --global --add safe.directory ${repo} && ` +
      `git fetch origin ${fetchRef} && ` +
      `git checkout -B ci-e2b ${args.sha ? args.sha : 'FETCH_HEAD'} && ` +
      `git reset --hard ${args.sha ? args.sha : 'FETCH_HEAD'} && git log -1 --oneline`;
    const g = await run('git sync', gitCmd);
    if (g.exitCode !== 0) throw new Error('git sync failed');

    // 2. Optional: apply an uncommitted local diff on top (branch push remains preferred).
    if (args.worktree) {
      const diff = spawnSync('git', ['-C', args.worktree, 'diff', 'HEAD'], { encoding: 'utf8', maxBuffer: 1 << 28 });
      if (diff.stdout && diff.stdout.trim()) {
        await sbx.files.write('/tmp/e2b-local.patch', diff.stdout); // /tmp is user-writable; repo is root-owned
        const ap = await run('apply worktree diff', `git apply --whitespace=nowarn /tmp/e2b-local.patch && echo applied`);
        if (ap.exitCode !== 0) throw new Error('worktree diff failed to apply');
      } else {
        console.log('## --worktree given but no local diff vs HEAD; using pushed branch as-is');
      }
    }

    // 3. Scoped build/test.
    const crateArgs = args.crates.map((c) => `-p ${c}`).join(' ');
    let recipe;
    if (args.mode === 'check') recipe = `just check-fast ${crateArgs}`;
    else if (args.mode === 'full') recipe = `just test ${crateArgs}`;
    else recipe = `just test-fast-target ${target} ${crateArgs}`; // default: fast + persistent warm target
    const cmd = `${recipe} ${args.extra.join(' ')}`.trim();
    const b = await run(`build (${args.mode})`, cmd);
    result = { ok: b.exitCode === 0, exit: b.exitCode };
  } catch (e) {
    console.error('## error:', String(e));
  } finally {
    clearInterval(keepalive);
    logStream.end();
  }

  // Summarize
  const joined = tail.join('');
  const m = joined.match(/Summary\s*\[[^\]]*\]\s*(\d+ tests? run:[^\n]*)/);
  const summary = m ? m[1].trim() : (result.ok ? 'build/check passed' : 'no nextest summary (compile or test failure)');
  const verdict = result.ok ? 'PASS' : 'FAIL';
  const wall = ((Date.now() - t0) / 1000).toFixed(1);

  // Cleanup policy
  let disposition;
  if (args.pause) { await sbx.betaPause().catch(() => {}); disposition = `paused (resume: --sandbox ${sbx.sandboxId})`; }
  else if (args.kill) { await sbx.kill(); disposition = 'killed'; }
  else if (reused || args.keep) { await sbx.setTimeout(timeoutMs).catch(() => {}); disposition = `kept running (reuse: --sandbox ${sbx.sandboxId})`; }
  else { await sbx.kill(); disposition = 'killed'; }

  console.log(`\n================ RESULT ================`);
  console.log(`verdict:   ${verdict}`);
  console.log(`crates:    ${args.crates.join(', ') || '(workspace)'}`);
  console.log(`branch:    ${args.branch}${args.sha ? ` @ ${args.sha}` : ''}`);
  console.log(`summary:   ${summary}`);
  console.log(`exit:      ${result.exit}`);
  console.log(`wall:      ${wall}s`);
  console.log(`sandbox:   ${sbx.sandboxId} (${disposition})`);
  console.log(`log:       ${logPath}`);
  console.log(`=======================================`);
  if (args.json) {
    console.log('JSON ' + JSON.stringify({
      verdict, exit: result.exit, crates: args.crates, branch: args.branch, sha: args.sha || null,
      summary, wallSeconds: Number(wall), sandboxId: sbx.sandboxId, disposition, logPath, template: args.template,
    }));
  }
  return result.ok ? 0 : 1;
}

main().then((c) => process.exit(c)).catch((e) => { console.error(e); process.exit(1); });
