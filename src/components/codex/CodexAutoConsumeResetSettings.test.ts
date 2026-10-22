import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, loadHookModule, settlePromises } from '../../../tests/helpers/reactHookHarness';

type Config = { enabled: boolean; threshold: number };
type Element = { type: unknown; props: Record<string, any> };
const defaults = (): Config => ({ enabled: false, threshold: 2 });
const t = (key: string, args?: unknown) => args ? `${key}:${JSON.stringify(args)}` : key;

function nodes(tree: unknown): Element[] {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== 'object' || !('props' in tree)) return [];
  const node = tree as Element;
  return [node, ...nodes(node.props.children)];
}

class SettingsEvent extends Event {
  detail: { accountId: string };
  constructor(name: string, options: { detail: { accountId: string } }) {
    super(name); this.detail = options.detail;
  }
}

function backend(initial: Record<string, Config> = {}) {
  const stored = new Map(Object.entries(initial).map(([id, value]) => [id, { ...value }]));
  const calls: { command: string; accountId: string; enabled?: boolean; threshold?: number }[] = [];
  const reads = new Map<string, ReturnType<typeof deferred<Config>>>();
  const saves = new Map<string, ReturnType<typeof deferred<void>>>();
  const readFailures = new Set<string>();
  return {
    stored, calls, events: new EventTarget(),
    delayRead(accountId: string) { const gate = deferred<Config>(); reads.set(accountId, gate); return gate; },
    delaySave(accountId: string) { const gate = deferred<void>(); saves.set(accountId, gate); return gate; },
    failRead(accountId: string) { readFailures.add(accountId); },
    async invoke(command: string, args: { accountId: string; enabled?: boolean; threshold?: number }) {
      assert.ok(args.accountId, 'all settings commands must include a nonempty account ID');
      calls.push({ command, ...args });
      if (command === 'get_codex_auto_reset_settings') {
        const delayed = reads.get(args.accountId); reads.delete(args.accountId);
        if (delayed) return delayed.promise;
        if (readFailures.has(args.accountId)) throw new Error('read failed');
        return { ...(stored.get(args.accountId) ?? defaults()) };
      }
      assert.equal(command, 'set_codex_auto_reset_settings', 'only account settings calls are permitted');
      const pending = saves.get(args.accountId); saves.delete(args.accountId);
      if (pending) await pending.promise;
      const current = stored.get(args.accountId) ?? defaults();
      const updated = { enabled: args.enabled ?? current.enabled, threshold: args.threshold ?? current.threshold };
      stored.set(args.accountId, updated);
      return { ...updated };
    },
  };
}

function harness(api = backend(), initialAccountId = 'A') {
  let accountId = initialAccountId;
  const h = loadHookModule(new URL('./CodexAutoConsumeResetSettings.tsx', import.meta.url), {
    'react-i18next': { useTranslation: () => ({ t }) },
    '../ModalErrorMessage': { ModalErrorMessage: 'modal-error' },
    '@tauri-apps/api/core': { invoke: api.invoke },
  }, { window: api.events, Event, CustomEvent: SettingsEvent });
  const render = () => h.render(() => h.exports.CodexAutoConsumeResetSettings({ accountId }));
  render();
  const find = (predicate: (node: Element) => boolean) => {
    const node = nodes(h.flush()).find(predicate);
    assert.ok(node, 'expected control is rendered'); return node;
  };
  return {
    h, api,
    switchAccount(id: string) { accountId = id; return render(); },
    toggle: () => find(node => node.type === 'input' && node.props.type === 'checkbox'),
    threshold: () => find(node => node.type === 'input' && node.props.type === 'number'),
    submit: () => find(node => node.type === 'form').props.onSubmit({ preventDefault() {} }),
    error: () => find(node => node.type === 'modal-error').props.message,
  };
}

const saves = (api: ReturnType<typeof backend>) => api.calls.filter(call => call.command === 'set_codex_auto_reset_settings');

