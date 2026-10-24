#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { stageInputs, mergeInputs } = require('../release/macos-universal.cjs');
const { stageReleaseAssets } = require('../release/stage_release_assets.cjs');

const root = path.resolve(__dirname, '../..');
const platforms = [
  { label: 'macos-x86_64', cache_target: 'macos-x86_64', runner: 'macos-15-intel', target: 'x86_64-apple-darwin', native_mac: '1' },
  { label: 'macos-aarch64', cache_target: 'macos-aarch64', runner: 'macos-15', target: 'aarch64-apple-darwin', native_mac: '1' },
  { label: 'windows', cache_target: 'windows', runner: 'windows-2022', target: 'x86_64-pc-windows-msvc', native_mac: '0' },
  { label: 'linux-x86_64', cache_target: 'linux-x86_64', runner: 'ubuntu-22.04', target: 'x86_64-unknown-linux-gnu', native_mac: '0' },
  { label: 'linux-aarch64', cache_target: 'linux-aarch64', runner: 'ubuntu-24.04-arm', target: 'aarch64-unknown-linux-gnu', native_mac: '0' },
];

function buildPlan(selection = 'macos-x86_64', probeOnly = false) {
  if (probeOnly && !['macos-x86_64', 'macos-aarch64'].includes(selection)) {
    throw new Error('Standalone Native acceptance requires one thin macOS platform');
  }
  const universal = ['all', 'macos-universal'].includes(selection);
  const include = platforms.filter(p => selection === 'all' || p.label === selection || (selection === 'macos-universal' && p.native_mac === '1'));
  if (!include.length) throw new Error(`Unsupported platform: ${selection}`);
  return { matrix: { include }, universal };
}

function run(command, args) {
  execFileSync(command, args, { cwd: root, stdio: 'inherit' });
}

function target() {
  const value = process.env.CLOUD_BUILD_TARGET;
  if (![...platforms.map(p => p.target), 'universal-apple-darwin'].includes(value)) throw new Error('CLOUD_BUILD_TARGET must be a supported Rust target');
  return value;
}

function version() { return JSON.parse(fs.readFileSync(path.join(root, 'package.json'))).version; }

function config() {
  const file = path.join(root, '.tmp/cloud-build/tauri.conf.json');
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify({
    build: { beforeBuildCommand: '' },
    bundle: { createUpdaterArtifacts: false, macOS: { signingIdentity: '-' } },
  }, null, 2));
  return file;
}

function tauriArgs(mode, selectedTarget, configFile) {
  const args = [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), mode, '--ci', '--config', configFile];
  // Match formal release target layouts so both workflows reuse the same cache.
  if (selectedTarget.endsWith('-apple-darwin')) args.push('--target', selectedTarget, '--bundles', 'app');
  if (mode === 'build') args.push('--', '--locked');
  return args;
}

function nativeRegressionArgs(selectedTarget) {
  if (!platforms.some(p => p.target === selectedTarget && p.native_mac === '1')) {
    throw new Error('Native gateway regressions require a thin macOS build target');
  }
  // Reuse release dependencies but avoid optimizing this large test crate again.
  // This invocation-only package override does not change the application build.
  // libtest accepts multiple OR filters. Serialize fixtures sharing runtime state;
  // real-account acceptance tests are neither selected nor explicitly unignored.
  return [
    'test', '--locked', '--release', '--target', selectedTarget,
    '-p', 'cockpit-tools', '--lib',
    '--config', 'profile.release.package.cockpit-tools.opt-level=0',
    '--', '--test-threads=1',
    'modules::codex_native_api::capabilities::',
    'modules::codex_native_api::capability_fallback_http_tests::',
    'modules::codex_native_api::mapping::capability::diagnostics_tests::',
    'modules::codex_native_api::chat_upstream::',
    'native_chat_upstream',
    'chat_legacy_reasoning',
    'provider_metadata',
    'native_provider_reasoning',
    'manual_reasoning',
    'manual_provider_reasoning',
    'reasoning_order_',
    'modules::codex_native_api::native_chat::',
    'modules::codex_local_access::native_capability_tests::',
    'modules::codex_local_access::native_capability_integration_tests::',
    'modules::codex_local_access::native_catalog_tools_tests::',
    'modules::codex_local_access::native_provider_routes_tests::',
    'native_catalog_metadata_tests',
    'provider_model_bindings_tests',
    'native_provider_profile_selection_tests',
    'native_provider_wire_projection_',
    'api_key_edit_',
    'full_key_edit_migrates_persisted_references',
    'provider_snapshot_sync_updates_account',
    'editing_current_api_key_account_rewrites_relay_key_and_base_url',
    'chat_completions_api_key_bundle_defers_catalog',
    'provider_gateway_model_slots',
    'modules::codex_protocol::tests::',
    'chat_client_discovery',
    'chat_tool_integrity',
    'modules::codex_native_api::websocket::bridge_tests::tool_integrity_tests::',
    'chatgpt_plan_tools',
    'modules::codex_native_api::chatgpt_plan_http_tests::tool_discovery::',
    'grok_tool_search',
    'modules::codex_native_api::grok_wire::tool_search::tests::',
    'passive_native_takeover_preserves_chat_tool_negotiation',
    'parity22_http_responses_chat_restores_namespaced_custom_and_keeps_native_custom',
    'native_provider_live_',
    'native_credits', 'native_admission', 'native_gateway_credits',
  ];
}

