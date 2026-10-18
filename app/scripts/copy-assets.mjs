// Copies Excalidraw's fonts into public/ so canvases render offline.
// Xiaolai (12 MB, CJK handwriting) is skipped; CJK text falls back to system fonts.
import { cpSync, existsSync, mkdirSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = resolve(here, '../node_modules/@excalidraw/excalidraw/dist/prod/fonts')
const dest = resolve(here, '../public/excalidraw/fonts')
if (!existsSync(src)) {
  console.error('excalidraw fonts not found; run pnpm install')
  process.exit(1)
}
mkdirSync(dest, { recursive: true })
for (const family of readdirSync(src)) {
  if (family === 'Xiaolai') continue
  cpSync(join(src, family), join(dest, family), { recursive: true })
}
