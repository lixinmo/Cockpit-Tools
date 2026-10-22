const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const crypto = require('node:crypto');
const test = require('node:test');
const { buildPlan, platforms, tauriArgs, releaseDirectory } = require('./cloud-build.cjs');
const { verifyBuild } = require('./cloud-publish.cjs');

test('default selects only this Mac architecture on its native standard runner', () => {
  const plan = buildPlan();
  assert.equal(plan.universal, false);
  assert.deepEqual(plan.matrix.include.map(p => [p.target, p.runner, p.native_mac]), [['x86_64-apple-darwin', 'macos-15-intel', '1']]);
});

test('each platform can be selected independently and all retains every architecture', () => {
  for (const platform of platforms) assert.deepEqual(buildPlan(platform.label).matrix.include, [platform]);
  assert.deepEqual(buildPlan('all').matrix.include, platforms);
  assert.equal(buildPlan('all').universal, true);
  assert.throws(() => buildPlan('self-hosted'), /Unsupported platform/);
});

test('Universal selects exactly the two thin inputs for the merge job', () => {
  const plan = buildPlan('macos-universal');
  assert.equal(plan.universal, true);
  assert.deepEqual(plan.matrix.include.map(p => p.target).sort(), ['aarch64-apple-darwin', 'x86_64-apple-darwin']);
});

test('cloud builds share formal release Cargo output layouts', () => {
  for (const platform of platforms) {
    const args = tauriArgs('build', platform.target, 'example.json');
    if (platform.native_mac === '1') {
      assert.equal(args[args.indexOf('--target') + 1], platform.target);
      assert.ok(releaseDirectory(platform.target).endsWith(path.join('target', platform.target, 'release')));
    } else {
      assert.ok(!args.includes('--target'));
      assert.ok(releaseDirectory(platform.target).endsWith(path.join('target', 'release')));
    }
    assert.deepEqual(args.slice(-2), ['--', '--locked']);
  }
});

test('the cloud command refuses accidental local compilation', () => {
  const result = spawnSync(process.execPath, [path.join(__dirname, 'cloud-build.cjs'), 'build'], { encoding: 'utf8', env: { ...process.env, GITHUB_ACTIONS: 'false' } });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /GitHub Actions only/);
});

test('publication rejects wrong provenance, tampered assets and unlisted files', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'cockpit-cloud-publish-'));
  try {
    const expected = { repository: 'example/cockpit-tools', commit: 'a'.repeat(40), target: 'x86_64-apple-darwin' };
    fs.writeFileSync(path.join(directory, 'provenance.json'), JSON.stringify(expected));
    fs.writeFileSync(path.join(directory, 'example.app.tar.gz'), 'synthetic archive');
    const manifest = ['provenance.json', 'example.app.tar.gz'].map(name => `${crypto.createHash('sha256').update(fs.readFileSync(path.join(directory, name))).digest('hex')}  ${name}\n`).join('');
    fs.writeFileSync(path.join(directory, 'SHA256SUMS.txt'), manifest);
    assert.equal(verifyBuild(directory, expected).names.length, 3);
    assert.throws(() => verifyBuild(directory, { ...expected, commit: 'b'.repeat(40) }), /identity mismatch/);
    fs.writeFileSync(path.join(directory, 'unlisted.txt'), 'not covered');
    assert.throws(() => verifyBuild(directory, expected), /coverage is incomplete/);
    fs.unlinkSync(path.join(directory, 'unlisted.txt'));
    fs.appendFileSync(path.join(directory, 'example.app.tar.gz'), 'tampered');
    assert.throws(() => verifyBuild(directory, expected), /Checksum mismatch/);
  } finally { fs.rmSync(directory, { recursive: true, force: true }); }
});
