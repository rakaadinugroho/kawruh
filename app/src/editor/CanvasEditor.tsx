import { Excalidraw, exportToSvg, reconcileElements } from '@excalidraw/excalidraw'
import '@excalidraw/excalidraw/index.css'
import type {
  AppState,
  ExcalidrawImperativeAPI,
} from '@excalidraw/excalidraw/types'
import type { OrderedExcalidrawElement } from '@excalidraw/excalidraw/element/types'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import * as Y from 'yjs'
import { canvasPreviews } from './canvas-view'

/// Excalidraw loads its fonts from here (copied by scripts/copy-assets.mjs).
;(window as unknown as { EXCALIDRAW_ASSET_PATH: string }).EXCALIDRAW_ASSET_PATH = '/excalidraw/'

const ORIGIN = 'canvas'

type Element = OrderedExcalidrawElement

type Props = {
  doc: Y.Doc
  canvasId: string
  readonly: boolean
  onClose: () => void
}

const sceneMap = (doc: Y.Doc, id: string) => doc.getMap<string>(`canvas:${id}`)

function readScene(map: Y.Map<string>): Element[] {
  const out: Element[] = []
  map.forEach((json) => {
    try {
      out.push(JSON.parse(json))
    } catch {
      // A malformed element from a foreign writer is skipped, not fatal.
    }
  })
  // Fractional indices compare by code unit (Excalidraw's order), not locale:
  // localeCompare would put "aa" before "aZ" and reshuffle layers on open.
  return out.sort((a, b) => {
    const x = a.index ?? ''
    const y = b.index ?? ''
    return x < y ? -1 : x > y ? 1 : 0
  })
}

/// Full-window canvas. Each Excalidraw element is one entry of the page doc's
/// `canvas:<id>` map, so concurrent edits merge per element (higher version wins).
export default function CanvasEditor({ doc, canvasId, readonly, onClose }: Props) {
  const map = useMemo(() => sceneMap(doc, canvasId), [doc, canvasId])
  const api = useRef<ExcalidrawImperativeAPI | null>(null)
  const versions = useRef(new Map<string, number>())
  const previewTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const dark = window.matchMedia('(prefers-color-scheme: dark)').matches

  const initial = useMemo(() => {
    const elements = readScene(map)
    elements.forEach((e) => versions.current.set(e.id, e.version))
    return { elements, scrollToContent: true }
  }, [map])

  const writePreview = useCallback(async () => {
    const excal = api.current
    if (!excal || readonly) return
    const elements = excal.getSceneElements()
    const previews = canvasPreviews(doc)
    if (elements.length === 0) {
      if (previews.has(canvasId)) previews.delete(canvasId)
      return
    }
    const svg = await exportToSvg({
      elements,
      appState: { exportBackground: false, exportWithDarkMode: false, exportPadding: 16 },
      files: excal.getFiles(),
    })
    previews.set(canvasId, svg.outerHTML)
  }, [doc, canvasId, readonly])

  const onChange = useCallback(
    (elements: readonly Element[], _state: AppState) => {
      if (readonly) return
      const changed = elements.filter((e) => versions.current.get(e.id) !== e.version)
      // Elements Excalidraw drops without marking deleted (a cancelled arrow,
      // a zero-size shape) must be tombstoned, or they come back on reopen.
      const present = new Set(elements.map((e) => e.id))
      const vanished = [...versions.current.keys()].filter((id) => !present.has(id))
      if (changed.length === 0 && vanished.length === 0) return
      doc.transact(() => {
        for (const e of changed) {
          versions.current.set(e.id, e.version)
          map.set(e.id, JSON.stringify(e))
        }
        for (const id of vanished) {
          versions.current.delete(id)
          const json = map.get(id)
          if (!json) continue
          const el = JSON.parse(json) as Element
          if (!el.isDeleted) {
            map.set(id, JSON.stringify({ ...el, isDeleted: true, version: el.version + 1 }))
          }
        }
      }, ORIGIN)
      if (previewTimer.current) clearTimeout(previewTimer.current)
      previewTimer.current = setTimeout(() => void writePreview(), 1200)
    },
    [doc, map, readonly, writePreview]
  )

  // Collaborators' (and MCP agents') changes flow into the open scene.
  useEffect(() => {
    const observer = (_e: Y.YMapEvent<string>, tr: Y.Transaction) => {
      if (tr.origin === ORIGIN) return
      const excal = api.current
      if (!excal) return
      const remote = readScene(map)
      remote.forEach((e) => versions.current.set(e.id, e.version))
      const merged = reconcileElements(
        excal.getSceneElementsIncludingDeleted(),
        remote as unknown as Parameters<typeof reconcileElements>[1],
        excal.getAppState()
      )
      excal.updateScene({ elements: merged })
    }
    map.observe(observer)
    return () => map.unobserve(observer)
  }, [map])

  const close = useCallback(async () => {
    if (previewTimer.current) clearTimeout(previewTimer.current)
    await writePreview()
    onClose()
  }, [writePreview, onClose])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && (e.metaKey || e.ctrlKey)) void close()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [close])

  return (
    <div className="fixed inset-0 z-40 flex flex-col bg-paper" data-testid="canvas-editor">
      <div
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center justify-between border-b border-line pr-3 pl-20"
      >
        <span data-tauri-drag-region className="text-[13px] font-medium text-ink-2">
          Canvas{readonly ? ' · view only' : ''}
        </span>
        <button
          type="button"
          onClick={() => void close()}
          className="press rounded-md bg-accent px-3 py-1.5 text-[12px] font-medium text-accent-ink"
        >
          Done
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <Excalidraw
          excalidrawAPI={(a) => {
            api.current = a
          }}
          initialData={initial}
          onChange={onChange}
          viewModeEnabled={readonly}
          theme={dark ? 'dark' : 'light'}
          UIOptions={{
            canvasActions: { loadScene: false, saveToActiveFile: false, export: false },
          }}
        />
      </div>
    </div>
  )
}
