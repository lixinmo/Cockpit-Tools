import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { PlatformLayoutModal } from '../../src/components/PlatformLayoutModal';
import enResources from '../../src/locales/en.json';
import { usePlatformLayoutStore } from '../../src/stores/usePlatformLayoutStore';
import '../../src/App.css';
const snapshot = { values: {}, revisions: {} };
let writes = 0, closed = 0;
window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  transformCallback: () => 1, unregisterCallback: () => {},
  invoke: async (command, args) => {
    if (command === 'load_ui_preferences') return structuredClone(snapshot);
    if (command === 'update_ui_preference') {
      snapshot.values[args.key] = args.value;
      snapshot.revisions[args.key] = (snapshot.revisions[args.key] || 0) + 1;
      if (args.key === 'agtools.platform_layout.v1') writes++;
      return structuredClone(snapshot);
    }
    if (command === 'plugin:event|listen') return 1;
    return {};
  },
};
usePlatformLayoutStore.getState().upsertPlatformGroup({
  id: 'fixture-sort-group', name: 'Fixture group',
  platformIds: ['qoder', 'codebuddy', 'codebuddy_cn'], defaultPlatformId: 'qoder',
  iconKind: 'platform', iconPlatformId: 'qoder',
});
window.fixture = {
  get writes() { return writes; },
  get closed() { return closed; },
  get order() { return usePlatformLayoutStore.getState().orderedEntryIds; },
  get groupOrder() { return usePlatformLayoutStore.getState().platformGroups.find(g => g.id === 'fixture-sort-group').platformIds; },
};
function Fixture() {
  const [open, setOpen] = useState(true);
  return <PlatformLayoutModal open={open} onClose={() => { closed++; setOpen(false); }} />;
}
const i18n = createInstance();
void i18n.init({
  lng: 'en', fallbackLng: false, initAsync: false,
  resources: { en: { translation: { platformLayout: enResources.platformLayout } } },
});
createRoot(document.getElementById('root')!).render(
  <React.StrictMode><I18nextProvider i18n={i18n}><Fixture /></I18nextProvider></React.StrictMode>,
);
