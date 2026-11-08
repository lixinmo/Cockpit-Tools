const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const { hostTarget, buildPlan, cleanupTargets, cleanupAfterSuccess, toolchainEnvironment, appFiles, publishApp } = require('./build-local-app.cjs');

function fixture(t) {
  const parent = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'cockpit-local-app-test-')));
  t.after(() => fs.rmSync(parent, { recursive: true, force: true }));
  const root = path.join(parent, 'project');
  fs.mkdirSync(root);
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ version: '1.2.3' }));
  return { parent, root };
}
function file(root, relative, content = 'preserve') {
  const target = path.join(root, relative);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, content);
  return target;
}

test('selects exactly the native thin Mac architecture and refuses other hosts', () => {
  assert.deepEqual(hostTarget('darwin', 'x64'), { target: 'x86_64-apple-darwin', architecture: 'x86_64' });
  assert.deepEqual(hostTarget('darwin', 'arm64'), { target: 'aarch64-apple-darwin', architecture: 'arm64' });
  assert.throws(() => hostTarget('linux', 'x64'), /require macOS/);
  assert.throws(() => hostTarget('win32', 'x64'), /require macOS/);
  assert.throws(() => hostTarget('darwin', 'universal'), /Unsupported/);
});

test('plan has one frontend check, only an App bundle and a project-local output', t => {
  const { root } = fixture(t);
  for (const arch of ['x64', 'arm64']) {
    const plan = buildPlan({ root, platform: 'darwin', arch });
    assert.equal(plan.version, '1.2.3');
    assert.equal(plan.frontendTypechecks, 1);
    assert.deepEqual(plan.bundles, ['app']);
    assert.equal(plan.engine, 'Rust Native');
    assert.equal(plan.signing, 'ad-hoc');
    assert.equal(plan.output, path.join(root, '.local-app-output', 'Cockpit Tools.app'));
    assert.equal(plan.steps.filter(step => step.startsWith('npm run build')).length, 1);
    assert.ok(plan.steps.some(step => step.includes('beforeBuildCommand disabled')));
    assert.deepEqual(plan.cleanupAfterSuccess, ['target', 'node_modules', 'dist'].map(name => path.join(root, name)));
    assert.ok(!JSON.stringify(plan).includes('/Applications'));
  }
});

test('dry-run creates no directories and needs no npm, Rust or Xcode subprocess', t => {
  const { root } = fixture(t);
  const script = file(root, 'scripts/build-local-app.cjs', fs.readFileSync(path.join(__dirname, 'build-local-app.cjs')));
  const before = fs.readdirSync(root).sort();
  const result = spawnSync(process.execPath, [script, '--dry-run'], { cwd: root, encoding: 'utf8', env: { ...process.env, PATH: '/no-build-tools' } });
  assert.equal(result.error, undefined);
  if (process.platform === 'darwin') {
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).version, '1.2.3');
  } else {
    assert.equal(result.status, 1);
    assert.match(result.stderr, /require macOS/);
  }
  assert.deepEqual(fs.readdirSync(root).sort(), before);
  assert.ok(!fs.existsSync(path.join(root, '.tmp')));
  assert.ok(!fs.existsSync(path.join(root, '.local-app-output')));
});

test('success cleanup removes only generated caches and this invocation scratch', t => {
  const { parent, root } = fixture(t);
  for (const name of ['target/cache', 'node_modules/dependency', 'dist/frontend', '.tmp/local-app-build-ABC123/log']) file(root, name);
  const retained = ['.git/config', 'src/main.rs', 'config.json', '.codex/auth.json', '.antigravity_cockpit/accounts.json',
    '.local-app-output/Cockpit Tools.app/Contents/MacOS/app', '.tmp/local-app-build-OLD456/diagnostics'];
  for (const name of retained) file(root, name);
  const shared = file(parent, 'shared-toolchain/keep');
  fs.symlinkSync(path.dirname(shared), path.join(root, 'target', 'shared-link'), 'dir');
  const scratch = path.join(root, '.tmp', 'local-app-build-ABC123');
  assert.equal(cleanupTargets(root, scratch).length, 4);
  cleanupAfterSuccess(root, scratch);
  for (const name of ['target', 'node_modules', 'dist', '.tmp/local-app-build-ABC123']) assert.ok(!fs.existsSync(path.join(root, name)), name);
  for (const name of retained) assert.equal(fs.readFileSync(path.join(root, name), 'utf8'), 'preserve', name);
  assert.equal(fs.readFileSync(shared, 'utf8'), 'preserve');
});

test('a top-level cache alias refuses the whole cleanup before deleting anything', t => {
  const { parent, root } = fixture(t);
  const shared = file(parent, 'shared-cargo/registry');
  const keep = file(root, 'target/cache');
  fs.symlinkSync(path.dirname(shared), path.join(root, 'node_modules'), 'dir');
  const scratch = path.join(root, '.tmp', 'local-app-build-ABC123');
  file(root, '.tmp/local-app-build-ABC123/log');
  assert.throws(() => cleanupAfterSuccess(root, scratch), /linked or non-directory/);
  assert.ok(fs.existsSync(keep));
  assert.ok(fs.existsSync(scratch));
  assert.ok(fs.existsSync(shared));
});