test('default is off/2, labels are accessible, and an empty account never invokes commands', async () => {
  const x = harness();
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.toggle().props.disabled, true);
  await settlePromises();
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.threshold().props.value, '2');
  assert.deepEqual(x.api.calls, [{ command: 'get_codex_auto_reset_settings', accountId: 'A' }]);
  const tree = nodes(x.h.flush());
  assert.ok(tree.some(node => node.type === 'label' && node.props.htmlFor === x.threshold().props.id));
  assert.ok(tree.some(node => node.type === 'p' && node.props.id === x.toggle().props['aria-describedby']));
  x.h.unmount();
  const empty = harness(backend(), ''); await settlePromises();
  empty.toggle().props.onChange({ target: { checked: true } });
  empty.submit();
  assert.equal(empty.api.calls.length, 0);
  assert.equal(empty.toggle().props.checked, false);
  assert.equal(empty.toggle().props.disabled, true);
  empty.h.unmount();
});

test('enabling A deduplicates input, preserves B, and restores A after remount', async () => {
  const api = backend({ A: { enabled: false, threshold: 7 }, B: { enabled: false, threshold: 12 } });
  const x = harness(api); await settlePromises();
  const pending = api.delaySave('A');
  const toggle = x.toggle();
  toggle.props.onChange({ target: { checked: true } });
  toggle.props.onChange({ target: { checked: true } });
  assert.equal(saves(api).length, 1);
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.toggle().props.disabled, true);
  pending.resolve(); await settlePromises();
  assert.equal(x.toggle().props.checked, true);
  assert.deepEqual(api.stored.get('A'), { enabled: true, threshold: 7 });
  assert.deepEqual(api.stored.get('B'), { enabled: false, threshold: 12 });
  x.h.unmount();
  const restored = harness(api); await settlePromises();
  assert.equal(restored.toggle().props.checked, true);
  assert.equal(restored.threshold().props.value, '7');
  restored.h.unmount();
});

test('invalid thresholds cannot enable; threshold-only save and disabling stay scoped to A', async () => {
  const api = backend({ B: { enabled: true, threshold: 19 } });
  const x = harness(api); await settlePromises();
  for (const value of ['', '-1', '100', '2.5']) {
    x.threshold().props.onChange({ target: { value } });
    x.toggle().props.onChange({ target: { checked: true } });
    x.submit();
    assert.match(x.error(), /autoConsumeResetInvalidThreshold/);
  }
  assert.equal(saves(api).length, 0);
  x.threshold().props.onChange({ target: { value: '7' } });
  x.submit(); await settlePromises();
  assert.deepEqual(api.stored.get('A'), { enabled: false, threshold: 7 });
  x.toggle().props.onChange({ target: { checked: true } }); await settlePromises();
  x.threshold().props.onChange({ target: { value: '' } });
  x.toggle().props.onChange({ target: { checked: false } }); await settlePromises();
  assert.deepEqual(api.stored.get('A'), { enabled: false, threshold: 7 });
  assert.deepEqual(api.stored.get('B'), { enabled: true, threshold: 19 });
  assert.ok(saves(api).every(call => call.accountId === 'A'));
  x.h.unmount();
});

test('late A read cannot display or enable A settings after switching to B', async () => {
  const api = backend();
  const readA = api.delayRead('A');
  const readB = api.delayRead('B');
  const x = harness(api);
  x.switchAccount('B');
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.toggle().props.disabled, true);
  readA.resolve({ enabled: true, threshold: 44 }); await settlePromises();
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.threshold().props.value, '2');
  assert.equal(x.toggle().props.disabled, true);
  readB.resolve({ enabled: false, threshold: 3 }); await settlePromises();
  assert.equal(x.threshold().props.value, '3');
  assert.equal(saves(api).length, 0);
  x.h.unmount();
});

