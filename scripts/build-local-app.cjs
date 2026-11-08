#!/usr/bin/env node
// Personal Native-only macOS build. Never installs or stops a running app.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawn, spawnSync } = require('node:child_process');

const REPO_ROOT = path.resolve(__dirname, '..');
const APP_NAME = 'Cockpit Tools.app';
const CACHE_NAMES = ['target', 'node_modules', 'dist'];

function hostTarget(platform = process.platform, arch = process.arch) {
  if (platform !== 'darwin') throw new Error('Local App builds require macOS; use build:app:cloud on other hosts.');
  if (arch === 'x64') return { target: 'x86_64-apple-darwin', architecture: 'x86_64' };
  if (arch === 'arm64') return { target: 'aarch64-apple-darwin', architecture: 'arm64' };
  throw new Error(`Unsupported macOS host architecture: ${arch}`);
}

function buildPlan({ root = REPO_ROOT, platform = process.platform, arch = process.arch } = {}) {
  const host = hostTarget(platform, arch);
  const version = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  return {
    version, ...host, engine: 'Rust Native', bundles: ['app'], signing: 'ad-hoc', frontendTypechecks: 1,
    output: path.join(root, '.local-app-output', APP_NAME),
    builtApp: path.join(root, 'target', host.target, 'release', 'bundle', 'macos', APP_NAME),
    steps: [
      'Check Node/npm, native Rust host and macOS SDK/Swift/signing tools',
      'npm ci --no-audit --no-fund',
      'npm run build (version sync, one TypeScript check, Vite)',
      `Tauri build --target ${host.target} --bundles app -- --locked (beforeBuildCommand disabled)`,
      'Verify version, thin Mach-O architectures, ad-hoc signature and absence of Go sidecars',
      'Publish the verified App, preserving the previous App if publication fails',
      'Remove only project target, node_modules, dist and this invocation scratch',
    ],
    cleanupAfterSuccess: CACHE_NAMES.map(name => path.join(root, name)),
    failurePolicy: 'Keep diagnostics and build caches; do not replace the previous verified App',
  };
}

function isInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative !== '' && !relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative);
}

function assertProjectDirectory(root, candidate) {
  const resolvedRoot = fs.realpathSync(root);
  if (resolvedRoot !== path.resolve(root) || !isInside(resolvedRoot, path.resolve(candidate))) {
    throw new Error(`Refusing a path outside the canonical project: ${candidate}`);
  }
  const relative = path.relative(resolvedRoot, candidate);
  let current = resolvedRoot;
  for (const part of relative.split(path.sep)) {
    current = path.join(current, part);
    let entry;
    try { entry = fs.lstatSync(current); } catch (error) { if (error.code === 'ENOENT') continue; throw error; }
    if (entry.isSymbolicLink() || !entry.isDirectory()) throw new Error(`Refusing a linked or non-directory build path: ${current}`);
  }
}

function cleanupTargets(root, scratch, env = process.env) {
  const scratchParent = path.join(root, '.tmp');
  if (path.dirname(scratch) !== scratchParent || !/^local-app-build-[A-Za-z0-9]+$/.test(path.basename(scratch))) {
    throw new Error('Cleanup requires the exact scratch directory created by this invocation.');
  }
  const targets = [...CACHE_NAMES.map(name => path.join(root, name)), scratch];
  // Validate the complete set before deleting anything. fs.rm does not follow
  // symlinks inside these generated directories; top-level aliases are refused.
  for (const target of targets) assertProjectDirectory(root, target);
  for (const key of ['CODEX_HOME', 'COCKPIT_TOOLS_DATA_DIR', 'COCKPIT_TOOLS_TEST_DATA_DIR', 'COCKPIT_TEST_DATA_DIR', 'CARGO_HOME', 'RUSTUP_HOME']) {
    if (!env[key]) continue;
    const protectedPath = path.resolve(env[key]);
    if (targets.some(target => protectedPath === target || isInside(target, protectedPath))) {
      throw new Error(`Refusing cleanup because ${key} points inside generated build output.`);
    }
  }
  return targets;
}

function cleanupAfterSuccess(root, scratch, env = process.env) {
  for (const target of cleanupTargets(root, scratch, env)) fs.rmSync(target, { recursive: true, force: true });
}

function capture(command, args, { root, env = process.env } = {}) {
  const result = spawnSync(command, args, { cwd: root, env, encoding: 'utf8', timeout: 120_000, maxBuffer: 2 * 1024 * 1024 });
  if (result.error || result.status !== 0) {
    const reason = result.error?.code || result.status;
    throw new Error(`${path.basename(command)} ${args[0] || ''} failed (${reason}). ${result.stderr || ''}`.trim());
  }
  return `${result.stdout || ''}${result.stderr || ''}`.trim();
}