test('cleanup rejects ancestors, source paths, output App and unrelated temporary directories', t => {
  const { parent, root } = fixture(t);
  const retained = file(root, 'target/cache');
  for (const invalid of [root, parent, path.join(root, 'src'), path.join(root, '.local-app-output'),
    path.join(parent, 'local-app-build-ABC123'), path.join(root, '.tmp', 'cloud-build')]) {
    assert.throws(() => cleanupAfterSuccess(root, invalid), /exact scratch directory/);
    assert.ok(fs.existsSync(retained));
  }
});

test('cleanup refuses runtime homes or shared toolchains placed under generated output', t => {
  const { root } = fixture(t);
  const retained = file(root, 'target/cache');
  const scratch = path.join(root, '.tmp/local-app-build-ABC123');
  file(root, '.tmp/local-app-build-ABC123/log');
  for (const key of ['CODEX_HOME', 'COCKPIT_TOOLS_DATA_DIR', 'CARGO_HOME', 'RUSTUP_HOME']) {
    assert.throws(() => cleanupAfterSuccess(root, scratch, { [key]: path.join(root, 'target/protected') }), /points inside generated/);
    assert.ok(fs.existsSync(retained));
  }
});

test('build environment preserves shared toolchains but confines Cargo output and removes signing secrets', t => {
  const { root } = fixture(t);
  const scratch = path.join(root, '.tmp', 'local-app-build-ABC123');
  const original = { PATH: '/tools/bin', HOME: '/user', CARGO_HOME: '/shared/cargo', RUSTUP_HOME: '/shared/rustup',
    CARGO_TARGET_DIR: '/other-project/target', APPLE_ID: 'not-a-real-account', APPLE_SIGNING_IDENTITY: 'private',
    TAURI_SIGNING_PRIVATE_KEY: 'not-a-real-key' };
  const env = toolchainEnvironment(root, scratch, original);
  assert.equal(env.CARGO_TARGET_DIR, path.join(root, 'target'));
  assert.equal(env.CARGO_HOME, original.CARGO_HOME);
  assert.equal(env.RUSTUP_HOME, original.RUSTUP_HOME);
  assert.equal(env.HOME, original.HOME);
  assert.equal(env.TMPDIR, `${path.join(scratch, 'tmp')}${path.sep}`);
  assert.equal(env.ELECTRON_SKIP_BINARY_DOWNLOAD, '1');
  for (const key of ['APPLE_ID', 'APPLE_SIGNING_IDENTITY', 'TAURI_SIGNING_PRIVATE_KEY']) assert.ok(!(key in env));
  assert.equal(original.CARGO_TARGET_DIR, '/other-project/target');
});

test('Native App inventory permits license resources but rejects Go sidecars and external links', t => {
  const { parent, root } = fixture(t);
  const app = path.join(root, 'test.app');
  const license = file(app, 'Contents/Resources/licenses/CLIProxyAPI.txt');
  assert.deepEqual(appFiles(app), [license]);
  const sidecar = file(app, 'Contents/MacOS/cockpit-cliproxy-x86_64-apple-darwin');
  assert.throws(() => appFiles(app), /Unexpected Go sidecar/);
  fs.unlinkSync(sidecar);
  const shared = file(parent, 'shared-runtime/data');
  fs.symlinkSync(shared, path.join(app, 'Contents/Resources/external'));
  assert.throws(() => appFiles(app), /external link/);
});

test('failed final verification restores the previous successful App and retains the failed candidate', t => {
  const { root } = fixture(t);
  const scratch = path.join(root, '.tmp/local-app-build-ABC123');
  const staged = path.join(scratch, 'Cockpit Tools.app');
  const destination = path.join(root, '.local-app-output/Cockpit Tools.app');
  file(staged, 'app', 'candidate');
  file(destination, 'app', 'old-success');
  assert.throws(() => publishApp(staged, destination, scratch, () => { throw new Error('verification failed'); }), /verification failed/);
  assert.equal(fs.readFileSync(path.join(destination, 'app'), 'utf8'), 'old-success');
  assert.equal(fs.readFileSync(path.join(staged, 'app'), 'utf8'), 'candidate');
});

test('successful publication retains old output until the new output has passed verification', t => {
  const { root } = fixture(t);
  const scratch = path.join(root, '.tmp/local-app-build-ABC123');
  const staged = path.join(scratch, 'Cockpit Tools.app');
  const destination = path.join(root, '.local-app-output/Cockpit Tools.app');
  file(staged, 'app', 'new-success');
  file(destination, 'app', 'old-success');
  publishApp(staged, destination, scratch, final => {
    assert.equal(fs.readFileSync(path.join(final, 'app'), 'utf8'), 'new-success');
    assert.equal(fs.readFileSync(path.join(scratch, 'previous-successful.app/app'), 'utf8'), 'old-success');
  });
  assert.equal(fs.readFileSync(path.join(destination, 'app'), 'utf8'), 'new-success');
  cleanupAfterSuccess(root, scratch);
  assert.ok(!fs.existsSync(scratch));
  assert.equal(fs.readFileSync(path.join(destination, 'app'), 'utf8'), 'new-success');
});
