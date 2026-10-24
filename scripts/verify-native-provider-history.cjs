#!/usr/bin/env node
// Explicit local acceptance only. This runs a cloud-built, request-bounded test
// driver; it never builds code or changes a real Codex profile/session.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');

const MODELS = ['step-5-preview', 'step-3.7-flash'];
const GENERIC_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'];
const MAX_OUTPUT = 16 * 1024 * 1024;
const safeFailure = (code) => Object.assign(new Error(code), { safeCode: code });
const requireThat = (condition, code) => { if (!condition) throw safeFailure(code); };
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const print = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);

function help() {
  return `Usage: node scripts/verify-native-provider-history.cjs --probe PATH --data-dir PATH --account-id ID [options]

Runs a read-only saved StepFun account snapshot through the cloud-built ignored
native_provider_process_driver. The production writer creates an isolated Codex
profile. One synthetic session performs a fresh Step 5 read/apply_patch/verify
turn, resumes Step 5 with a synthetic old-Go reasoning item, then resumes the
same session with Step 3.7 selected in that temporary profile's config.toml.
No wire_api, model catalog, or authentication override is supplied to Codex.

Options:
  --codex PATH                Installed Codex CLI (default: codex)
  --timeout-seconds N          Per-turn timeout, 30..600 (default: 240)
  --max-requests N             Total driver route/send budget, 12..32 (default: 32)
  --expect-legacy-rejection    Negative control: require old probe to reject the
                               first resume with chat_parameter_not_representable
  --check-effort-passthrough   Configure temporary Step 3.7 low-only menu/default;
                               verify explicit high stays high and absent effort stays absent
  --check-generic-effort-menu  Require both models' production catalogs to publish the
                               six compatibility choices, medium default, and unknown capability
                               (Step 5 only when combined with --check-effort-passthrough);
                               fail if upstream/manual capability information is now present
  --self-test                 Synthetic helper checks only; no account/network
  --help                      Show this help; no account/network

Only status, shape, and count evidence is printed. All generated profiles,
session journals, fixtures and reports are removed on success or failure.
The supplied probe must already have passed its cloud-build verification.
`;
}

function options(args) {
  const result = { codex: 'codex', timeout: 240, requests: 32, negative: false, efforts: false, genericMenu: false };
  const names = { '--probe': 'probe', '--data-dir': 'data', '--account-id': 'account', '--codex': 'codex', '--timeout-seconds': 'timeout', '--max-requests': 'requests' };
  for (let i = 0; i < args.length; i += 1) {
    const name = args[i];
    if (name === '--expect-legacy-rejection') result.negative = true;
    else if (name === '--check-effort-passthrough') result.efforts = true;
    else if (name === '--check-generic-effort-menu') result.genericMenu = true;
    else if (names[name]) {
      requireThat(i + 1 < args.length && !args[i + 1].startsWith('--'), 'missing_argument');
      result[names[name]] = args[++i];
    } else throw safeFailure('unknown_argument');
  }
  requireThat(result.probe && result.data && /^[A-Za-z0-9_-]+$/.test(result.account || ''), 'explicit_scope_required');
  requireThat(!(result.negative && result.efforts), 'incompatible_acceptance_modes');
  result.timeout = Number(result.timeout);
  result.requests = Number(result.requests);
  requireThat(Number.isInteger(result.timeout) && result.timeout >= 30 && result.timeout <= 600, 'invalid_timeout');
  requireThat(Number.isInteger(result.requests) && result.requests >= 12 && result.requests <= 32, 'invalid_request_budget');
  result.probe = fs.realpathSync(result.probe);
  result.data = fs.realpathSync(result.data);
  requireThat(fs.statSync(result.probe).isFile() && fs.statSync(result.data).isDirectory(), 'invalid_paths');
  return result;
}

function minimalEnvironment() {
  const env = {};
  for (const name of ['PATH', 'HOME', 'TMPDIR', 'LANG', 'LC_ALL', 'USER', 'LOGNAME', 'SSL_CERT_FILE', 'SSL_CERT_DIR']) {
    if (process.env[name] !== undefined) env[name] = process.env[name];
  }
  return env;
}

