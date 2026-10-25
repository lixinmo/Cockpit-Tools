#!/usr/bin/env node
// Cloud-only compiled state. Never cache an App, bundle, profile or user data.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { platforms } = require('./cloud-build.cjs');

const ROOT = path.resolve(__dirname, '../..');
const MAX_BYTES = 6 * 1024 ** 3;
const BUILD_DIRS = ['build', '.fingerprint', 'deps', 'incremental'];
const BUILD_INPUTS = [
  '.github/workflows/build-matrix.yml', '.github/actions/host-build-cache/action.yml',
  'scripts/ci/cloud-build.cjs', 'scripts/ci/cloud-build-state-cache.cjs',
  'src-tauri/build.rs', 'src-tauri/tauri.conf.json', 'package.json', 'package-lock.json',
];

function trusted(env) {
  return env.GITHUB_ACTIONS === 'true'
    && ['push', 'workflow_dispatch'].includes(env.GITHUB_EVENT_NAME)
    && /^refs\/heads\/.+/.test(env.GITHUB_REF || '')
    && !env.GITHUB_HEAD_REF && !env.GITHUB_BASE_REF
    && /^[a-z0-9][a-z0-9-]*\/(?!\.{1,2}$)[\w.-]+$/i.test(env.GITHUB_REPOSITORY || '')
    && /^[a-f0-9]{40}$/i.test(env.GITHUB_SHA || '');
}

function selection(platform, mode) {
  const selected = platforms.find(item => item.label === platform);
  if (!selected || !['full', 'probe'].includes(mode)) throw new Error('invalid_selection');
  if (mode === 'probe' && selected.native_mac !== '1') throw new Error('invalid_probe_platform');
  return selected;
}

function cachePaths(platform) {
  const { target } = selection(platform, 'full');
  return ['target/release', `target/${target}/release`]
    .flatMap(directory => BUILD_DIRS.map(name => `${directory}/${name}`));
}

function namespace(platform, mode) {
  selection(platform, mode);
  return `ct-cloud-state-v1-${platform}-${mode}-`;
}

function cachePlan({ platform, mode, env, rustVersion, inputs }) {
  const scope = namespace(platform, mode);
  if (!trusted(env)) return { enabled: false };
  if (!rustVersion?.trim()) throw new Error('rust_version_required');
  const compilerEnv = {};
  for (const name of ['RUNNER_OS', 'RUNNER_ARCH', 'ImageOS', 'ImageVersion', 'RUSTFLAGS',
    'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CC', 'CXX',
    'CFLAGS', 'CXXFLAGS', 'MACOSX_DEPLOYMENT_TARGET', 'SDKROOT']) compilerEnv[name] = env[name] || '';
  const fingerprint = crypto.createHash('sha256').update(JSON.stringify({
    rustVersion: rustVersion.trim(), compilerEnv,
    profiles: { incremental: true, codegenUnits: 16, appOptLevel: 3, probeOptLevel: 0 },
    inputs: Object.entries(inputs).sort(([left], [right]) => left.localeCompare(right)),
  })).digest('hex').slice(0, 32);
  const prefix = `${scope}${fingerprint}-`;
  return { enabled: true, key: `${prefix}${env.GITHUB_SHA.toLowerCase()}`,
    prefix, namespace: scope, paths: cachePaths(platform).join('\n') };
}

