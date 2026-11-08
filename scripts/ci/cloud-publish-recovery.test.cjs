const assert = require('node:assert/strict');
const test = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const { validateSourceRun } = require('./cloud-publish-recovery.cjs');

test('recovery and normal publishing share one concurrency lock', () => {
  const root = path.resolve(__dirname, '../..');
  const recovery = fs.readFileSync(path.join(root, '.github/workflows/publish-cloud-build.yml'), 'utf8');
  const build = fs.readFileSync(path.join(root, '.github/workflows/build-matrix.yml'), 'utf8');
  assert.match(recovery, /concurrency:\s+group: cloud-development-publication\s+cancel-in-progress: false/);
  assert.equal((recovery.match(/group: cloud-development-publication/g) || []).length, 1);
  assert.match(build.slice(build.indexOf('  publish-development:')), /group: cloud-development-publication/);
});

function fixture() {
  const repository = 'example/cockpit';
  const run = { id: 1234, head_repository: { full_name: repository }, head_branch: 'main',
    event: 'push', path: '.github/workflows/build-matrix.yml', status: 'completed',
    conclusion: 'failure', head_sha: 'a'.repeat(40) };
  const jobs = ['plan', 'frontend', 'Native contracts'].map(name => ({ name, conclusion: 'success' }));
  jobs.push({ name: 'Build macos-x86_64', conclusion: 'success', steps: [
    'Build selected application', 'Archive application and write provenance',
    'Native gateway capability, chat bridge and Credits regressions',
  ].map(name => ({ name, conclusion: 'success' })) });
  jobs.push({ name: 'Bundle macOS Universal without recompiling', conclusion: 'skipped' });
  jobs.push({ name: 'Replace rolling development downloads', conclusion: 'failure' });
  return { run, jobs, repository };
}

test('recovers a publication failure only after every required build check passed', () => {
  const f = fixture();
  assert.deepEqual(validateSourceRun(f.run, f.jobs, ['scripts/ci/cloud-publish.cjs'], f.repository),
    { commit: 'a'.repeat(40), runId: '1234' });
});

test('rejects another repository, PR event, branch or workflow', () => {
  for (const patch of [{ head_repository: { full_name: 'other/fork' } }, { event: 'pull_request' },
    { head_branch: 'feature' }, { path: '.github/workflows/unrelated.yml' }]) {
    const f = fixture();
    assert.throws(() => validateSourceRun({ ...f.run, ...patch }, f.jobs, [], f.repository), /trusted main/);
  }
});

test('cannot use canceled, unfinished or unverified builds', () => {
  for (const patch of [{ status: 'in_progress' }, { conclusion: 'cancelled' }, { head_sha: 'invalid' }]) {
    const f = fixture();
    assert.throws(() => validateSourceRun({ ...f.run, ...patch }, f.jobs, [], f.repository), /completed/);
  }
  for (const name of ['plan', 'frontend', 'Native contracts', 'Build macos-x86_64']) {
    const f = fixture(); f.jobs.find(job => job.name === name).conclusion = 'failure';
    assert.throws(() => validateSourceRun(f.run, f.jobs, [], f.repository), /did not pass|build failed/);
  }
});

test('a probe-only job or skipped Native regressions cannot authorize an App publication', () => {
  for (const name of ['Build selected application', 'Archive application and write provenance',
    'Native gateway capability, chat bridge and Credits regressions']) {
    const f = fixture(); f.jobs[3].steps.find(step => step.name === name).conclusion = 'skipped';
    assert.throws(() => validateSourceRun(f.run, f.jobs, [], f.repository), /lacks successful/);
  }
});

test('rejects unverified application, dependency and compilation changes', () => {
  for (const file of ['src/App.tsx', 'src-tauri/src/lib.rs', 'Cargo.lock', 'package.json',
    '.github/workflows/build-matrix.yml', 'scripts/ci/cloud-build.cjs', 'sidecars/cockpit-cliproxy/main.go']) {
    const f = fixture();
    assert.throws(() => validateSourceRun(f.run, f.jobs, [file], f.repository), /inputs changed/);
  }
});

test('allows only publication tooling and maintenance documentation after the verified commit', () => {
  const f = fixture();
  assert.doesNotThrow(() => validateSourceRun(f.run, f.jobs, [
    '.github/workflows/publish-cloud-build.yml', 'scripts/ci/cloud-publish-recovery.cjs',
    'scripts/ci/cloud-publish-recovery.test.cjs', 'docs/local-patches/upstream-sync-v1.3.80.md',
  ], f.repository));
  f.jobs[4].conclusion = 'failure';
  assert.throws(() => validateSourceRun(f.run, f.jobs, [], f.repository), /unsuccessful job/);
});