function privateWrite(file, data) { fs.writeFileSync(file, data, { mode: 0o600 }); }
function records(file) {
  if (!fs.existsSync(file)) return [];
  requireThat(fs.statSync(file).size <= MAX_OUTPUT, 'private_evidence_too_large');
  return fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

function startProcess(command, args, env, cwd, onRecord) {
  const child = spawn(command, args, { env, cwd, stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '', stderr = '', pending = '', overflow = false;
  const collect = (name, chunk) => {
    const text = chunk.toString();
    if (stdout.length + stderr.length + text.length > MAX_OUTPUT) {
      overflow = true;
      child.kill('SIGTERM');
      return;
    }
    if (name === 'stderr') { stderr += text; return; }
    stdout += text;
    pending += text;
    const lines = pending.split('\n');
    pending = lines.pop();
    for (const line of lines) {
      if (!onRecord) continue;
      try { onRecord(JSON.parse(line), child); } catch { /* Non-JSON driver output stays private. */ }
    }
  };
  child.stdout.on('data', (chunk) => collect('stdout', chunk));
  child.stderr.on('data', (chunk) => collect('stderr', chunk));
  let finished = false, result;
  const completion = new Promise((resolve) => {
    child.once('error', () => { finished = true; result = { code: null, spawnFailed: true }; resolve(result); });
    child.once('close', (code, signal) => {
      finished = true;
      result = { code, signal, overflow, stdout, stderr };
      resolve(result);
    });
  });
  return { child, completion, get finished() { return finished; }, get result() { return result; } };
}

async function stopProcess(running, graceful) {
  if (!running || running.finished) return running?.result;
  if (graceful) running.child.stdin.end('stop\n');
  else running.child.kill('SIGTERM');
  const first = await Promise.race([running.completion, pause(12_000).then(() => null)]);
  if (first) return first;
  running.child.kill('SIGKILL');
  return running.completion;
}

function changeSavedModel(configPath, model) {
  requireThat(MODELS.includes(model), 'model_outside_scope');
  const text = fs.readFileSync(configPath, 'utf8');
  const tableStart = text.search(/^\s*\[/m);
  const prefix = tableStart < 0 ? text : text.slice(0, tableStart);
  const suffix = tableStart < 0 ? '' : text.slice(tableStart);
  requireThat(/^model[ \t]*=[ \t]*"[^"\n]+"[ \t]*$/m.test(prefix), 'generated_model_selection_missing');
  privateWrite(configPath, prefix.replace(/^model[ \t]*=[ \t]*"[^"\n]+"[ \t]*$/m, `model = "${model}"`) + suffix);
}

function sessionFile(home, sessionId) {
  requireThat(/^[a-f0-9-]{36}$/i.test(sessionId), 'invalid_synthetic_session_id');
  const found = [];
  function visit(dir) {
    if (!fs.existsSync(dir)) return;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) visit(file);
      else if (entry.isFile() && entry.name.endsWith('.jsonl') && entry.name.includes(sessionId)) found.push(file);
    }
  }
  visit(path.join(home, 'sessions'));
  requireThat(found.length === 1, 'synthetic_session_not_unique');
  const metadata = records(found[0]).find((item) => item.type === 'session_meta');
  requireThat(metadata?.payload?.id === sessionId, 'synthetic_session_identity_mismatch');
  return found[0];
}

function injectLegacyReasoning(file) {
  const rows = records(file);
  const index = rows.findLastIndex((row) => row.type === 'response_item' && row.payload?.type === 'message' && row.payload.role === 'assistant');
  requireThat(index >= 0, 'synthetic_assistant_history_missing');
  rows.splice(index, 0, {
    timestamp: new Date().toISOString(), type: 'response_item',
    payload: { type: 'reasoning', id: `rs_synthetic_${crypto.randomBytes(8).toString('hex')}`, summary: [{ type: 'summary_text', text: 'The synthetic fixture was read, edited with apply_patch, and checked.' }], encrypted_content: '' },
  });
  privateWrite(file, `${rows.map((row) => JSON.stringify(row)).join('\n')}\n`);
}

function historyEvidence(rows) {
  const calls = new Map(), results = new Map();
  let customCalls = 0, reasoningEmpty = 0;
  for (const row of rows) {
    if (row.type !== 'response_item') continue;
    const item = row.payload;
    if (item?.type === 'reasoning' && item.encrypted_content === '') reasoningEmpty += 1;
    if (['function_call', 'custom_tool_call'].includes(item?.type)) {
      requireThat(typeof item.call_id === 'string' && !calls.has(item.call_id), 'duplicate_synthetic_call_id');
      calls.set(item.call_id, item.type);
      if (item.type === 'custom_tool_call') customCalls += 1;
    }
    if (['function_call_output', 'custom_tool_call_output'].includes(item?.type)) {
      requireThat(typeof item.call_id === 'string' && !results.has(item.call_id), 'duplicate_synthetic_result_id');
      results.set(item.call_id, item.type);
    }
  }
  return { calls: calls.size, results: results.size, customCalls, reasoningEmpty, paired: calls.size === results.size && [...calls.entries()].every(([id, type]) => results.get(id) === `${type}_output`) };
}

function reportEvidence(file, offset) {
  const rows = records(file).slice(offset);
  const completed = rows.filter((row) => row.stage === 'completed');
  return {
    requests: completed.length,
    statuses: completed.map((row) => row.status),
    success: completed.length > 0 && completed.every((row) => row.status === 200 && row.success === true),
    reasoningSeen: rows.some((row) => row.stage === 'prepare' && row.history?.some((item) => item.type === 'reasoning')),
    functionHistorySeen: rows.some((row) => row.stage === 'prepare' && row.history?.some((item) => item.type === 'function_call_output')),
    customHistorySeen: rows.some((row) => row.stage === 'prepare' && row.history?.some((item) => item.type === 'custom_tool_call_output')),
  };
}

function genericEffortMenuEvidence(catalog, modelId = MODELS[0]) {
  requireThat(MODELS.includes(modelId), 'model_outside_scope');
  const matches = Array.isArray(catalog.models)
    ? catalog.models.filter((item) => item?.slug === modelId) : [];
  requireThat(matches.length === 1, 'generic_effort_model_not_unique');
  const model = matches[0];
  // This check deliberately fails when the source changes. Never discard newly
  // discovered or manually supplied capabilities to force a fallback assertion.
  requireThat(model.cockpit_reasoning_known !== true, 'generic_effort_menu_has_declared_capabilities');
  requireThat(model.cockpit_reasoning_known === false, 'generic_effort_source_not_unknown');
  const choices = Array.isArray(model.supported_reasoning_levels)
    ? model.supported_reasoning_levels.map((item) => item?.effort) : [];
  requireThat(JSON.stringify(choices) === JSON.stringify(GENERIC_EFFORTS), 'generic_effort_choices_mismatch');
  requireThat(model.default_reasoning_level === 'medium', 'generic_effort_default_mismatch');
  return { stage: 'generic_effort_menu', model: modelId, choices: [...GENERIC_EFFORTS],
    default: 'medium', capabilitiesKnown: false, source: 'compatibility_fallback', passed: true };
}

async function runTurn(context, model, resume, expectedRejection = false) {
  changeSavedModel(context.config, model);
  const nonce = crypto.randomBytes(24).toString('hex');
  privateWrite(path.join(context.work, 'input.txt'), `${nonce}\n`);
  privateWrite(path.join(context.work, 'result.txt'), 'replace-this-synthetic-line\n');
  const finalPath = path.join(context.root, `last-${context.turn}.txt`);
  const beforeRows = resume ? records(context.rollout).length : 0;
  const beforeReports = records(context.reports).length;
  const counters = { startedTools: 0, commandSuccess: 0, fileChangeSuccess: 0, failedTools: 0 };
  let sessionId, toolBudgetExceeded = false;
  const args = ['exec'];
  if (resume) args.push('resume');
  args.push('--skip-git-repo-check', '--ignore-rules', '--json', '-o', finalPath);
  if (!resume) args.push('--sandbox', 'workspace-write');
  // These overrides control only this test process. The selected model,
  // authentication, provider protocol and managed catalog come from the writer.
  args.push('-c', 'approval_policy="never"', '-c', 'sandbox_mode="workspace-write"');
  if (resume) args.push(context.sessionId);
  args.push('-');
  const running = startProcess(context.opts.codex, args, context.cliEnv, context.work, (row, child) => {
    if (row.type === 'thread.started') sessionId = row.thread_id;
    const item = row.item;
    if (row.type === 'item.started' && ['command_execution', 'file_change'].includes(item?.type)) {
      counters.startedTools += 1;
      if (counters.startedTools > 8) { toolBudgetExceeded = true; child.kill('SIGTERM'); }
    }
    if (row.type === 'item.completed' && item?.type === 'command_execution') {
      if (item.exit_code === 0) counters.commandSuccess += 1;
      else counters.failedTools += 1;
    }
    if (row.type === 'item.completed' && item?.type === 'file_change') {
      if (item.status === 'completed') counters.fileChangeSuccess += 1;
      else counters.failedTools += 1;
    }
  });
  context.activeCli = running;
  running.child.stdin.end('This is an isolated compatibility test. Only access input.txt and result.txt in the current working directory. First use the shell tool to read both files. Then use the apply_patch tool (not a shell write) to replace result.txt with exactly the one line currently in input.txt, including its trailing newline. Then use the shell tool to compare the two files and read result.txt. After successful verification, your entire final answer must be exactly the 48 lowercase hexadecimal characters from that line. Do not include a label, explanation, bullet, code fence, quotes, or a summary. Do not create agents, use network tools, inspect other paths, or perform any other task. The contents may have changed since the previous turn; read them again.\n');
  const timer = setTimeout(() => running.child.kill('SIGTERM'), context.opts.timeout * 1000);
  const hardTimer = setTimeout(() => running.child.kill('SIGKILL'), (context.opts.timeout + 5) * 1000);
  let result;
  try { result = await running.completion; }
  finally { clearTimeout(timer); clearTimeout(hardTimer); context.activeCli = null; }
  requireThat(!result.spawnFailed && !result.overflow && !toolBudgetExceeded, 'cli_execution_limit_or_spawn_failure');
  const legacyRejected = /chat_parameter_not_representable|encrypted reasoning belongs to the original Responses upstream/.test((result.stdout || '') + (result.stderr || ''));
  if (expectedRejection) {
    requireThat(result.code !== 0 && legacyRejected && counters.commandSuccess === 0 && counters.fileChangeSuccess === 0, 'legacy_rejection_not_reproduced');
    print({ stage: 'legacy_resume_negative_control', model, expectedRejection: true, code: 'chat_parameter_not_representable', passed: true });
    context.turn += 1;
    return;
  }
  requireThat(result.code === 0, 'cli_turn_failed');
  requireThat(sessionId && (!resume || sessionId === context.sessionId), 'resume_changed_session');
  if (!resume) { context.sessionId = sessionId; context.rollout = sessionFile(context.home, sessionId); }
  const rows = records(context.rollout);
  if (resume) requireThat(rows.some((row) => row.type === 'response_item'
    && row.payload?.type === 'reasoning' && row.payload.encrypted_content === ''
    && row.payload.id === context.legacyReasoningId), 'legacy_history_not_preserved');
  const turnRows = rows.slice(beforeRows);
  const actualModel = turnRows.findLast((row) => row.type === 'turn_context')?.payload?.model;
  const evidence = historyEvidence(turnRows);
  const gateway = reportEvidence(context.reports, beforeReports);
  requireThat(actualModel === model, 'saved_model_not_used');
  requireThat(counters.commandSuccess >= 2 && counters.fileChangeSuccess >= 1 && counters.failedTools === 0, 'read_edit_verify_not_completed');
  if (!(evidence.calls >= 3 && evidence.customCalls >= 1 && evidence.paired)) {
    print({stage: 'tool_history_check', model, ...counters, history: evidence, native: gateway,
      passed: false});
  }
  requireThat(evidence.calls >= 3 && evidence.customCalls >= 1 && evidence.paired, 'tool_history_not_paired');
  requireThat(fs.readFileSync(path.join(context.work, 'result.txt'), 'utf8') === `${nonce}\n`, 'file_nonce_mismatch');
  const finalText = fs.existsSync(finalPath) ? fs.readFileSync(finalPath, 'utf8').trim() : '';
  if (finalText !== nonce) print({ stage: 'final_answer_check', model,
    commandSuccess: counters.commandSuccess, fileChangeSuccess: counters.fileChangeSuccess,
    fileNonceMatches: true, finalNonceMatches: false, finalCharacters: finalText.length,
    expectedCharacters: nonce.length, containsExactNonce: finalText.includes(nonce),
    native: gateway });
  requireThat(finalText === nonce, 'final_nonce_mismatch');
  requireThat(gateway.success && gateway.functionHistorySeen && gateway.customHistorySeen, 'native_tool_replay_not_verified');
  if (resume) requireThat(gateway.reasoningSeen, 'reasoning_history_not_observed');
  if (context.opts.efforts && model === MODELS[1]) {
    const efforts = records(context.reports).slice(beforeReports).filter((row) => row.stage === 'reasoning');
    requireThat(efforts.length > 0 && efforts.every((row) => row.effective === row.requested), 'caller_effort_not_preserved');
    print({stage: 'client_effort', model, requested: [...new Set(efforts.map((row) => row.requested))], preserved: true});
  }
  print({ stage: resume ? 'resume' : 'fresh', model, sameSession: !resume || sessionId === context.sessionId, savedModelConfirmed: true, ...counters, history: evidence, native: gateway, fileNonceMatches: true, finalNonceMatches: true, passed: true });
  context.turn += 1;
}

async function main(opts) {
  requireThat(!process.env.GITHUB_ACTIONS, 'local_acceptance_only');
  const version = spawnSync(opts.codex, ['--version'], { encoding: 'utf8', env: minimalEnvironment() });
  const cliVersion = version.stdout?.trim().match(/^codex-cli (\S+)$/)?.[1];
  requireThat(version.status === 0 && ['0.162.1', '0.164.0-alpha.11.1'].includes(cliVersion), 'unexpected_codex_version');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-native-history-'));
  fs.chmodSync(root, 0o700);
  const output = path.join(root, 'driver');
  const config = path.join(root, 'driver-config.json');
  const work = path.join(root, 'work');
  const isolatedHome = path.join(root, 'user-home');
  fs.mkdirSync(work, { mode: 0o700 });
  fs.mkdirSync(isolatedHome, { mode: 0o700 });
  const localKey = crypto.randomBytes(32).toString('hex');
  privateWrite(config, JSON.stringify({ account_id: opts.account, allowed_models: MODELS, key: localKey, output_dir: output, max_requests: opts.requests, ...(opts.efforts ? {manual_low: true} : {}) }));
  const context = { root, work, opts, turn: 0, activeCli: null };
  let driver;
  const interrupted = () => { context.interrupted = true; context.activeCli?.child.kill('SIGTERM'); driver?.child.stdin.end('stop\n'); };
  process.once('SIGINT', interrupted);
  process.once('SIGTERM', interrupted);
  try {
    const env = { ...minimalEnvironment(), COCKPIT_TOOLS_TEST_DATA_DIR: opts.data, COCKPIT_NATIVE_PROVIDER_LIVE_CONFIG: config };
    // The fully qualified module name can vary across compatible driver builds;
    // the unique suffix selects exactly one ignored test without running others.
    driver = startProcess(opts.probe, ['native_provider_process_driver', '--ignored', '--nocapture', '--test-threads=1'], env, root);
    const readyFile = path.join(output, 'ready.json');
    const deadline = Date.now() + 30_000;
    while (!fs.existsSync(readyFile) && !driver.finished && Date.now() < deadline && !context.interrupted) await pause(250);
    requireThat(fs.existsSync(readyFile) && !driver.finished && !context.interrupted, 'provider_driver_not_ready');
    const ready = JSON.parse(fs.readFileSync(readyFile, 'utf8'));
    requireThat(ready.production_client_projection === true && ready.client_profile === 'codex-home' && Number.isInteger(ready.port), 'production_profile_not_generated');
    fs.unlinkSync(config);
    context.home = path.join(output, 'codex-home');
    context.config = path.join(context.home, 'config.toml');
    context.reports = path.join(output, 'reports.jsonl');
    if (opts.efforts || opts.genericMenu) {
      requireThat(typeof ready.model_catalog === 'string' && path.basename(ready.model_catalog) === ready.model_catalog, 'model_catalog_reference_invalid');
      const catalog = JSON.parse(fs.readFileSync(path.join(context.home, ready.model_catalog), 'utf8'));
      if (opts.genericMenu) {
        for (const model of opts.efforts ? [MODELS[0]] : MODELS) print(genericEffortMenuEvidence(catalog, model));
      }
      if (opts.efforts) {
        const model = catalog.models?.find((item) => item.slug === MODELS[1]);
        requireThat(model?.default_reasoning_level === 'low'
          && JSON.stringify(model.supported_reasoning_levels?.map((item) => item.effort)) === '["low"]', 'manual_menu_not_published');
        print({stage: 'manual_menu', model: MODELS[1], choices: ['low'], default: 'low', temporary: true, passed: true});
      }
    }
    context.cliEnv = { ...minimalEnvironment(), HOME: isolatedHome, CODEX_HOME: context.home, NO_COLOR: '1' };
    const initial = records(context.reports).find((row) => row.stage === 'ready');
    requireThat(initial?.sidecar_blocked === true && initial.account_snapshot === 'read_only' && initial.saved_policy === 'prefer_regular', 'credits_or_snapshot_contract_missing');
    print({ stage: 'ready', cliVersion, productionClientProjection: true, savedPolicy: 'prefer_regular', sidecarBlocked: true, maxRequests: opts.requests });
    await runTurn(context, MODELS[0], false);
    injectLegacyReasoning(context.rollout);
    context.legacyReasoningId = records(context.rollout).findLast((row) =>
      row.type === 'response_item' && row.payload?.type === 'reasoning'
      && row.payload.encrypted_content === '')?.payload.id;
    requireThat(context.legacyReasoningId, 'legacy_history_not_injected');
    print({ stage: 'synthetic_history_injected', type: 'reasoning', encryptedContent: 'empty', summaryPresent: true, realHistoryModified: false });
    await runTurn(context, MODELS[0], true, opts.negative);
    if (!opts.negative) await runTurn(context, MODELS[1], true);
    if (opts.efforts) {
      for (const control of [
        {model: MODELS[0], requested: 'high', expected: 'high'},
        {model: MODELS[1], requested: 'high', expected: 'high'},
        {model: MODELS[1], requested: null, expected: 'absent'},
      ]) {
        const {model, requested, expected} = control;
        const offset = records(context.reports).length;
        const response = await fetch(`http://127.0.0.1:${ready.port}/v1/responses`, {
          method: 'POST', headers: {Authorization: `Bearer ${localKey}`, 'content-type': 'application/json'},
          body: JSON.stringify({model, stream: false, store: false, ...(requested === null ? {} : {reasoning: {effort: requested}}), input: 'This is a synthetic compatibility check. Reply only OK.'}),
          signal: AbortSignal.timeout(opts.timeout * 1000),
        });
        const result = await response.json();
        requireThat(response.status === 200 && result.status === 'completed', 'effort_upstream_failed');
        const efforts = records(context.reports).slice(offset).filter((row) => row.stage === 'reasoning');
        requireThat(efforts.length > 0 && efforts.every((row) => row.requested === (requested ?? 'absent') && row.effective === expected), 'effort_preservation_evidence_failed');
        print({stage: 'effort_contract', model, requested: requested ?? 'absent', effective: expected, status: response.status, manualMenu: model === MODELS[1], passed: true});
      }
    }
    const stopped = await stopProcess(driver, true);
    requireThat(stopped?.code === 0 && !stopped.overflow && !/FAILED|panicked/.test(stopped.stdout || ''), 'driver_shutdown_failed');
    print({ stage: 'complete', turns: context.turn, negativeControl: opts.negative, driverExitCode: 0, passed: true });
  } finally {
    await stopProcess(context.activeCli, false);
    await stopProcess(driver, true);
    fs.rmSync(root, { recursive: true, force: true });
    process.removeListener('SIGINT', interrupted);
    process.removeListener('SIGTERM', interrupted);
    print({ stage: 'cleanup', temporaryDataRemoved: !fs.existsSync(root) });
  }
}

function selfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ct-native-history-selftest-'));
  try {
    const file = path.join(root, 'config.toml');
    privateWrite(file, 'model = "step-5-preview"\n[model_providers.codex_local_access]\nwire_api = "responses"\n');
    changeSavedModel(file, MODELS[1]);
    requireThat(fs.readFileSync(file, 'utf8') === 'model = "step-3.7-flash"\n[model_providers.codex_local_access]\nwire_api = "responses"\n', 'selftest_model');
    const journal = path.join(root, 'rollout.jsonl');
    privateWrite(journal, `${JSON.stringify({ type: 'response_item', payload: { type: 'message', role: 'assistant', content: [] } })}\n`);
    injectLegacyReasoning(journal);
    requireThat(historyEvidence(records(journal)).reasoningEmpty === 1, 'selftest_injection');
    const paired = historyEvidence([{ type: 'response_item', payload: { type: 'custom_tool_call', call_id: 'synthetic-call' } }, { type: 'response_item', payload: { type: 'custom_tool_call_output', call_id: 'synthetic-call' } }]);
    requireThat(paired.paired && paired.customCalls === 1, 'selftest_pairing');
    const genericModel = { slug: MODELS[0], cockpit_reasoning_known: false,
      default_reasoning_level: 'medium', supported_reasoning_levels: GENERIC_EFFORTS.map((effort) => ({effort})) };
    const genericCatalog = { models: [genericModel, {slug: MODELS[1], cockpit_reasoning_known: true,
      default_reasoning_level: 'low', supported_reasoning_levels: [{effort: 'low'}]}] };
    const before = JSON.stringify(genericCatalog);
    requireThat(genericEffortMenuEvidence(genericCatalog).passed && JSON.stringify(genericCatalog) === before, 'selftest_generic_menu');
    const expectFailure = (catalog, code, modelId) => {
      let caught;
      try { genericEffortMenuEvidence(catalog, modelId); } catch (error) { caught = error.safeCode; }
      requireThat(caught === code, 'selftest_generic_menu_rejection');
    };
    expectFailure({models: [{...genericModel, cockpit_reasoning_known: true}]}, 'generic_effort_menu_has_declared_capabilities');
    expectFailure({models: [{...genericModel, cockpit_reasoning_known: undefined}]}, 'generic_effort_source_not_unknown');
    expectFailure({models: [{...genericModel, supported_reasoning_levels: [{effort: 'low'}]}]}, 'generic_effort_choices_mismatch');
    expectFailure({models: [{...genericModel, default_reasoning_level: 'low'}]}, 'generic_effort_default_mismatch');
    expectFailure({models: [genericModel, genericModel]}, 'generic_effort_model_not_unique');
    expectFailure(genericCatalog, 'generic_effort_menu_has_declared_capabilities', MODELS[1]);
    expectFailure(genericCatalog, 'model_outside_scope', 'outside-model');
    const bothGeneric = {models: MODELS.map((slug) => ({...genericModel, slug}))};
    requireThat(MODELS.every((modelId) => genericEffortMenuEvidence(bothGeneric, modelId).model === modelId), 'selftest_both_generic_menus');
    const scopedArgs = ['--probe', __filename, '--data-dir', root, '--account-id', 'synthetic'];
    requireThat(options(scopedArgs).genericMenu === false, 'selftest_default_options');
    const combined = options([...scopedArgs, '--check-effort-passthrough', '--check-generic-effort-menu']);
    requireThat(combined.efforts && combined.genericMenu, 'selftest_combined_options');
    print({ stage: 'self_test', passed: true, realAccountRead: false, networkRequests: 0 });
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
}

if (require.main === module) {
  const args = process.argv.slice(2);
  Promise.resolve().then(() => {
    if (args.length === 1 && args[0] === '--help') process.stdout.write(help());
    else if (args.length === 1 && args[0] === '--self-test') selfTest();
    else return main(options(args));
  }).catch((error) => { print({ stage: 'failed', code: error.safeCode || 'acceptance_internal_failure', passed: false }); process.exitCode = 1; });
}

module.exports = { changeSavedModel, genericEffortMenuEvidence, historyEvidence, injectLegacyReasoning, options };