function tauri(mode, selectedTarget) {
  if (mode === 'build') {
    const rust = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
    if (rust.match(/^host: (.+)$/m)?.[1] !== selectedTarget) throw new Error('Thin cloud builds require the selected native runner architecture');
  }
  run(process.execPath, tauriArgs(mode, selectedTarget, config()));
}

function releaseDirectory(selectedTarget) {
  return selectedTarget.endsWith('-apple-darwin') ? path.join(root, 'target', selectedTarget, 'release') : path.join(root, 'target/release');
}

function identity() {
  return { repoRoot: root, version: version(), commit: process.env.GITHUB_SHA, runId: process.env.GITHUB_RUN_ID };
}

function nativeProbeExecutable(output) {
  const executables = new Set(output.split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line))
    .filter(item => item.reason === 'compiler-artifact' && item.profile?.test === true
      && item.target?.name === 'antigravity_cockpit_tools_lib' && item.executable)
    .map(item => item.executable));
  if (executables.size !== 1) throw new Error('Expected exactly one compiled Native test executable');
  return [...executables][0];
}

function packageNativeProbe(selectedTarget, validate = false) {
  const args = nativeRegressionArgs(selectedTarget);
  // Ask Cargo for this build's exact executable, never choose a stale cached file.
  args.splice(args.indexOf('--'));
  args.push('--no-run', '--message-format=json');
  const output = execFileSync('cargo', args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  const executable = nativeProbeExecutable(output);
  const outputDir = path.join(root, 'native-probe-assets');
  fs.mkdirSync(outputDir, { recursive: true });
  const file = path.join(outputDir, 'native-gateway-probe');
  fs.copyFileSync(executable, file);
  fs.chmodSync(file, 0o755);
  const architectures = execFileSync('lipo', ['-archs', file], { encoding: 'utf8' }).trim().split(/\s+/);
  const expected = selectedTarget.startsWith('aarch64') ? 'arm64' : 'x86_64';
  if (architectures.length !== 1 || architectures[0] !== expected) throw new Error('Native probe architecture mismatch');
  run('codesign', ['--force', '--sign', '-', file]);
  run('codesign', ['--verify', '--strict', file]);
  if (validate) {
    const testArgs = nativeRegressionArgs(selectedTarget);
    run(file, testArgs.slice(testArgs.indexOf('--') + 1));
  }
  fs.writeFileSync(path.join(outputDir, 'provenance.json'), `${JSON.stringify({
    schema: 1, repository: process.env.GITHUB_REPOSITORY, commit: process.env.GITHUB_SHA,
    target: selectedTarget, architectures, artifact: 'native-gateway-probe',
    workflowRun: `https://github.com/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`,
  }, null, 2)}\n`);
  fs.writeFileSync(path.join(outputDir, 'SHA256SUMS.txt'), ['native-gateway-probe', 'provenance.json']
    .map(name => `${sha256(path.join(outputDir, name))}  ${name}\n`).join(''));
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function packageOutputs(selectedTarget) {
  const releaseDir = releaseDirectory(selectedTarget);
  const outputDir = path.join(root, 'cloud-assets');
  fs.rmSync(outputDir, { recursive: true, force: true });
  fs.mkdirSync(outputDir, { recursive: true });
  const isMac = selectedTarget.endsWith('-apple-darwin');
  const binaries = {};
  if (isMac) {
    const appName = 'Cockpit Tools.app';
    const app = path.join(releaseDir, 'bundle/macos', appName);
    const expected = selectedTarget === 'universal-apple-darwin' ? ['arm64', 'x86_64'] : [selectedTarget.startsWith('aarch64') ? 'arm64' : 'x86_64'];
    for (const name of ['cockpit-tools', 'cockpit-cliproxy']) {
      const file = path.join(app, 'Contents/MacOS', name);
      const archs = execFileSync('lipo', ['-archs', file], { encoding: 'utf8' }).trim().split(/\s+/).sort();
      if (JSON.stringify(archs) !== JSON.stringify(expected)) throw new Error(`Unexpected architecture in ${name}: ${archs}`);
      binaries[name] = { sha256: sha256(file), architectures: archs };
    }
    run('codesign', ['--verify', '--deep', '--strict', app]);
    run('tar', ['-czf', path.join(outputDir, `Cockpit.Tools_${version()}_${selectedTarget}.app.tar.gz`), '-C', path.dirname(app), appName]);
  } else {
    stageReleaseAssets({ platform: selectedTarget.includes('windows') ? 'windows' : 'linux', assetsDir: path.join(releaseDir, 'bundle'), outputDir });
    if (selectedTarget.includes('windows')) {
      run(process.execPath, ['scripts/release/build_windows_portable.cjs', '--version', version(), '--release-dir', releaseDir, '--output-dir', outputDir]);
    }
  }
  const artifacts = {};
  for (const name of fs.readdirSync(outputDir).sort()) artifacts[name] = sha256(path.join(outputDir, name));
  const provenance = {
    schema: 1, version: version(), target: selectedTarget,
    repository: process.env.GITHUB_REPOSITORY, commit: process.env.GITHUB_SHA,
    workflowRun: `https://github.com/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`,
    runnerOS: process.env.RUNNER_OS, runnerArchitecture: process.env.RUNNER_ARCH,
    createdAt: new Date().toISOString(), updaterSigned: false, binaries, artifacts,
  };
  fs.writeFileSync(path.join(outputDir, 'provenance.json'), `${JSON.stringify(provenance, null, 2)}\n`);
  const all = [...Object.entries(artifacts), ['provenance.json', sha256(path.join(outputDir, 'provenance.json'))]];
  fs.writeFileSync(path.join(outputDir, 'SHA256SUMS.txt'), all.map(([name, hash]) => `${hash}  ${name}\n`).join(''));
  if (process.env.GITHUB_STEP_SUMMARY) {
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `### ${selectedTarget}\n\nBuilt commit \`${process.env.GITHUB_SHA}\`. The **cockpit-tools-${platforms.find(p => p.target === selectedTarget)?.label || 'macos-universal'}** artifact contains the application, SHA-256 checksums and provenance.\n\nMac applications use ad-hoc signing; these development builds do not include signed updater packages. Artifacts expire after 1 day; successful default-branch builds are moved to rolling development Releases.\n`);
  }
}

