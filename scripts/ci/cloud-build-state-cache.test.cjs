const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');
const { MAX_BYTES, trusted, cachePaths, namespace, cachePlan, measureState, inspectCompiledState, pruneCandidates, prune } = require('./cloud-build-state-cache.cjs');

const env = {
  GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: 'refs/heads/codex/test',
  GITHUB_REPOSITORY: 'example/cockpit-tools', GITHUB_SHA: 'a'.repeat(40),
  RUNNER_OS: 'macOS', RUNNER_ARCH: 'X64', ImageOS: 'macos15', ImageVersion: 'one',
};
const options = { platform: 'macos-x86_64', mode: 'full', env, rustVersion: 'rustc 1.test host=x86_64',
  inputs: { 'Cargo.lock': 'lock-hash', 'Cargo.toml': 'manifest-hash' } };

test('only trusted same-repository branch events can retain compiled state', () => {
  assert.equal(trusted(env), true);
  assert.equal(trusted({ ...env, GITHUB_EVENT_NAME: 'push' }), true);
  for (const change of [{ GITHUB_ACTIONS: 'false' }, { GITHUB_EVENT_NAME: 'pull_request' },
    { GITHUB_EVENT_NAME: 'pull_request_target' }, { GITHUB_EVENT_NAME: 'workflow_run' },
    { GITHUB_REF: 'refs/tags/v1.0' }, { GITHUB_HEAD_REF: 'foreign-branch' },
    { GITHUB_BASE_REF: 'main' }, { GITHUB_REPOSITORY: '../outside' }, { GITHUB_SHA: 'missing' }]) {
    assert.equal(trusted({ ...env, ...change }), false, JSON.stringify(change));
    assert.deepEqual(cachePlan({ ...options, env: { ...env, ...change } }), { enabled: false });
  }
});

test('source changes restore the same compatible prefix; toolchain and build inputs invalidate it', () => {
  const original = cachePlan(options);
  const changedSource = cachePlan({ ...options, env: { ...env, GITHUB_SHA: 'b'.repeat(40) } });
  assert.equal(original.prefix, changedSource.prefix);
  assert.notEqual(original.key, changedSource.key);
  assert.equal(original.namespace, 'ct-cloud-incremental-v1-macos-x86_64-full-');
  assert.equal(original.key, original.prefix + env.GITHUB_SHA);
  const variants = [
    { rustVersion: 'rustc next' }, { inputs: { ...options.inputs, 'Cargo.lock': 'new-lock' } },
    { inputs: { ...options.inputs, 'Cargo.toml': 'new-manifest' } },
    { inputs: { ...options.inputs, 'scripts/ci/cloud-build.cjs': 'new-build-flags' } },
    { env: { ...env, RUNNER_OS: 'Linux' } }, { env: { ...env, RUNNER_ARCH: 'ARM64' } },
    { env: { ...env, ImageVersion: 'new-sdk-image' } }, { env: { ...env, RUSTFLAGS: '-C target-cpu=native' } },
    { platform: 'macos-aarch64' }, { mode: 'probe' },
  ];
  for (const variant of variants) assert.notEqual(cachePlan({ ...options, ...variant }).prefix, original.prefix);
  assert.equal(cachePlan({ ...options, inputs: { 'Cargo.toml': 'manifest-hash', 'Cargo.lock': 'lock-hash' } }).key, original.key);
  assert.throws(() => cachePlan({ ...options, platform: '../private' }), /invalid_selection/);
  assert.throws(() => cachePlan({ ...options, platform: 'windows', mode: 'probe' }), /invalid_probe_platform/);
});

test('cache paths contain only rustc incremental state, without duplicating dependencies or packages', () => {
  for (const platform of ['macos-x86_64', 'macos-aarch64', 'windows', 'linux-x86_64', 'linux-aarch64']) {
    const paths = cachePaths(platform);
    assert.equal(paths.length, 2);
    for (const item of paths) {
      assert.ok(item.startsWith('target/'));
      assert.equal(path.posix.basename(item), 'incremental');
      assert.doesNotMatch(item, /bundle|\.app|profile|\.cargo|credential|account|deps|fingerprint|build/);
    }
  }
  assert.throws(() => cachePaths('all'), /invalid_selection/);
});