function inputHashes(root) {
  const tracked = execFileSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8' }).split('\0');
  const names = new Set([...BUILD_INPUTS, ...tracked.filter(name =>
    /(^|\/)Cargo\.(toml|lock)$/.test(name) || /(^|\/)\.cargo\/config(\.toml)?$/.test(name)
    || /(^|\/)rust-toolchain(\.toml)?$/.test(name))]);
  return Object.fromEntries([...names].sort().map(name => {
    const file = path.join(root, name);
    if (!fs.lstatSync(file).isFile()) throw new Error('invalid_build_input');
    return [name, crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')];
  }));
}

function measureState(root, platform, maxBytes = MAX_BYTES) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) throw new Error('invalid_size_cap');
  const result = { bytes: 0, files: 0, incrementalBytes: 0, incrementalFiles: 0, save: false, reason: '' };
  const fail = (file, type, code) => {
    throw Object.assign(new Error('cache_path_blocked'), {
      blockedPath: path.relative(root, file).split(path.sep).join('/'), blockedType: type,
      blockedCode: /^[A-Z0-9_]+$/.test(code || '') ? code : 'IO_ERROR',
    });
  };
  const kind = stat => stat.isSymbolicLink() ? 'symlink' : stat.isDirectory() ? 'directory'
    : stat.isFile() ? 'file' : 'other';
  const visit = (file, incremental) => {
    let stat;
    try { stat = fs.lstatSync(file); } catch (error) { fail(file, 'unknown', error.code); }
    if (stat.isSymbolicLink() || (!stat.isDirectory() && !stat.isFile())) fail(file, kind(stat), 'UNSUPPORTED_ENTRY');
    if (stat.isDirectory()) {
      let entries;
      try { entries = fs.readdirSync(file); } catch (error) { fail(file, 'directory', error.code); }
      for (const entry of entries) visit(path.join(file, entry), incremental);
    } else {
      result.bytes += stat.size;
      result.files += 1;
      if (incremental) { result.incrementalBytes += stat.size; result.incrementalFiles += 1; }
    }
  };
  try {
    for (const relative of cachePaths(platform)) {
      let directory = root;
      let missing = false;
      // Reject a symlink in an ancestor as well as inside a cache directory.
      for (const part of relative.split('/')) {
        directory = path.join(directory, part);
        let stat;
        try { stat = fs.lstatSync(directory); }
        catch (error) {
          if (error.code === 'ENOENT') { missing = true; break; }
          fail(directory, 'unknown', error.code);
        }
        if (!stat.isDirectory() || stat.isSymbolicLink()) fail(directory, kind(stat), 'UNSUPPORTED_ANCESTOR');
      }
      if (!missing) visit(directory, relative.endsWith('/incremental'));
    }
    result.reason = result.bytes > maxBytes ? 'size_limit' : result.incrementalBytes === 0
      ? 'incremental_missing' : 'ready';
    result.save = result.reason === 'ready';
  } catch (error) {
    result.reason = 'unsupported_or_unreadable_path';
    if (error.message === 'cache_path_blocked') {
      result.blockedPath = error.blockedPath;
      result.blockedType = error.blockedType;
      result.blockedCode = error.blockedCode;
    }
  }
  return result;
}

function pruneCandidates(entries, { ref, namespace: scope, sha }) {
  const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const pattern = new RegExp(`^${escape(scope)}[a-f0-9]{32}-[a-f0-9]{40}$`);
  const scoped = [...new Map(entries.filter(entry => entry.ref === ref
    && pattern.test(entry.key || '') && Number.isSafeInteger(entry.id) && entry.id > 0
    && Number.isFinite(Date.parse(entry.created_at))).map(entry => [entry.id, entry])).values()]
    .sort((left, right) => Date.parse(right.created_at) - Date.parse(left.created_at) || right.id - left.id);
  const current = scoped.find(entry => entry.key.endsWith(`-${sha.toLowerCase()}`));
  if (!current) return { ids: [], reason: 'current_cache_missing' };
  // An older all/platform run must not remove a newer run's cache.
  if (scoped[0].id !== current.id) return { ids: [], reason: 'newer_cache_present' };
  return { ids: scoped.slice(1).map(entry => entry.id), reason: 'keep_latest_one' };
}

