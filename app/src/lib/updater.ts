/// App updates: signed builds published with each GitHub release
/// (`latest.json`). Checked shortly after launch and every few hours; the
/// sidebar offers to restart into a new version.
import { relaunch } from '@tauri-apps/plugin-process'
import { type Update, check } from '@tauri-apps/plugin-updater'

export type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'available'; version: string }
  | { kind: 'installing'; version: string; percent: number | null }

export const CHECK_EVERY_MS = 6 * 60 * 60 * 1000

// The Update handle isn't serializable state; keep it beside the store.
let pending: Update | null = null

/// The newer version's number, or null when this one is the latest.
export async function findUpdate(): Promise<string | null> {
  pending = await check()
  return pending?.version ?? null
}

/// Downloads and installs the found update, then restarts the app.
export async function installUpdate(onProgress: (percent: number | null) => void) {
  if (!pending) return
  let total = 0
  let done = 0
  await pending.downloadAndInstall((event) => {
    if (event.event === 'Started') total = event.data.contentLength ?? 0
    if (event.event === 'Progress') {
      done += event.data.chunkLength
      onProgress(total ? Math.min(100, Math.round((done / total) * 100)) : null)
    }
  })
  await relaunch()
}
