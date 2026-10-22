import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, loadHookModule, settlePromises } from '../../../tests/helpers/reactHookHarness';

type Element = { type: unknown; props: Record<string, any> };
function nodes(tree: unknown): Element[] {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== 'object' || !('props' in tree)) return [];
  const node = tree as Element;
  return [node, ...nodes(node.props.children)];
}

function setup(save: (patch: unknown) => Promise<any>, collection: Record<string, any> = {}) {
  const props = {
    state: { collection: { accountIds: ['a', 'b', 'api'], ...collection } },
    accounts: [
      { id: 'a', email: 'a@example.com' },
      { id: 'b', email: 'b@example.com' },
      { id: 'outside', email: 'outside@example.com' },
      { id: 'api', email: 'api@example.com', auth_mode: 'api_key' },
    ],
    busy: false,
    maskAccountText: () => 'masked',
    onSaved: (state: any) => { props.state = state; },
  };
  const harness = loadHookModule(new URL('./CodexCreditsSettings.tsx', import.meta.url), {
    'react-i18next': { useTranslation: () => ({ t: (_: string, fallback: string) => fallback }) },
    '../../types/codex': { isCodexApiKeyAccount: (a: any) => a.auth_mode === 'api_key' },
    '../../services/codexLocalAccessService': { updateCodexLocalAccessCreditsPolicy: save },
    '../SingleSelectDropdown': { SingleSelectDropdown: 'select' },
  });
  const render = () => nodes(harness.render(() => harness.exports.CodexCreditsSettings(props)));
  return { props, render };
}

test('credits settings migrate to default with enabled switches and preserve account privacy', () => {
  const { render } = setup(async () => ({}));
  const tree = render();
  const selector = tree.find(n => n.type === 'select')!;
  assert.equal(selector.props.value, 'default');
  assert.equal(selector.props.options.length, 3);
  const switches = tree.filter(n => n.type === 'input');
  assert.equal(switches.length, 2);
  assert.ok(switches.every(n => n.props.checked === true));
  assert.ok(switches.every(n => !n.props['aria-label'].includes('@')));
});

test('account overrides are available for custom API key and image pools too', () => {
  const { render } = setup(async () => ({}), {
    accountIds: ['a'], apiKeys: [{ accountIds: ['outside'] }], imageGenerationAccountIds: ['b'],
  });
  assert.equal(render().filter(n => n.type === 'input').length, 3);
});

test('account switch submits a delta, waits for save, and keeps preference under strict mode', async () => {
  const pending = deferred<any>();
  const calls: any[] = [];
  const { render, props } = setup(async patch => { calls.push(patch); return pending.promise; }, { creditsPolicy: 'strict_regular' });
  let tree = render();
  tree.find(n => n.type === 'input')!.props.onChange({ target: { checked: false } });
  tree = render();
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [{ accountId: 'a', allowed: false }]);
  assert.ok(tree.filter(n => n.type === 'input').every(n => n.props.disabled));
  assert.equal(tree.find(n => n.type === 'input')!.props.checked, true);
  pending.resolve({ collection: { ...props.state.collection, accountCreditsAllowed: { a: false } } });
  await settlePromises();
  tree = render();
  assert.equal(tree.find(n => n.type === 'input')!.props.checked, false);
  assert.equal(tree.find(n => n.type === 'select')!.props.value, 'strict_regular');
});

test('failed policy save surfaces error and does not claim the new policy is active', async () => {
  const calls: unknown[] = [];
  const { render } = setup(async patch => { calls.push(patch); throw new Error('gateway restart failed'); });
  render().find(n => n.type === 'select')!.props.onChange('prefer_regular');
  await settlePromises();
  const tree = render();
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [{ policy: 'prefer_regular' }]);
  assert.equal(tree.find(n => n.type === 'select')!.props.value, 'default');
  assert.match(tree.find(n => n.props.role === 'alert')!.props.children, /gateway restart failed/);
});

test('Go compatibility rejection keeps the current policy and account switch visible', async () => {
  const message = 'CREDITS_REQUIRES_NATIVE: Switch API service and related instances to Native first';
  const { render } = setup(async () => { throw new Error(message); }, { gatewayMode: 'sidecar' });
  render().find(n => n.type === 'input')!.props.onChange({ target: { checked: false } });
  await settlePromises();
  const tree = render();
  assert.equal(tree.find(n => n.type === 'select')!.props.value, 'default');
  assert.equal(tree.find(n => n.type === 'input')!.props.checked, true);
  assert.match(tree.find(n => n.props.role === 'alert')!.props.children, /CREDITS_REQUIRES_NATIVE/);
});
