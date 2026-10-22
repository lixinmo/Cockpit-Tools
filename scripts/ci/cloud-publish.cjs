#!/usr/bin/env node
// One replaceable prerelease per platform; never accumulate development versions.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync, spawnSync } = require('node:child_process');
const { platforms } = require('./cloud-build.cjs');

function gh(args) {
  return execFileSync('gh', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function verifyBuild(directory, expected) {
  const provenance = JSON.parse(fs.readFileSync(path.join(directory, 'provenance.json')));
  if (provenance.repository !== expected.repository || provenance.commit !== expected.commit || provenance.target !== expected.target) {
    throw new Error(`Build identity mismatch: ${directory}`);
  }
  const checksums = fs.readFileSync(path.join(directory, 'SHA256SUMS.txt'), 'utf8').trim().split('\n');
  const checked = new Set();
  for (const line of checksums) {
    const match = /^([a-f\d]{64})  ([^/\\]+)$/.exec(line);
    if (!match || ['.', '..'].includes(match[2]) || checked.has(match[2])) throw new Error('Invalid checksum manifest');
    const file = path.join(directory, match[2]);
    if (!fs.lstatSync(file).isFile() || fs.lstatSync(file).isSymbolicLink()) throw new Error('Artifact must be a regular file');
    const actual = crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
    if (actual !== match[1]) throw new Error(`Checksum mismatch: ${match[2]}`);
    checked.add(match[2]);
  }
  const names = fs.readdirSync(directory).sort();
  if (!checked.has('provenance.json') || names.some(name => name !== 'SHA256SUMS.txt' && !checked.has(name))) throw new Error('Artifact checksum coverage is incomplete');
  return { provenance, names };
}

function main() {
  if (process.env.GITHUB_ACTIONS !== 'true') throw new Error('Publishing runs only in GitHub Actions');
  const repository = process.env.GITHUB_REPOSITORY;
  const commit = process.env.GITHUB_SHA;
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository || '') || !/^[a-f\d]{40}$/i.test(commit || '')) throw new Error('Missing GitHub repository or commit');
  const targets = [...platforms, { label: 'macos-universal', target: 'universal-apple-darwin' }];
  const downloads = path.resolve('downloaded-builds');
  const artifacts = fs.readdirSync(downloads, { withFileTypes: true }).filter(e => e.isDirectory());
  if (!artifacts.length) throw new Error('No build artifacts found');
  for (const artifact of artifacts) {
    const platform = targets.find(p => artifact.name === `cockpit-tools-${p.label}`);
    if (!platform) throw new Error(`Unexpected artifact: ${artifact.name}`);
    const directory = path.join(downloads, artifact.name);
    const { provenance, names } = verifyBuild(directory, { repository, commit, target: platform.target });
    const tag = `cloud-${platform.label}`;
    const existing = spawnSync('gh', ['release', 'view', tag, '--repo', repository, '--json', 'isPrerelease,isDraft,assets'], { encoding: 'utf8' });
    let previous;
    if (existing.status === 0) {
      previous = JSON.parse(existing.stdout);
      if (!previous.isPrerelease || previous.isDraft) throw new Error(`Refusing to replace non-development release ${tag}`);
      const ref = JSON.parse(gh(['api', `repos/${repository}/git/ref/tags/${tag}`]));
      const comparison = JSON.parse(gh(['api', `repos/${repository}/compare/${ref.object.sha}...${commit}`]));
      if (comparison.status === 'behind') {
        console.log(`Retaining newer development build: ${tag}`);
        continue;
      }
      if (!['ahead', 'identical'].includes(comparison.status)) throw new Error(`Rolling release source diverged: ${tag}`);
    } else if (!/release not found|not found|HTTP 404/i.test(existing.stderr || '')) {
      throw new Error(existing.stderr || 'Could not inspect the rolling release');
    }
    const note = path.join(directory, '..', `${platform.label}-notes.md`);
    fs.writeFileSync(note, `# Development build: ${platform.label}\n\nVersion: ${provenance.version}\n\nSource: [${commit}](https://github.com/${repository}/commit/${commit})\n\n[Cloud build and verification](${provenance.workflowRun})\n\nThis rolling prerelease keeps only the latest verified build for this platform. SHA256SUMS.txt and provenance.json accompany the download. Native retry, Go reference retry, Credits policy, Auto Reset and frontend checks passed in this run.\n\nMac applications are ad-hoc signed, without Apple notarization. Development builds do not include signed automatic updater packages.\n`);
    if (!previous) {
      gh(['release', 'create', tag, '--repo', repository, '--target', commit, '--title', `Cloud development · ${platform.label}`, '--notes-file', note, '--prerelease', '--latest=false']);
    }
    gh(['release', 'upload', tag, '--repo', repository, '--clobber', ...names.map(name => path.join(directory, name))]);
    for (const asset of previous?.assets || []) {
      if (!names.includes(asset.name)) gh(['release', 'delete-asset', tag, asset.name, '--repo', repository, '--yes']);
    }
    if (previous) {
      gh(['api', `repos/${repository}/git/refs/tags/${tag}`, '--method', 'PATCH', '-f', `sha=${commit}`, '-F', 'force=true']);
      gh(['release', 'edit', tag, '--repo', repository, '--title', `Cloud development · ${platform.label}`, '--notes-file', note, '--prerelease', '--latest=false']);
    }
    const url = `https://github.com/${repository}/releases/tag/${tag}`;
    console.log(url);
    if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `- [Download ${platform.label}](${url}) — ${provenance.version}, commit \`${commit}\`\n`);
  }
  // Once the durable public downloads exist, remove this run's temporary copies.
  const transient = JSON.parse(gh(['api', `repos/${repository}/actions/runs/${process.env.GITHUB_RUN_ID}/artifacts`])).artifacts;
  for (const artifact of transient) {
    if (artifact.name.startsWith('cockpit-tools-') || artifact.name.startsWith('cloud-')) {
      gh(['api', `repos/${repository}/actions/artifacts/${artifact.id}`, '--method', 'DELETE']);
    }
  }
}

if (require.main === module) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = { verifyBuild };
