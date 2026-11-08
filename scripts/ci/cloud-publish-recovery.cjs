#!/usr/bin/env node
// Reuse an already verified Cloud Build when only its publication failed.
const fs = require('node:fs');
const { execFileSync } = require('node:child_process');

const PUBLISH_ONLY_FILES = new Set([
  '.github/workflows/publish-cloud-build.yml',
  'scripts/ci/cloud-publish.cjs',
  'scripts/ci/cloud-publish.test.cjs',
  'scripts/ci/cloud-publish-recovery.cjs',
  'scripts/ci/cloud-publish-recovery.test.cjs',
  'docs/cloud-builds.md',
]);

function validateSourceRun(run, jobs, changedFiles, repository) {
  if (run.head_repository?.full_name !== repository || run.head_branch !== 'main'
      || !['push', 'workflow_dispatch'].includes(run.event)
      || run.path !== '.github/workflows/build-matrix.yml') {
    throw new Error('Require a trusted main Cloud Build from this repository');
  }
  if (run.status !== 'completed' || !['success', 'failure'].includes(run.conclusion)
      || !/^[a-f\d]{40}$/i.test(run.head_sha || '')) {
    throw new Error('Source build must be completed with a valid commit');
  }
  for (const name of ['plan', 'frontend', 'Native contracts']) {
    const matches = jobs.filter(job => job.name === name);
    if (matches.length !== 1 || matches[0].conclusion !== 'success') {
      throw new Error(`Required verification did not pass: ${name}`);
    }
  }
  const builds = jobs.filter(job => job.name.startsWith('Build ') && job.conclusion !== 'skipped');
  if (!builds.length) throw new Error('No completed application build');
  for (const job of builds) {
    if (job.conclusion !== 'success') throw new Error(`Application build failed: ${job.name}`);
    const requiredSteps = ['Build selected application', 'Archive application and write provenance'];
    if (job.name.startsWith('Build macos-')) requiredSteps.push('Native gateway capability, chat bridge and Credits regressions');
    for (const name of requiredSteps) {
      if (!job.steps?.some(step => step.name === name && step.conclusion === 'success')) {
        throw new Error(`Build lacks successful ${name}`);
      }
    }
  }
  const maintenanceJobs = new Set(['Replace rolling development downloads', 'Retain current branch compilation state']);
  for (const job of jobs) {
    if (!['success', 'skipped'].includes(job.conclusion) && !maintenanceJobs.has(job.name)) {
      throw new Error(`Source workflow has another unsuccessful job: ${job.name}`);
    }
  }
  for (const file of changedFiles) {
    if (!PUBLISH_ONLY_FILES.has(file) && !/^docs\/local-patches\/[^/]+\.md$/.test(file)) {
      throw new Error(`Application/build inputs changed after verification: ${file}`);
    }
  }
  return { commit: run.head_sha, runId: String(run.id) };
}

function command(program, args) {
  return execFileSync(program, args, { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 }).trim();
}

function verifiedSource(runId, repository) {
  if (!/^[1-9]\d*$/.test(runId || '') || !/^[\w.-]+\/[\w.-]+$/.test(repository || '')) {
    throw new Error('Require a numeric source run ID and repository');
  }
  const run = JSON.parse(command('gh', ['api', `repos/${repository}/actions/runs/${runId}`,
    '--jq', '{id,path,event,head_branch,head_sha,status,conclusion,head_repository:{full_name:.head_repository.full_name}}']));
  if (String(run.id) !== runId || !/^[a-f\d]{40}$/i.test(run.head_sha || '')) throw new Error('Source run identity mismatch');
  const page = JSON.parse(command('gh', ['api', `repos/${repository}/actions/runs/${runId}/jobs?per_page=100`]));
  if (!Array.isArray(page.jobs) || page.jobs.length !== page.total_count) throw new Error('Incomplete source job listing');
  // The recovery workflow checks out trusted main with full history. It may only
  // add publication tooling/docs, never silently reuse a binary for changed code.
  command('git', ['merge-base', '--is-ancestor', run.head_sha, 'HEAD']);
  const changedFiles = command('git', ['diff', '--name-only', run.head_sha, 'HEAD']).split(/\r?\n/).filter(Boolean);
  return validateSourceRun(run, page.jobs, changedFiles, repository);
}

function main() {
  if (process.env.GITHUB_ACTIONS !== 'true') throw new Error('Publication recovery runs only in GitHub Actions');
  const [mode, runId] = process.argv.slice(2);
  if (!['verify', 'publish'].includes(mode)) throw new Error('Usage: cloud-publish-recovery.cjs verify|publish RUN_ID');
  const source = verifiedSource(runId, process.env.GITHUB_REPOSITORY);
  console.log(`Verified build ${source.runId} at ${source.commit}; application inputs unchanged.`);
  if (mode === 'verify' && process.env.GITHUB_OUTPUT) {
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `source_sha=${source.commit}\nsource_run_id=${source.runId}\n`);
  }
  if (mode === 'publish') require('./cloud-publish.cjs').main(source);
}

if (require.main === module) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = { validateSourceRun, verifiedSource };
