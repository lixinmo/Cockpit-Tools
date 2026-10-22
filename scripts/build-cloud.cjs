const { spawnSync } = require('node:child_process');
const path = require('node:path');

const repoRoot = path.resolve(__dirname, '..');
const workflow = 'build-matrix.yml';
const platforms = [
  'macos-x86_64', 'macos-aarch64', 'macos-universal',
  'linux-x86_64', 'linux-aarch64', 'windows', 'all',
];

function hostPlatform() {
  return {
    'darwin:x64': 'macos-x86_64', 'darwin:arm64': 'macos-aarch64',
    'linux:x64': 'linux-x86_64', 'linux:arm64': 'linux-aarch64',
    'win32:x64': 'windows',
  }[`${process.platform}:${process.arch}`];
}

function help() {
  console.log(`Dispatch a GitHub Actions build; no local dependencies or compiler required.

Usage: node scripts/build-cloud.cjs [options]
       npm run build:app -- [options]

  --platform <name>  ${platforms.join(', ')}
                     Default: this computer's platform (${hostPlatform() || 'specify explicitly'}).
  --ref <ref>         Remote branch or tag to build (default: main).
  --repo <owner/repo> GitHub repository (default: this checkout's origin).
  --dry-run          Print the plan without starting a build.
  --help             Show this help.

Builds use the remote ref, so commit and push changes first.
A build-relevant push to main already starts the default Intel Mac build.
Use --platform all for every supported platform. Downloads are linked on the run.
The legacy build:app:local command dispatches this same cloud workflow.`);
}

function parseArgs(args) {
  const options = { platform: hostPlatform(), ref: 'main', dryRun: false };
  const seen = new Set();
  for (let index = 0; index < args.length; index += 1) {
    const [flag, ...inline] = args[index].split('=');
    if (seen.has(flag)) throw new Error(`Duplicate option: ${flag}`);
    seen.add(flag);
    if (flag === '--help' && !inline.length) options.help = true;
    else if (flag === '--dry-run' && !inline.length) options.dryRun = true;
    else if (['--platform', '--ref', '--repo'].includes(flag)) {
      const value = inline.length ? inline.join('=') : args[++index];
      if (!value || value.startsWith('-') || /[\r\n\0]/.test(value)) {
        throw new Error(`A value is required for ${flag}.`);
      }
      options[flag.slice(2)] = value;
    } else {
      throw new Error(`Unknown option: ${flag}. Use --help. Local compilation and --full-opt are no longer supported by this entry point.`);
    }
  }
  return options;
}

function originRepository() {
  const result = spawnSync('git', ['remote', 'get-url', 'origin'], {
    cwd: repoRoot, encoding: 'utf8',
  });
  if (result.error || result.status !== 0) {
    throw new Error('Cannot read origin. Add your GitHub origin remote or pass --repo owner/repo.');
  }
  // Never print remote URLs: an HTTPS remote can contain credentials.
  const remote = result.stdout.trim();
  let pathname;
  const ssh = remote.match(/^git@github\.com:(.+)$/i);
  if (ssh) pathname = ssh[1];
  else {
    try {
      const url = new URL(remote);
      if (!['https:', 'ssh:'].includes(url.protocol) || url.hostname.toLowerCase() !== 'github.com') {
        throw new Error('Unsupported remote');
      }
      pathname = url.pathname.replace(/^\//, '');
    } catch {
      throw new Error('Origin must point to github.com. Otherwise pass --repo owner/repo.');
    }
  }
  return pathname.replace(/\/$/, '').replace(/\.git$/, '');
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) return help();
  if (!platforms.includes(options.platform)) {
    throw new Error(`Select --platform ${platforms.join('|')}.`);
  }
  const repository = options.repo || originRepository();
  if (!/^[A-Za-z0-9][A-Za-z0-9-]*\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error('Repository must use the owner/repo format.');
  }
  const commandArgs = [
    'workflow', 'run', workflow, '--repo', repository,
    '--ref', options.ref, '-f', `platform=${options.platform}`,
  ];
  const actionsUrl = `https://github.com/${repository}/actions/workflows/${workflow}`;
  if (options.dryRun) {
    console.log(JSON.stringify({
      repository, ref: options.ref, platform: options.platform,
      command: ['gh', ...commandArgs], actionsUrl, localCompilation: false,
    }, null, 2));
    return;
  }
  console.log(`Starting ${options.platform} on GitHub Actions: ${repository}@${options.ref}`);
  const result = spawnSync('gh', commandArgs, { cwd: repoRoot, stdio: 'inherit' });
  if (result.error?.code === 'ENOENT') {
    throw new Error(`Install GitHub CLI and run gh auth login, or use Run workflow at ${actionsUrl}`);
  }
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.exitCode = result.status || 1;
    return;
  }
  console.log(`Build status and artifacts: ${actionsUrl}`);
  if (options.platform === 'all') {
    console.log(`Platform downloads: https://github.com/${repository}/releases (cloud-<platform>)`);
  } else {
    console.log(`Latest main-branch download: https://github.com/${repository}/releases/tag/cloud-${options.platform}`);
  }
}

try {
  main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
