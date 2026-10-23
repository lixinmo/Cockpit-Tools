const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const crypto = require('node:crypto');
const test = require('node:test');
const { buildPlan, platforms, tauriArgs, nativeRegressionArgs, nativeProbeExecutable, releaseDirectory } = require('./cloud-build.cjs');
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
  for (const mode of ['build', 'native-regressions', 'package-native-probe', 'build-native-probe']) {
    const result = spawnSync(process.execPath, [path.join(__dirname, 'cloud-build.cjs'), mode], { encoding: 'utf8', env: { ...process.env, GITHUB_ACTIONS: 'false' } });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /GitHub Actions only/);
  }
});

test('standalone Native acceptance cannot produce an untested application release', () => {
  for (const platform of ['macos-x86_64', 'macos-aarch64']) {
    assert.equal(buildPlan(platform, true).matrix.include.length, 1);
  }
  for (const platform of ['all', 'macos-universal', 'windows', 'linux-x86_64']) {
    assert.throws(() => buildPlan(platform, true), /one thin macOS/);
  }
  const workflow = fs.readFileSync(path.join(__dirname, '../../.github/workflows/build-matrix.yml'), 'utf8');
  for (const title of ['Build selected application', 'Archive application and write provenance']) {
    assert.ok(workflow.includes(`- name: ${title}\n        if: inputs.native_probe_only != true`));
  }
  assert.match(workflow.slice(workflow.indexOf('  publish-development:')), /inputs\.native_probe_only != true/);
  assert.match(workflow, /group: cloud-build-.*inputs\.native_probe_only/);
  const release = fs.readFileSync(path.join(__dirname, '../../.github/workflows/release.yml'), 'utf8');
  for (const step of ['Build selected application', 'Archive application and write provenance']) {
    assert.ok(release.includes(`.name == "${step}" and .conclusion == "success"`));
  }
  assert.ok(workflow.indexOf('run: node scripts/ci/cloud-build.cjs build-native-probe') < workflow.indexOf('name: native-gateway-probe-${{ matrix.label }}'));
});

test('native probe packaging selects Cargo evidence instead of cached executable names', () => {
  const artifact = executable => JSON.stringify({ reason: 'compiler-artifact', target: { name: 'antigravity_cockpit_tools_lib' }, profile: { test: true }, executable });
  const dependency = JSON.stringify({ reason: 'compiler-artifact', target: { name: 'serde' }, profile: { test: false }, executable: '/not-a-test' });
  assert.equal(nativeProbeExecutable(`${dependency}\n${artifact('/current/test')}\n`), '/current/test');
  assert.throws(() => nativeProbeExecutable(dependency), /exactly one/);
  assert.throws(() => nativeProbeExecutable(`${artifact('/old/test')}\n${artifact('/new/test')}`), /exactly one/);
});

test('native gateway regressions reuse the thin application target and isolate synthetic fixtures', () => {
  for (const platform of platforms.filter(p => p.native_mac === '1')) {
    const args = nativeRegressionArgs(platform.target);
    assert.deepEqual(args.slice(0, 8), [
      'test', '--locked', '--release', '--target', platform.target,
      '-p', 'cockpit-tools', '--lib',
    ]);
    assert.deepEqual(args.slice(8, 11), [
      '--config', 'profile.release.package.cockpit-tools.opt-level=0', '--',
    ]);
    assert.ok(!tauriArgs('build', platform.target, 'example.json').some(arg => arg.includes('opt-level')));
    assert.ok(args.includes('--test-threads=1'));
    assert.ok(!args.includes('--ignored') && !args.includes('--include-ignored'));
    for (const name of ['capabilities', 'capability_fallback_http_tests', 'mapping::capability::diagnostics_tests', 'chat_upstream', 'native_chat', 'native_capability_tests', 'native_capability_integration_tests', 'native_catalog_tools_tests', 'native_provider_profile_selection_tests', 'native_provider_wire_projection_', 'api_key_edit_', 'full_key_edit_migrates_persisted_references', 'editing_current_api_key_account_rewrites_relay_key_and_base_url', 'provider_snapshot_sync_updates_account', 'chat_completions_api_key_bundle_defers_catalog', 'passive_native_takeover_preserves_chat_tool_negotiation', 'parity22_http_responses_chat', 'native_provider_live_', 'native_credits', 'native_admission', 'native_gateway_credits']) {
      assert.ok(args.some(arg => arg.includes(name)), `Missing ${name} coverage`);
    }
  }
  assert.throws(() => nativeRegressionArgs('universal-apple-darwin'), /thin macOS/);
  assert.throws(() => nativeRegressionArgs('x86_64-unknown-linux-gnu'), /thin macOS/);
});

test('native gateway regressions gate packaging after the application has built its sidecar', () => {
  const workflow = fs.readFileSync(path.join(__dirname, '../../.github/workflows/build-matrix.yml'), 'utf8');
  const build = workflow.indexOf('run: node scripts/ci/cloud-build.cjs build');
  const regressions = workflow.indexOf('run: node scripts/ci/cloud-build.cjs native-regressions');
  const universal = workflow.indexOf('run: node scripts/ci/cloud-build.cjs stage-universal');
  const archive = workflow.indexOf('run: node scripts/ci/cloud-build.cjs package');
  assert.ok(build >= 0 && regressions > build && universal > regressions && archive > regressions);
  const step = workflow.slice(workflow.lastIndexOf('      - name:', regressions), regressions);
  assert.match(step, /if: runner\.os == 'macOS'/);
  assert.match(step, /CLOUD_BUILD_TARGET: \$\{\{ matrix\.target \}\}/);
  assert.match(step, /COCKPIT_NATIVE_APP_BUILD: '1'/);
  assert.match(step, /COCKPIT_SKIP_CLIPROXY_BUILD: '1'/);
  assert.match(step, /inputs\.native_probe != true/);
  const probeStart = workflow.indexOf('      - name: Package optional local Native acceptance driver');
  const probeEnd = workflow.indexOf('      - name:', probeStart + 10);
  assert.match(workflow.slice(probeStart, probeEnd), /run: node scripts\/ci\/cloud-build\.cjs build-native-probe/);
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