function main(argv) {
  const [mode, selection] = argv;
  if (mode === 'plan') {
    const result = buildPlan(selection, process.env.CLOUD_NATIVE_PROBE_ONLY === 'true');
    if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `matrix=${JSON.stringify(result.matrix)}\nuniversal=${result.universal}\n`);
    console.log(JSON.stringify(result, null, 2));
    return;
  }
  if (process.env.GITHUB_ACTIONS !== 'true') throw new Error('Application compilation and packaging run in GitHub Actions only.');
  const selectedTarget = target();
  if (mode === 'build') tauri('build', selectedTarget);
  else if (mode === 'native-regressions') run('cargo', nativeRegressionArgs(selectedTarget));
  else if (mode === 'package-native-probe') packageNativeProbe(selectedTarget);
  else if (mode === 'build-native-probe') packageNativeProbe(selectedTarget, true);
  else if (mode === 'stage-universal') stageInputs({ ...identity(), target: selectedTarget, outputDir: path.join(root, 'universal-inputs', selectedTarget) });
  else if (mode === 'universal') {
    mergeInputs({ ...identity(), inputsDir: path.join(root, 'universal-inputs') });
    tauri('bundle', selectedTarget);
  } else if (mode === 'package') packageOutputs(selectedTarget);
  else throw new Error('Usage: cloud-build.cjs plan [platform] | build | native-regressions | package-native-probe | build-native-probe | stage-universal | universal | package');
}

if (require.main === module) {
  try { main(process.argv.slice(2)); } catch (error) { console.error(error); process.exitCode = 1; }
}
module.exports = { buildPlan, platforms, tauriArgs, nativeRegressionArgs, nativeProbeExecutable, releaseDirectory };
