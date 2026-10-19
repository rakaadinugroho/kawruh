/// Pasted images live in Kawruh's asset store, not in the document. Markdown
/// refers to them as `kawruh-asset://<workspace>/<sha256>.<ext>`; the webview
/// loads them through the `kawruh-asset` protocol served by Rust.
import { convertFileSrc } from '@tauri-apps/api/core'

export const ASSET_PREFIX = 'kawruh-asset://'

/// URLs the webview uses for the protocol: macOS/Linux and Windows spell it differently.
const SERVED = [/^kawruh-asset:\/\/localhost\//, /^https?:\/\/kawruh-asset\.localhost\//]

/// The URL an <img> can load for a stored `src`.
export function displaySrc(src: string): string {
  if (!src.startsWith(ASSET_PREFIX)) return src
  return convertFileSrc(src.slice(ASSET_PREFIX.length), 'kawruh-asset')
}

/// Back from a served URL (e.g. HTML copied out of the editor) to the stored form.
export function canonicalSrc(src: string): string {
  for (const re of SERVED) {
    if (re.test(src)) return ASSET_PREFIX + decodeURIComponent(src.replace(re, ''))
  }
  return src
}
