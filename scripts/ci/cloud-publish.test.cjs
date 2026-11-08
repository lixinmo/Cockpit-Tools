const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const publisher = path.join(__dirname, 'cloud-publish.cjs');
const repository = 'example/cockpit-tools';
const sourceCommit = 'a'.repeat(40);
const recoveryCommit = 'b'.repeat(40);
const sourceRunId = '42462535936';
const recoveryRunId = '42470000000';

// A real subprocess runs the production publisher, but every gh invocation is
// intercepted by this executable. Unknown calls fail rather than reaching GitHub.
const fakeGh = `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
const fixture = JSON.parse(process.env.FAKE_GH_FIXTURE);
const entry = { args };
let output = '';
let error = '';
let code = 0;
if (args[0] === 'release' && args[1] === 'view') {
  if (fixture.missing) { error = 'release not found'; code = 1; }
  else output = JSON.stringify({ isPrerelease: fixture.isPrerelease !== false, isDraft: Boolean(fixture.isDraft), assets: [{ name: 'obsolete.zip' }] });
} else if (args[0] === 'api' && args[1].includes('/git/ref/tags/')) {
  output = JSON.stringify({ object: { sha: 'c'.repeat(40) } });
} else if (args[0] === 'api' && args[1].includes('/compare/')) {
  const response = JSON.stringify({ status: fixture.comparison || 'ahead', files: Array.from({length: 1000}, (_, i) => ({ filename: 'file-' + i, patch: 'x'.repeat(4096) })) });
  entry.rawBytes = Buffer.byteLength(response);
  const jq = args.indexOf('--jq');
  output = jq >= 0 && args[jq + 1] === '.status' ? (fixture.comparison || 'ahead') : response;
} else if (args[0] === 'api' && args[1].includes('/actions/runs/') && args[1].endsWith('/artifacts')) {
  output = JSON.stringify({ artifacts: [{ id: 101, name: 'cockpit-tools-macos-x86_64' }, { id: 102, name: 'cloud-state-fixture' }, { id: 103, name: 'unrelated-report' }] });
} else if (args[0] === 'api' && args.includes('--method') && ['PATCH', 'DELETE'].includes(args[args.indexOf('--method') + 1])) {
  output = '{}';
} else if (args[0] === 'release' && ['create', 'upload', 'edit', 'delete-asset'].includes(args[1])) {
  output = '{}';
} else { error = 'Unexpected fake gh call: ' + JSON.stringify(args); code = 2; }
fs.appendFileSync(process.env.FAKE_GH_LOG, JSON.stringify(entry) + '\\n');
if (output) process.stdout.write(output + '\\n');
if (error) process.stderr.write(error + '\\n');
process.exitCode = code;
`;