function toolchainEnvironment(root, scratch, source = process.env) {
  const env = { ...source };
  const cargoBin = path.join(os.homedir(), '.cargo', 'bin');
  env.PATH = [cargoBin, source.PATH || ''].join(path.delimiter);
  // Toolchains and their shared download caches remain wherever the user keeps
  // them. Only compilation output and this invocation's temporary files move.
  env.CARGO_TARGET_DIR = path.join(root, 'target');
  env.ELECTRON_SKIP_BINARY_DOWNLOAD = '1';
  if (scratch) env.TMPDIR = `${path.join(scratch, 'tmp')}${path.sep}`;
  for (const key of Object.keys(env)) {
    if (key.startsWith('APPLE_') || key.startsWith('TAURI_SIGNING_')) delete env[key];
  }
  return env;
}

function preflight(root, plan, env) {
  const [major, minor] = process.versions.node.split('.').map(Number);
  if (!(major === 20 && minor >= 19 || major === 22 && minor >= 12 || major > 22)) {
    throw new Error('Vite requires Node 20.19+ or 22.12+; select a supported Node version before building.');
  }
  for (const filename of ['package-lock.json', 'Cargo.lock', 'src-tauri/tauri.conf.json']) {
    if (!fs.statSync(path.join(root, filename)).isFile()) throw new Error(`Missing required source input: ${filename}`);
  }
  const options = { root, env };
  const npm = capture('npm', ['--version'], options);
  const rust = capture('rustc', ['-vV'], options);
  const actualHost = rust.match(/^host: (.+)$/m)?.[1];
  if (actualHost !== plan.target) throw new Error(`Rust host ${actualHost || 'unknown'} does not match this Mac build target ${plan.target}. Select its native Rust toolchain.`);
  const cargo = capture('cargo', ['--version'], options);
  const developer = capture('xcode-select', ['-p'], options);
  const sdk = capture('xcrun', ['--sdk', 'macosx', '--show-sdk-path'], options);
  capture('xcrun', ['--find', 'clang'], options);
  const swift = capture('xcrun', ['swift', '--version'], options);
  const swiftVersion = swift.match(/Swift version (\d+)\.(\d+)/i);
  if (!swiftVersion || Number(swiftVersion[1]) < 5 || Number(swiftVersion[1]) === 5 && Number(swiftVersion[2]) < 9) {
    throw new Error('The native menu package requires Swift 5.9 or newer. Select a compatible Xcode installation.');
  }
  for (const tool of ['/usr/bin/codesign', '/usr/bin/lipo', '/usr/bin/ditto', '/usr/libexec/PlistBuddy']) {
    fs.accessSync(tool, fs.constants.X_OK);
  }
  return { node: process.versions.node, npm, rustHost: actualHost, cargo, developer, sdk, swift: swift.split('\n')[0] };
}

