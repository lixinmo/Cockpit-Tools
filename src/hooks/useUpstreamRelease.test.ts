import assert from 'node:assert/strict';
import test from 'node:test';
import { loadHookModule, settlePromises } from '../../tests/helpers/reactHookHarness';

function setup(savedVersion?: string) {
  const storage = new Map<string, string>();
  if (savedVersion) storage.set('cockpit-tools.viewed-upstream-release', savedVersion);
  const commands: string[] = [];
  const urls: string[] = [];
  let latest: string | null = '1.3.82';
  let poll: () => void = () => {};
  const harness = loadHookModule(new URL('./useUpstreamRelease.ts', import.meta.url), {
    '@tauri-apps/api/core': { async invoke(command: string) { commands.push(command); return latest; } },
    '@tauri-apps/plugin-opener': { async openUrl(url: string) { urls.push(url); } },
  }, {
    localStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    },
    window: {
      setInterval(callback: () => void) { poll = callback; return 1; },
      clearInterval() {},
    },
  });
  harness.render(() => harness.exports.useUpstreamRelease());
  return { ...harness, commands, urls, storage, poll(next: string | null) { latest = next; poll(); } };
}

test('the upstream notice opens only the official release notes and marks that version read', async () => {
  const h = setup();
  await settlePromises();
  assert.equal(h.flush().unread, true);
  assert.equal(h.flush().release.version, '1.3.82');
  await h.flush().release.openReleaseNotes();
  assert.deepEqual(h.urls, ['https://github.com/jlcodes99/cockpit-tools/releases/tag/v1.3.82']);
  assert.deepEqual(h.commands, ['check_upstream_release']);
  assert.equal(h.storage.get('cockpit-tools.viewed-upstream-release'), '1.3.82');
  assert.equal(h.flush().unread, false);
  assert.equal(h.flush().release.version, '1.3.82', 'About still shows the release after it is read');
});

test('a read version does not notify again, but a later official version does', async () => {
  const h = setup('1.3.82');
  await settlePromises();
  assert.equal(h.flush().unread, false);
  h.poll('1.3.83');
  await settlePromises();
  assert.equal(h.flush().unread, true);
  assert.equal(h.flush().release.version, '1.3.83');
  h.poll(null);
  await settlePromises();
  assert.equal(h.flush().release, null);
  assert.equal(h.flush().unread, false);
});
