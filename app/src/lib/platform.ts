/// Wording that differs between macOS and Windows: the device name and shortcut keys.
export const isMac = navigator.userAgent.includes('Mac')

/// "this Mac" / "this computer", for copy like "saved on this Mac".
export const thisDevice = isMac ? 'this Mac' : 'this computer'

/// Shortcut label: key('K') is "⌘K" on macOS and "Ctrl+K" elsewhere.
export const key = (k: string) => (isMac ? `⌘${k}` : `Ctrl+${k}`)

/// The graph view's shortcut label (⌘⇧G / Ctrl+Shift+G).
export const graphKey = key(isMac ? '⇧G' : 'Shift+G')