function run(command, args, { root, env, log }) {
  console.log(`\n[Local App] ${path.basename(command)} ${args.join(' ')}`);
  fs.appendFileSync(log, `\n$ ${path.basename(command)} ${args.join(' ')}\n`);
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', data => { process.stdout.write(data); fs.appendFileSync(log, data); });
    child.stderr.on('data', data => { process.stderr.write(data); fs.appendFileSync(log, data); });
    child.once('error', reject);
    child.once('close', (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${path.basename(command)} failed (${signal || code}); see ${log}`));
    });
  });
}

function appFiles(app) {
  const files = [];
  function visit(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      // CLIProxyAPI license text remains a legal resource, not a Go executable.
      if (/^(?:cockpit[-_]cliproxy.*|cliproxy(?:api)?(?:\.exe)?)$/i.test(entry.name)) {
        throw new Error(`Unexpected Go sidecar in Native app: ${file}`);
      }
      if (entry.isSymbolicLink()) {
        const resolved = fs.realpathSync(file);
        if (resolved !== app && !isInside(app, resolved)) throw new Error(`App contains an external link: ${file}`);
      } else if (entry.isDirectory()) visit(file);
      else if (entry.isFile()) files.push(file);
      else throw new Error(`App contains a non-regular entry: ${file}`);
    }
  }
  visit(app);
  return files;
}

function isMachO(file) {
  const fd = fs.openSync(file, 'r');
  try {
    const bytes = Buffer.alloc(4);
    if (fs.readSync(fd, bytes, 0, 4, 0) !== 4) return false;
    return ['feedface', 'feedfacf', 'cefaedfe', 'cffaedfe', 'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca'].includes(bytes.toString('hex'));
  } finally { fs.closeSync(fd); }
}

function verifyApp(app, plan, options) {
  const plist = path.join(app, 'Contents', 'Info.plist');
  const version = capture('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleShortVersionString', plist], options);
  if (version !== plan.version) throw new Error(`App version mismatch: expected ${plan.version}, got ${version}`);
  const executable = capture('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleExecutable', plist], options);
  if (!executable || executable !== path.basename(executable)) throw new Error('Invalid bundle executable path.');
  const main = path.join(app, 'Contents', 'MacOS', executable);
  const binaries = appFiles(app).filter(isMachO);
  if (!binaries.includes(main)) throw new Error('Bundle main executable is missing or is not Mach-O.');
  for (const binary of binaries) {
    const architectures = capture('/usr/bin/lipo', ['-archs', binary], options).split(/\s+/);
    if (architectures.length !== 1 || architectures[0] !== plan.architecture) throw new Error(`Unexpected architecture in ${binary}: ${architectures.join(', ')}`);
  }
  capture('/usr/bin/codesign', ['--verify', '--deep', '--strict', '--verbose=2', app], options);
  const signature = capture('/usr/bin/codesign', ['--display', '--verbose=4', app], options);
  if (!/^Signature=adhoc$/m.test(signature)) throw new Error('Expected an ad-hoc signed personal App.');
  return { version, architecture: plan.architecture, machOBinaries: binaries.length, signing: 'ad-hoc', goSidecar: false };
}

function publishApp(staged, destination, scratch, verify) {
  const previous = path.join(scratch, 'previous-successful.app');
  let movedOld = false;
  let movedNew = false;
  try {
    if (fs.existsSync(destination)) { fs.renameSync(destination, previous); movedOld = true; }
    fs.renameSync(staged, destination);
    movedNew = true;
    verify(destination);
  } catch (error) {
    // The previous verified result is never deleted until its replacement works.
    if (movedNew) fs.renameSync(destination, staged);
    if (movedOld) fs.renameSync(previous, destination);
    throw error;
  }
}

async function main(argv = process.argv.slice(2)) {
  if (argv.some(arg => !['--dry-run', '--help'].includes(arg))) throw new Error('Usage: build-local-app.cjs [--dry-run]');
  if (argv.includes('--help')) { console.log('Usage: node scripts/build-local-app.cjs [--dry-run]\nBuild this Mac\'s Native-only .app; retain only the verified App after success.'); return; }
  const root = fs.realpathSync(REPO_ROOT);
  const plan = buildPlan({ root });
  if (argv.includes('--dry-run')) { console.log(JSON.stringify(plan, null, 2)); return plan; }
  for (const directory of [...CACHE_NAMES, '.tmp', '.local-app-output']) assertProjectDirectory(root, path.join(root, directory));
  fs.mkdirSync(path.join(root, '.tmp'), { recursive: true });
  const scratch = fs.mkdtempSync(path.join(root, '.tmp', 'local-app-build-'));
  fs.chmodSync(scratch, 0o700);
  fs.mkdirSync(path.join(scratch, 'tmp'), { mode: 0o700 });
  const log = path.join(scratch, 'build.log');
  fs.writeFileSync(log, '', { mode: 0o600 });
  const env = toolchainEnvironment(root, scratch);
  const options = { root, env, log };
  let published = false;
  try {
    // Establish cleanup ownership before spending time on dependency/build work.
    cleanupTargets(root, scratch, env);
    const tools = preflight(root, plan, env);
    fs.writeFileSync(path.join(scratch, 'plan.json'), JSON.stringify({ ...plan, tools }, null, 2), { mode: 0o600 });
    console.log(`[Local App] ${plan.version}, ${plan.target}; prerequisites ready.`);
    await run('npm', ['ci', '--no-audit', '--no-fund'], options);
    await run('npm', ['run', 'build'], options);
    const config = path.join(scratch, 'tauri.conf.json');
    fs.writeFileSync(config, JSON.stringify({
      build: { beforeBuildCommand: '' },
      bundle: { targets: ['app'], externalBin: [], createUpdaterArtifacts: false, macOS: { signingIdentity: '-' } },
    }, null, 2));
    await run(process.execPath, [path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), 'build', '--ci', '--config', config, '--target', plan.target, '--bundles', 'app', '--', '--locked'], options);
    verifyApp(plan.builtApp, plan, options);
    const staged = path.join(scratch, APP_NAME);
    await run('/usr/bin/ditto', [plan.builtApp, staged], options);
    verifyApp(staged, plan, options);
    const outputDirectory = path.dirname(plan.output);
    assertProjectDirectory(root, outputDirectory);
    fs.mkdirSync(outputDirectory, { recursive: true });
    assertProjectDirectory(root, plan.output);
    publishApp(staged, plan.output, scratch, app => verifyApp(app, plan, options));
    published = true;
    cleanupAfterSuccess(root, scratch, env);
    console.log(`\n[Local App] Verified ${plan.version} (${plan.architecture}, ad-hoc): ${plan.output}\nProject build caches removed. The App was not installed or launched.`);
    return plan;
  } catch (error) {
    if (fs.existsSync(log)) fs.appendFileSync(log, `\nFAILED: ${error.message}\n`);
    console.error(published
      ? `[Local App] App verified at ${plan.output}; cleanup did not finish. Remaining diagnostics: ${scratch}`
      : `[Local App] Failed; project build caches and diagnostics retained: ${scratch}`);
    throw error;
  }
}

if (require.main === module) main().catch(error => { console.error(error.message); process.exitCode = 1; });
module.exports = { hostTarget, buildPlan, cleanupTargets, cleanupAfterSuccess, assertProjectDirectory, toolchainEnvironment, appFiles, isMachO, verifyApp, publishApp, main };