async function prune(env, platform, mode, fetcher = fetch) {
  if (!trusted(env)) return { deleted: 0, reason: 'untrusted_event' };
  if (!env.GH_TOKEN) return { deleted: 0, reason: 'token_unavailable' };
  if (env.GITHUB_API_URL && env.GITHUB_API_URL !== 'https://api.github.com') {
    return { deleted: 0, reason: 'unsupported_api_origin' };
  }
  const scope = namespace(platform, mode);
  const base = `https://api.github.com/repos/${env.GITHUB_REPOSITORY}/actions/caches`;
  const headers = { Authorization: `Bearer ${env.GH_TOKEN}`, Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28' };
  const entries = [];
  for (let page = 1; ; page += 1) {
    if (page > 20) return { deleted: 0, reason: 'pagination_limit' };
    const query = new URLSearchParams({ ref: env.GITHUB_REF, key: scope,
      sort: 'created_at', direction: 'desc', per_page: '100', page: String(page) });
    const response = await fetcher(`${base}?${query}`, { headers, signal: AbortSignal.timeout(30_000) });
    if (!response.ok) return { deleted: 0, reason: `list_http_${response.status}` };
    const body = await response.json();
    if (!Array.isArray(body.actions_caches)) return { deleted: 0, reason: 'invalid_list' };
    entries.push(...body.actions_caches);
    if (body.actions_caches.length < 100) break;
  }
  const candidates = pruneCandidates(entries, { ref: env.GITHUB_REF, namespace: scope, sha: env.GITHUB_SHA });
  let deleted = 0;
  for (const id of candidates.ids) {
    const response = await fetcher(`${base}/${id}`, { method: 'DELETE', headers, signal: AbortSignal.timeout(30_000) });
    if (response.ok) deleted += 1;
    else if (response.status !== 404) return { deleted, reason: `delete_http_${response.status}` };
  }
  return { deleted, reason: candidates.reason };
}

function outputs(values) {
  if (process.env.GITHUB_OUTPUT) {
    for (const [name, raw] of Object.entries(values)) {
      const value = String(raw);
      const delimiter = `CACHE_${crypto.randomBytes(12).toString('hex')}`;
      fs.appendFileSync(process.env.GITHUB_OUTPUT, `${name}<<${delimiter}\n${value}\n${delimiter}\n`);
    }
  }
}

async function main(args, env = process.env) {
  const [command, platform, mode] = args;
  if (env.GITHUB_ACTIONS !== 'true') throw new Error('GitHub Actions only');
  selection(platform, mode);
  if (!['prepare', 'measure', 'prune'].includes(command)) throw new Error('invalid_command');
  if (!trusted(env)) { outputs({ enabled: false, save: false }); return; }
  if (command === 'prepare') {
    const plan = cachePlan({ platform, mode, env,
      rustVersion: execFileSync('rustc', ['-vV'], { encoding: 'utf8' }), inputs: inputHashes(ROOT) });
    outputs(plan);
    console.log(JSON.stringify({ stage: 'build_state_cache', enabled: true, key: plan.key }));
  } else if (command === 'measure') {
    const result = measureState(ROOT, platform);
    outputs(result);
    console.log(JSON.stringify({ stage: 'build_state_cache_measure', ...result, limitBytes: MAX_BYTES }));
    if (!result.save) console.log(`::warning::Build-state cache not saved: ${result.reason}.`);
  } else {
    const result = await prune(env, platform, mode);
    outputs(result);
    console.log(JSON.stringify({ stage: 'build_state_cache_prune', ...result }));
  }
}

if (require.main === module) main(process.argv.slice(2)).catch(error => {
  if (process.env.GITHUB_ACTIONS !== 'true') { console.error('GitHub Actions only'); process.exitCode = 1; }
  else {
    outputs({ enabled: false, save: false, reason: 'cache_maintenance_failed' });
    console.log('::warning::Build-state cache maintenance failed; validated build results are unchanged.');
  }
});

module.exports = { MAX_BYTES, BUILD_DIRS, trusted, cachePaths, namespace, cachePlan, measureState, pruneCandidates, prune };