function setup(t, fixture = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cockpit-publish-fixture-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bin = path.join(root, 'bin');
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, 'gh'), fakeGh, { mode: 0o755 });
  const directory = path.join(root, 'downloaded-builds', 'cockpit-tools-macos-x86_64');
  fs.mkdirSync(directory, { recursive: true });
  const provenance = {
    repository, commit: sourceCommit, target: 'x86_64-apple-darwin', version: '1.3.80',
    workflowRun: `https://github.com/${repository}/actions/runs/${sourceRunId}`,
  };
  fs.writeFileSync(path.join(directory, 'provenance.json'), JSON.stringify(provenance));
  fs.writeFileSync(path.join(directory, 'Cockpit.Tools.app.tar.gz'), 'synthetic app bytes; never executed');
  const manifest = fs.readdirSync(directory).map(name => `${crypto.createHash('sha256').update(fs.readFileSync(path.join(directory, name))).digest('hex')}  ${name}`).join('\n');
  fs.writeFileSync(path.join(directory, 'SHA256SUMS.txt'), `${manifest}\n`);
  const log = path.join(root, 'fake-gh.jsonl');
  return {
    root, directory,
    calls() { return fs.existsSync(log) ? fs.readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line)) : []; },
    run(options = {}, env = {}) {
      return spawnSync(process.execPath, ['-e', 'try { require(process.argv[1]).main(JSON.parse(process.argv[2])); } catch (error) { console.error(error.message); process.exitCode = 1; }', publisher, JSON.stringify(options)], {
        cwd: root, encoding: 'utf8', timeout: 15_000,
        env: {
          ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}`,
          GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: repository,
          GITHUB_SHA: sourceCommit, GITHUB_RUN_ID: sourceRunId, GITHUB_STEP_SUMMARY: '',
          FAKE_GH_LOG: log, FAKE_GH_FIXTURE: JSON.stringify(fixture), ...env,
        },
      });
    },
  };
}

function assertSuccess(result) {
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
}
function releaseMutations(calls) {
  return calls.filter(({ args }) => (args[0] === 'release' && args[1] !== 'view') || args.includes('PATCH'));
}

test('large compare responses are projected inside gh before reaching the publisher pipe', t => {
  const f = setup(t);
  assertSuccess(f.run());
  const calls = f.calls();
  const compare = calls.find(({ args }) => args[1]?.includes('/compare/'));
  assert.ok(compare.rawBytes > 2 * 1024 * 1024, 'fixture exceeds execFileSync default maxBuffer');
  assert.deepEqual(compare.args.slice(-2), ['--jq', '.status']);
  assert.ok(calls.some(({ args }) => args[0] === 'release' && args[1] === 'upload'));
  assert.ok(calls.some(({ args }) => args.includes(`sha=${sourceCommit}`)));
});

test('behind keeps a newer rolling download without moving its tag or uploading old assets', t => {
  const f = setup(t, { comparison: 'behind' });
  const result = f.run();
  assertSuccess(result);
  assert.match(result.stdout, /Retaining newer development build/);
  assert.deepEqual(releaseMutations(f.calls()), []);
  assert.ok(f.calls().some(({ args }) => args[1] === `repos/${repository}/actions/runs/${sourceRunId}/artifacts`));
});

for (const comparison of ['diverged', 'unrelated', 'unknown']) {
  test(`comparison ${comparison} refuses publication and artifact cleanup`, t => {
    const f = setup(t, { comparison });
    const result = f.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Rolling release source diverged/);
    assert.deepEqual(releaseMutations(f.calls()), []);
    assert.ok(!f.calls().some(({ args }) => args[1]?.includes('/actions/')));
  });
}

test('recovery publishes verified source commit and only cleans artifacts from the source run', t => {
  const f = setup(t, { comparison: 'identical' });
  assertSuccess(f.run({ commit: sourceCommit, runId: sourceRunId }, { GITHUB_SHA: recoveryCommit, GITHUB_RUN_ID: recoveryRunId }));
  const calls = f.calls();
  assert.ok(calls.some(({ args }) => args[1] === `repos/${repository}/actions/runs/${sourceRunId}/artifacts`));
  assert.ok(!calls.some(({ args }) => args.some(arg => arg.includes(recoveryRunId) || arg.includes(recoveryCommit))));
  assert.deepEqual(calls.filter(({ args }) => args.includes('DELETE')).map(({ args }) => args[1]), [
    `repos/${repository}/actions/artifacts/101`, `repos/${repository}/actions/artifacts/102`,
  ]);
  const notes = fs.readFileSync(path.join(f.root, 'downloaded-builds', 'macos-x86_64-notes.md'), 'utf8');
  assert.ok(notes.includes(`/commit/${sourceCommit}`));
  assert.ok(notes.includes(`/actions/runs/${sourceRunId}`));
});

test('normal environment defaults can still create a rolling prerelease with the build commit', t => {
  const f = setup(t, { missing: true });
  assertSuccess(f.run());
  const create = f.calls().find(({ args }) => args[0] === 'release' && args[1] === 'create').args;
  assert.equal(create[create.indexOf('--target') + 1], sourceCommit);
  assert.ok(create.includes('--prerelease'));
  assert.ok(create.includes('--latest=false'));
});

for (const fixture of [{ isPrerelease: false }, { isDraft: true }]) {
  test(`non-development release stays untouched (${JSON.stringify(fixture)})`, t => {
    const f = setup(t, fixture);
    const result = f.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Refusing to replace non-development release/);
    assert.deepEqual(releaseMutations(f.calls()), []);
  });
}

test('explicit recovery parameters cannot bypass Actions, identity or checksum verification', async t => {
  for (const { name, options, env, tamper, expected } of [
    { name: 'local execution', options: { commit: sourceCommit, runId: sourceRunId }, env: { GITHUB_ACTIONS: 'false' }, expected: /only in GitHub Actions/ },
    { name: 'invalid commit', options: { commit: 'not-a-commit', runId: sourceRunId }, expected: /repository or commit/ },
    { name: 'invalid run', options: { commit: sourceCommit, runId: '../other' }, expected: /source run ID/ },
    { name: 'missing run', env: { GITHUB_RUN_ID: '' }, expected: /source run ID/ },
    { name: 'wrong source', options: { commit: recoveryCommit, runId: sourceRunId }, expected: /Build identity mismatch/ },
    { name: 'tampered artifact', tamper: true, expected: /Checksum mismatch/ },
  ]) {
    await t.test(name, st => {
      const f = setup(st);
      if (tamper) fs.appendFileSync(path.join(f.directory, 'Cockpit.Tools.app.tar.gz'), 'changed');
      const result = f.run(options, env);
      assert.equal(result.status, 1);
      assert.match(result.stderr, expected);
      assert.deepEqual(f.calls(), [], 'verification must finish before any gh call');
    });
  }
});