test('pending A save and stale handlers cannot overwrite B after prop switching', async () => {
  const api = backend({ A: { enabled: false, threshold: 7 }, B: { enabled: false, threshold: 9 } });
  const x = harness(api); await settlePromises();
  const pending = api.delaySave('A');
  const oldToggle = x.toggle();
  oldToggle.props.onChange({ target: { checked: true } });
  const readB = api.delayRead('B');
  x.switchAccount('B');
  oldToggle.props.onChange({ target: { checked: true } });
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.toggle().props.disabled, true);
  readB.resolve({ enabled: false, threshold: 9 }); await settlePromises();
  pending.resolve(); await settlePromises();
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.threshold().props.value, '9');
  assert.equal(saves(api).length, 1);
  x.toggle().props.onChange({ target: { checked: true } }); await settlePromises();
  assert.deepEqual(api.stored.get('A'), { enabled: true, threshold: 7 });
  assert.deepEqual(api.stored.get('B'), { enabled: true, threshold: 9 });
  x.h.unmount();
});

test('two editors save different accounts concurrently without cross-account refresh or edits', async () => {
  const api = backend(); const a = harness(api, 'A'); const b = harness(api, 'B'); await settlePromises();
  const saveA = api.delaySave('A'); const saveB = api.delaySave('B');
  a.threshold().props.onChange({ target: { value: '5' } });
  b.threshold().props.onChange({ target: { value: '11' } });
  a.toggle().props.onChange({ target: { checked: true } });
  b.toggle().props.onChange({ target: { checked: true } });
  saveB.resolve(); await settlePromises(); saveA.resolve(); await settlePromises();
  assert.deepEqual(api.stored.get('A'), { enabled: true, threshold: 5 });
  assert.deepEqual(api.stored.get('B'), { enabled: true, threshold: 11 });
  assert.equal(a.threshold().props.value, '5'); assert.equal(b.threshold().props.value, '11');
  assert.equal(api.calls.filter(call => call.command === 'get_codex_auto_reset_settings').length, 2);
  a.h.unmount(); b.h.unmount();
});

test('returning to A during its pending save stays locked until its persisted settings are reloaded', async () => {
  const api = backend(); const x = harness(api); await settlePromises();
  const pending = api.delaySave('A');
  x.threshold().props.onChange({ target: { value: '8' } });
  x.toggle().props.onChange({ target: { checked: true } });
  x.switchAccount('B'); await settlePromises();
  x.switchAccount('A'); await settlePromises();
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.toggle().props.disabled, true);
  pending.resolve(); await settlePromises();
  assert.equal(x.toggle().props.checked, true);
  assert.equal(x.toggle().props.disabled, false);
  assert.equal(x.threshold().props.value, '8');
  assert.equal(api.stored.has('B'), false);
  x.h.unmount();
});

test('returning to A after a stale save failure releases its lock without changing reloaded settings', async () => {
  const api = backend({ A: { enabled: false, threshold: 4 } });
  const x = harness(api); await settlePromises();
  const pending = api.delaySave('A');
  x.toggle().props.onChange({ target: { checked: true } });
  x.switchAccount('B'); await settlePromises();
  x.switchAccount('A'); await settlePromises();
  assert.equal(x.toggle().props.disabled, true);
  pending.reject(new Error('stale save failed')); await settlePromises();
  assert.equal(x.toggle().props.disabled, false);
  assert.equal(x.toggle().props.checked, false);
  assert.equal(x.threshold().props.value, '4');
  assert.equal(x.error(), null);
  assert.deepEqual(api.stored.get('A'), { enabled: false, threshold: 4 });
  x.toggle().props.onChange({ target: { checked: true } }); await settlePromises();
  assert.equal(x.toggle().props.checked, true);
  x.h.unmount();
});

test('save failure remains off and a B read failure never exposes the previous A settings', async () => {
  const api = backend(); const x = harness(api); await settlePromises();
  const pending = api.delaySave('A');
  x.toggle().props.onChange({ target: { checked: true } });
  pending.reject(new Error('disk failure')); await settlePromises();
  assert.equal(x.toggle().props.checked, false); assert.equal(x.toggle().props.disabled, false);
  assert.match(x.error(), /disk failure/);
  x.toggle().props.onChange({ target: { checked: true } }); await settlePromises();
  api.failRead('B'); x.switchAccount('B'); await settlePromises();
  assert.equal(x.toggle().props.checked, false); assert.equal(x.toggle().props.disabled, true);
  assert.match(x.error(), /read failed/);
  assert.equal(api.stored.has('B'), false);
  x.h.unmount();
});
