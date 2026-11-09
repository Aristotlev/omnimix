// In-app updates through the Tauri updater (signed releases on GitHub).
import type { Update } from '@tauri-apps/plugin-updater';

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export async function checkForUpdate(): Promise<Update | null> {
  if (!inTauri) return null;
  const { check } = await import('@tauri-apps/plugin-updater');
  return await check();
}

/** Download, hand audio back to the OS, install, relaunch. */
export async function installUpdate(u: Update, onProgress: (fraction: number | null) => void): Promise<void> {
  let total = 0;
  let done = 0;
  await u.download((ev) => {
    if (ev.event === 'Started') total = ev.data.contentLength ?? 0;
    else if (ev.event === 'Progress') {
      done += ev.data.chunkLength;
      onProgress(total ? Math.min(1, done / total) : null);
    }
  });
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('prepare_update');
  await u.install();
  const { relaunch } = await import('@tauri-apps/plugin-process');
  await relaunch();
}
