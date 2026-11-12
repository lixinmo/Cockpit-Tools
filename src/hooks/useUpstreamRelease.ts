import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';

const RELEASE_NOTES_URL = 'https://github.com/jlcodes99/cockpit-tools/releases/tag/v';
const VIEWED_VERSION_KEY = 'cockpit-tools.viewed-upstream-release';
const CHECK_INTERVAL_MS = 60 * 60 * 1000;

export interface UpstreamReleaseNotice {
  version: string;
  openReleaseNotes: () => void;
}

export function useUpstreamRelease() {
  const [version, setVersion] = useState<string | null>(null);
  const [viewedVersion, setViewedVersion] = useState(() => {
    try {
      return localStorage.getItem(VIEWED_VERSION_KEY);
    } catch {
      return null;
    }
  });

  useEffect(() => {
    let active = true;
    const check = async () => {
      try {
        const latest = await invoke<string | null>('check_upstream_release');
        if (active) setVersion(latest);
      } catch (error) {
        console.warn('[Upstream release] Could not check GitHub:', error);
      }
    };
    void check();
    const timer = window.setInterval(() => { void check(); }, CHECK_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const openReleaseNotes = useCallback(async () => {
    if (!version) return;
    try {
      await openUrl(`${RELEASE_NOTES_URL}${encodeURIComponent(version)}`);
    } catch (error) {
      console.warn('[Upstream release] Could not open release notes:', error);
      return;
    }
    setViewedVersion(version);
    try {
      localStorage.setItem(VIEWED_VERSION_KEY, version);
    } catch {
      // Keep the notice read for this session if browser storage is unavailable.
    }
  }, [version]);

  const release: UpstreamReleaseNotice | null = version ? { version, openReleaseNotes } : null;
  return { release, unread: version !== null && version !== viewedVersion };
}