test('measure reports incremental data, caps uploads, ignores out-of-scope files and rejects links', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-state-'));
  try {
    const write = (name, content) => {
      const file = path.join(root, name);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, content);
    };
    assert.equal(measureState(root, 'macos-x86_64').reason, 'incremental_missing');
    write('target/release/deps/dependency.rlib', 'abc');
    write('target/release/build/generated', 'exclude-build');
    write('target/release/.fingerprint/manifest', 'exclude-fingerprint');
    write('target/x86_64-apple-darwin/release/incremental/session/state', 'state');
    write('target/x86_64-apple-darwin/release/bundle/Cockpit.app/file', 'must-not-count');
    write('target/private/profile/auth.json', 'synthetic-do-not-cache');
    let result = measureState(root, 'macos-x86_64');
    assert.deepEqual(result, { bytes: 5, files: 1, symlinks: 0, incrementalBytes: 5, incrementalFiles: 1, save: true, reason: 'ready' });
    const inspected = inspectCompiledState(root, 'macos-x86_64');
    assert.equal(inspected.files, 4);
    assert.ok(inspected.bytes > result.bytes);
    assert.equal(inspected.incrementalBytes, result.incrementalBytes);
    assert.equal(MAX_BYTES, 6 * 1024 ** 3);
    result = measureState(root, 'macos-x86_64', 4);
    assert.equal(result.save, false);
    assert.equal(result.reason, 'size_limit');
    const incremental = path.join(root, 'target/x86_64-apple-darwin/release/incremental');
    fs.symlinkSync(path.join(root, 'target/private/profile/auth.json'), path.join(incremental, 'linked'));
    result = measureState(root, 'macos-x86_64');
    assert.equal(result.reason, 'unsupported_or_unreadable_path');
    assert.equal(result.blockedPath, 'target/x86_64-apple-darwin/release/incremental/linked');
    assert.equal(result.blockedType, 'symlink');
    assert.equal(result.blockedCode, 'ABSOLUTE_SYMLINK');
    assert.ok(!JSON.stringify(result).includes(root));
    fs.unlinkSync(path.join(incremental, 'linked'));
    fs.symlinkSync(path.join(root, 'target/private'), path.join(root, 'target/release/incremental'), 'dir');
    result = measureState(root, 'macos-x86_64');
    assert.equal(result.save, false);
    assert.equal(result.blockedPath, 'target/release/incremental');
    assert.equal(result.blockedType, 'symlink');
    assert.equal(result.blockedCode, 'UNSUPPORTED_ANCESTOR');
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test('incremental cache cannot include a dependency through a link to an uncached root', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-lean-link-'));
  try {
    const incremental = path.join(root, 'target/release/incremental');
    const deps = path.join(root, 'target/release/deps');
    fs.mkdirSync(incremental, { recursive: true });
    fs.mkdirSync(deps, { recursive: true });
    fs.writeFileSync(path.join(incremental, 'session.db'), 'state');
    fs.writeFileSync(path.join(deps, 'dependency.rlib'), 'artifact');
    const target = '../deps/dependency.rlib';
    fs.symlinkSync(target, path.join(incremental, 'alias'));
    const measured = measureState(root, 'macos-x86_64');
    assert.equal(measured.save, false);
    assert.equal(measured.blockedPath, 'target/release/incremental/alias');
    assert.equal(measured.blockedCode, 'EXTERNAL_SYMLINK');
    const inspected = inspectCompiledState(root, 'macos-x86_64');
    assert.equal(inspected.save, true);
    assert.equal(inspected.files, 2);
    assert.equal(inspected.symlinks, 1);
    assert.equal(inspected.bytes, 13 + Buffer.byteLength(target));
    assert.equal(inspected.incrementalBytes, 5);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test('relative Swift output aliases are preserved without traversing or counting their target twice', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-swift-link-'));
  try {
    const output = path.join(root, 'target/x86_64-apple-darwin/release/build/cockpit-tools/out/swift-rs/Menu');
    const target = 'x86_64-apple-macosx/release';
    fs.mkdirSync(path.join(output, target), { recursive: true });
    fs.writeFileSync(path.join(output, target, 'menu.a'), 'lib');
    fs.symlinkSync(target, path.join(output, 'release'), 'dir');
    const result = inspectCompiledState(root, 'macos-x86_64');
    assert.equal(result.reason, 'incremental_missing');
    assert.equal(result.symlinks, 1);
    assert.equal(result.files, 1);
    assert.equal(result.bytes, 3 + Buffer.byteLength(target));
    assert.equal(result.incrementalBytes, 0);
    assert.equal(result.save, false);
    fs.unlinkSync(path.join(output, 'release'));
    fs.symlinkSync(path.join(output, target), path.join(output, 'release'), 'dir');
    assert.equal(inspectCompiledState(root, 'macos-x86_64').blockedCode, 'ABSOLUTE_SYMLINK');
    assert.deepEqual(measureState(root, 'macos-x86_64'), {
      bytes: 0, files: 0, symlinks: 0, incrementalBytes: 0, incrementalFiles: 0,
      save: false, reason: 'incremental_missing',
    });
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test('relative links reject external targets, broken chains, cycles and links to ancestors', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-link-boundary-'));
  try {
    const deps = path.join(root, 'target/release/incremental');
    const outside = path.join(root, 'target/private');
    fs.mkdirSync(deps, { recursive: true });
    fs.mkdirSync(outside, { recursive: true });
    fs.writeFileSync(path.join(outside, 'private.txt'), 'not-read');
    const link = path.join(deps, 'alias');
    for (const [target, code] of [['../../private/private.txt', 'EXTERNAL_SYMLINK'],
      ['missing-target', 'ENOENT'], ['alias', 'ELOOP'], ['.', 'CYCLIC_SYMLINK']]) {
      fs.symlinkSync(target, link);
      const result = measureState(root, 'macos-x86_64');
      assert.equal(result.save, false);
      assert.equal(result.blockedPath, 'target/release/incremental/alias');
      assert.equal(result.blockedType, 'symlink');
      assert.equal(result.blockedCode, code);
      fs.unlinkSync(link);
    }
    fs.symlinkSync('second', link);
    fs.symlinkSync('alias', path.join(deps, 'second'));
    assert.equal(measureState(root, 'macos-x86_64').blockedCode, 'ELOOP');
    fs.unlinkSync(link);
    fs.unlinkSync(path.join(deps, 'second'));
    fs.writeFileSync(path.join(deps, 'real-file'), 'lib');
    fs.symlinkSync(path.join(deps, 'real-file'), path.join(outside, 'back-inside'));
    fs.symlinkSync('../../private/back-inside', link);
    assert.equal(measureState(root, 'macos-x86_64').blockedCode, 'EXTERNAL_SYMLINK');
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test('a link cannot authorize another cache root whose ancestor is itself a symlink', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-root-link-'));
  try {
    const deps = path.join(root, 'target/release/incremental');
    const outside = path.join(root, 'private');
    fs.mkdirSync(deps, { recursive: true });
    fs.mkdirSync(path.join(outside, 'release/incremental'), { recursive: true });
    fs.writeFileSync(path.join(outside, 'release/incremental/hidden'), 'do-not-cache');
    fs.symlinkSync(outside, path.join(root, 'target/x86_64-apple-darwin'), 'dir');
    fs.symlinkSync('../../x86_64-apple-darwin/release/incremental/hidden', path.join(deps, 'alias'));
    const result = measureState(root, 'macos-x86_64');
    assert.equal(result.blockedPath, 'target/x86_64-apple-darwin');
    assert.equal(result.blockedCode, 'UNSUPPORTED_ANCESTOR');
    assert.equal(result.files, 0);
    assert.equal(result.symlinks, 0);
    assert.equal(result.save, false);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test('cache diagnostics identify a non-directory ancestor without reading its contents', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-cache-diagnostic-'));
  try {
    fs.mkdirSync(path.join(root, 'target'));
    fs.writeFileSync(path.join(root, 'target/release'), 'synthetic-private-content');
    const result = measureState(root, 'macos-x86_64');
    assert.equal(result.save, false);
    assert.equal(result.blockedPath, 'target/release');
    assert.equal(result.blockedType, 'file');
    assert.equal(result.blockedCode, 'UNSUPPORTED_ANCESTOR');
    assert.ok(!JSON.stringify(result).includes('synthetic-private-content'));
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

const scope = namespace('macos-x86_64', 'full');
const entry = (id, sha, time, overrides = {}) => ({ id, ref: env.GITHUB_REF,
  key: `${scope}${'c'.repeat(32)}-${sha.repeat(40)}`, created_at: `2026-10-25T0${time}:00:00Z`, ...overrides });
const pruneOptions = { ref: env.GITHUB_REF, namespace: scope, sha: env.GITHUB_SHA };

test('pruning retains current latest state and cannot cross branch, platform, mode or cache family', () => {
  const current = entry(2, 'a', 2);
  const old = entry(1, 'b', 1);
  const ignored = [
    entry(3, 'b', 0, { ref: 'refs/heads/main' }),
    entry(4, 'b', 0, { key: old.key.replace('macos-x86_64', 'macos-aarch64') }),
    entry(5, 'b', 0, { key: old.key.replace('-full-', '-probe-') }),
    entry(6, 'b', 0, { key: 'host-release-dependencies' }),
    entry(7, 'b', 0, { key: `${scope}unexpected-key-shape` }),
    entry(8, 'b', 0, { created_at: 'invalid' }),
    entry(10, 'b', 0, { key: old.key.replace('ct-cloud-incremental-v1-', 'ct-cloud-state-v1-') }),
  ];
  assert.deepEqual(pruneCandidates([old, current, old, ...ignored], pruneOptions), { ids: [1], reason: 'keep_latest_one' });
  assert.deepEqual(pruneCandidates([old, ...ignored], pruneOptions), { ids: [], reason: 'current_cache_missing' });
  assert.deepEqual(pruneCandidates([old, current, entry(9, 'd', 3)], pruneOptions), { ids: [], reason: 'newer_cache_present' });
});

test('maintenance only deletes selected IDs; permissions/network failures leave validated builds intact', async () => {
  const calls = [];
  const fake = async (url, init) => {
    calls.push([url, init.method || 'GET']);
    return init.method === 'DELETE' ? { ok: true, status: 204 }
      : { ok: true, json: async () => ({ actions_caches: [entry(1, 'b', 1), entry(2, 'a', 2)] }) };
  };
  assert.deepEqual(await prune({ ...env, GH_TOKEN: 'synthetic' }, 'macos-x86_64', 'full', fake), { deleted: 1, reason: 'keep_latest_one' });
  assert.equal(calls.length, 2);
  assert.match(calls[0][0], /^https:\/\/api\.github\.com\/repos\/example\/cockpit-tools\/actions\/caches\?/);
  assert.equal(new URL(calls[0][0]).searchParams.get('ref'), env.GITHUB_REF);
  assert.equal(new URL(calls[0][0]).searchParams.get('key'), scope);
  assert.deepEqual(calls[1], ['https://api.github.com/repos/example/cockpit-tools/actions/caches/1', 'DELETE']);
  assert.deepEqual(await prune({ ...env, GH_TOKEN: 'synthetic' }, 'macos-x86_64', 'full', async () => ({ ok: false, status: 403 })), { deleted: 0, reason: 'list_http_403' });
  let contacted = false;
  await prune({ ...env, GITHUB_EVENT_NAME: 'pull_request', GH_TOKEN: 'synthetic' }, 'macos-x86_64', 'full', async () => { contacted = true; });
  assert.equal(contacted, false);
  assert.equal((await prune({ ...env, GH_TOKEN: 'synthetic', GITHUB_API_URL: 'https://foreign.invalid' }, 'macos-x86_64', 'full', fake)).reason, 'unsupported_api_origin');
});

test('local CLI invocations cannot query the compiler or perform cloud maintenance', () => {
  for (const command of ['prepare', 'measure', 'prune']) {
    const result = spawnSync(process.execPath, [path.join(__dirname, 'cloud-build-state-cache.cjs'), command, 'macos-x86_64', 'full'], {
      encoding: 'utf8', env: { ...process.env, GITHUB_ACTIONS: 'false' },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /GitHub Actions only/);
  }
});
