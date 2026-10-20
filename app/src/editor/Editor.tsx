import { Crepe } from '@milkdown/crepe'
import { commandsCtx, editorViewCtx } from '@milkdown/kit/core'
import type { Ctx } from '@milkdown/kit/ctx'
import { history } from '@milkdown/kit/plugin/history'
import { uploadConfig } from '@milkdown/kit/plugin/upload'
import {
  addBlockTypeCommand,
  clearTextInCurrentBlockCommand,
  codeBlockSchema,
  setBlockTypeCommand,
} from '@milkdown/kit/preset/commonmark'
import type { Node } from '@milkdown/kit/prose/model'
import type { EditorView } from '@milkdown/kit/prose/view'
import { collab, collabServiceCtx } from '@milkdown/plugin-collab'
import { openUrl } from '@tauri-apps/plugin-opener'
import { FileText, Image, Link2, PanelTop, Type } from 'lucide-react'
import { type MouseEvent, useEffect, useMemo, useRef, useState } from 'react'
import { Menu, type MenuItem } from '../components/ui'
import type { DocHandle } from '../lib/docs'
import { api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { type Page, newId } from '../lib/workspace'
import { canvasView } from './canvas-view'
import { ensureMeta, readMeta } from './link-meta'
import { linkCardView } from './link-view'
import { renderMermaidPreview } from './mermaid'
import { insertPageLink, pageIdOf, pageMention, startMention } from './page-link'
import { blockPastedLink, keepPastedLink, pasteLink, titlePastedLink } from './paste-link'
import { type LinkView, canvasSchema, kawruhSchema } from './schema'

const MAX_IMAGE_BYTES = 10 * 1024 * 1024

const diagramIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="6" rx="1.5"/><rect x="14" y="15" width="7" height="6" rx="1.5"/><path d="M6.5 9v3.5a2 2 0 0 0 2 2H17.5V15"/></svg>`
const pageLinkIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="m10 15 4-4"/><path d="M11 11h3v3"/></svg>`
const canvasIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19c3-1 4.5-6 8-6s3 4 8 3"/><path d="m14.5 4.5 5 5L11 18l-5.5.5L6 13z"/></svg>`

const MERMAID_SAMPLE = 'graph TD\n  A[Idea] --> B[Draft]\n  B --> C[Share]'

type Props = {
  handle: DocHandle
  workspaceId: string
  readonly: boolean
  /// Live pages of the workspace, for `[[` links.
  pages: Page[]
  onOpenCanvas: (canvasId: string) => void
  onOpenPage: (pageId: string) => void
}

/// Stores pasted or dropped image files and returns image nodes for them.
async function uploadImages(files: FileList, workspaceId: string, imageNode: (src: string) => Node | null) {
  const { toast, fail } = useApp.getState()
  const nodes: Node[] = []
  for (const file of Array.from(files)) {
    if (!file.type.startsWith('image/')) continue
    if (file.size > MAX_IMAGE_BYTES) {
      toast(`${file.name || 'That image'} is over 10 MB.`, 'error')
      continue
    }
    try {
      const src = await api.assetPut(workspaceId, new Uint8Array(await file.arrayBuffer()), file.type)
      const node = imageNode(src)
      if (node) nodes.push(node)
    } catch (e) {
      fail(e)
    }
  }
  return nodes
}

/// Milkdown Crepe bound to the page's Y.Doc (prosemirror fragment + awareness).
export function Editor({ handle, workspaceId, readonly, pages, onOpenCanvas, onOpenPage }: Props) {
  const root = useRef<HTMLDivElement>(null)
  const crepeRef = useRef<Crepe | null>(null)
  const openCanvas = useRef(onOpenCanvas)
  openCanvas.current = onOpenCanvas
  // Creation is async; the role may change meanwhile, so read it when done.
  const readonlyRef = useRef(readonly)
  readonlyRef.current = readonly
  const workspaceRef = useRef(workspaceId)
  workspaceRef.current = workspaceId
  const [pasteMenu, setPasteMenu] = useState<{ href: string; x: number; y: number } | null>(null)
  const [mention, setMention] = useState<{ query: string; x: number; y: number } | null>(null)
  const [mentionActive, setMentionActive] = useState(0)
  const mentionKey = useRef<(key: 'ArrowUp' | 'ArrowDown' | 'Enter' | 'Escape') => boolean>(() => false)

  useEffect(() => {
    if (!root.current) return
    const crepe = new Crepe({
      root: root.current,
      defaultValue: '',
      features: {
        [Crepe.Feature.ImageBlock]: false,
        [Crepe.Feature.Latex]: false,
      },
      featureConfigs: {
        [Crepe.Feature.Placeholder]: {
          text: "Write something, or press '/' for blocks",
          mode: 'block',
        },
        [Crepe.Feature.CodeMirror]: {
          renderPreview: renderMermaidPreview,
          previewOnlyByDefault: true,
          previewLabel: 'Diagram',
        },
        [Crepe.Feature.BlockEdit]: {
          buildMenu: (builder) => {
            builder
              .addGroup('kawruh', 'Visual')
              .addItem('mermaid', {
                label: 'Mermaid diagram',
                icon: diagramIcon,
                onRun: (ctx) => {
                  const commands = ctx.get(commandsCtx)
                  commands.call(clearTextInCurrentBlockCommand.key)
                  commands.call(setBlockTypeCommand.key, {
                    nodeType: codeBlockSchema.type(ctx),
                    attrs: { language: 'mermaid' },
                  })
                  const view = ctx.get(editorViewCtx)
                  view.dispatch(view.state.tr.insertText(MERMAID_SAMPLE))
                },
              })
              .addItem('page-link', {
                label: 'Link to page',
                icon: pageLinkIcon,
                onRun: (ctx: Ctx) => {
                  ctx.get(commandsCtx).call(clearTextInCurrentBlockCommand.key)
                  startMention(ctx.get(editorViewCtx))
                },
              })
              .addItem('canvas', {
                label: 'Canvas drawing',
                icon: canvasIcon,
                onRun: (ctx: Ctx) => {
                  const id = newId()
                  const commands = ctx.get(commandsCtx)
                  commands.call(clearTextInCurrentBlockCommand.key)
                  commands.call(addBlockTypeCommand.key, {
                    nodeType: canvasSchema.type(ctx),
                    attrs: { id },
                  })
                  openCanvas.current(id)
                },
              })
          },
        },
      },
    })
    crepeRef.current = crepe
    let disposed = false

    const ready = (async () => {
      // Undo/redo comes from Yjs (only your own edits); ProseMirror's history
      // would also undo collaborators' changes.
      await crepe.editor.remove(history)
      const canWrite = () => !readonlyRef.current
      crepe.editor
        .config((ctx) => {
          // Pasted and dropped images go to the asset store, not blob: URLs
          // nobody else could load.
          ctx.update(uploadConfig.key, (prev) => ({
            ...prev,
            uploader: (files, schema) =>
              uploadImages(files, workspaceRef.current, (src) =>
                schema.nodes.image.createAndFill({ src, alt: '' })
              ),
          }))
        })
        .use(kawruhSchema)
        .use(canvasView(handle.doc, (id) => openCanvas.current(id)))
        .use(linkCardView(handle.doc, canWrite))
        .use(
          pasteLink({
            onPaste: (p) => {
              setPasteMenu(p)
              void ensureMeta(handle.doc, p.href, canWrite())
            },
            onDone: () => setPasteMenu(null),
          })
        )
        .use(
          pageMention({
            onChange: (m) => {
              setMention((prev) => {
                if (!m || prev?.query !== m.query) setMentionActive(0)
                return m
              })
            },
            onKey: (key) => mentionKey.current(key),
          })
        )
        .use(collab)
      await crepe.create()
      if (disposed) return
      crepe.editor.action((ctx) => {
        ctx
          .get(collabServiceCtx)
          .bindDoc(handle.doc)
          .setAwareness(handle.awareness)
          .connect()
      })
      crepe.setReadonly(readonlyRef.current)
      // Dev builds expose the editor to the E2E automation hook.
      if (import.meta.env.DEV) (window as unknown as { __kawruhEditor: Crepe }).__kawruhEditor = crepe
    })()

    return () => {
      disposed = true
      crepeRef.current = null
      ready.finally(() => crepe.destroy())
    }
    // The editor is rebuilt only for a different document.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handle])

  useEffect(() => {
    crepeRef.current?.setReadonly(readonly)
  }, [readonly])

  const withView = (fn: (view: EditorView, ctx: Ctx) => void) =>
    crepeRef.current?.editor.action((ctx) => fn(ctx.get(editorViewCtx), ctx))

  const asBlock = (kind: LinkView) =>
    withView((view, ctx) => {
      const href = pasteMenu?.href ?? ''
      blockPastedLink(view, ctx, kind, readMeta(handle.doc, href)?.title ?? '')
    })

  const pasteItems: MenuItem[] = [
    { label: 'Link', icon: <Link2 size={14} />, onSelect: () => withView(keepPastedLink) },
    {
      label: 'Link with title',
      icon: <Type size={14} />,
      onSelect: () => {
        const href = pasteMenu?.href ?? ''
        void ensureMeta(handle.doc, href, !readonly).then((meta) => {
          if (!meta?.title) useApp.getState().toast("Couldn't read that page's title.")
          withView((view) => titlePastedLink(view, meta?.title ?? ''))
        })
      },
    },
    { label: 'Card', icon: <PanelTop size={14} />, onSelect: () => asBlock('card') },
    { label: 'Preview', icon: <Image size={14} />, onSelect: () => asBlock('preview') },
  ]

  const mentionMatches = useMemo(() => {
    if (!mention) return []
    const q = mention.query.trim().toLowerCase()
    return pages
      .filter((p) => p.id !== handle.docId && (p.title || 'untitled').toLowerCase().includes(q))
      .sort((a, b) => {
        const ai = (a.title || 'untitled').toLowerCase().indexOf(q)
        const bi = (b.title || 'untitled').toLowerCase().indexOf(q)
        return ai - bi || b.created - a.created
      })
      .slice(0, 8)
  }, [mention, pages, handle.docId])

  const chooseMention = (page: Page) => withView((view, ctx) => insertPageLink(view, ctx, page))

  mentionKey.current = (key) => {
    if (key === 'Escape') {
      setMention(null)
      return true
    }
    if (mentionMatches.length === 0) return false
    if (key === 'ArrowDown') setMentionActive((a) => (a + 1) % mentionMatches.length)
    else if (key === 'ArrowUp') setMentionActive((a) => (a - 1 + mentionMatches.length) % mentionMatches.length)
    else chooseMention(mentionMatches[Math.min(mentionActive, mentionMatches.length - 1)])
    return true
  }

  /// Page links open the page. Other links open in the browser on ⌘/Ctrl-click
  /// (or any click when the page is read-only). Cards handle their own clicks.
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = (e.target as HTMLElement).closest('a[href]')
    if (!a || a.closest('.kw-link')) return
    const href = a.getAttribute('href') ?? ''
    const pageId = pageIdOf(href)
    if (pageId) {
      e.preventDefault()
      if (pages.some((p) => p.id === pageId)) onOpenPage(pageId)
      else useApp.getState().toast("That page isn't in this workspace any more.")
      return
    }
    if (!/^(https?:|mailto:)/i.test(href) || !(e.metaKey || e.ctrlKey || readonly)) return
    e.preventDefault()
    void openUrl(href)
  }

  return (
    <>
      <div ref={root} className="kw-editor" data-testid="editor" onClick={onClick} />
      {mention && (
        <div
          role="listbox"
          aria-label="Link to page"
          className="fixed z-50 w-64 rounded-lg bg-raised p-1 shadow-pop"
          style={{ left: Math.min(mention.x, window.innerWidth - 264), top: mention.y }}
          onMouseDown={(e) => e.preventDefault()}
        >
          {mentionMatches.length === 0 ? (
            <p className="px-2 py-1.5 text-[12px] text-ink-3">No pages match “{mention.query}”</p>
          ) : (
            mentionMatches.map((p, i) => (
              <button
                key={p.id}
                type="button"
                role="option"
                aria-selected={i === mentionActive}
                onMouseEnter={() => setMentionActive(i)}
                onClick={() => chooseMention(p)}
                className={`flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] ${
                  i === mentionActive ? 'bg-hover' : ''
                }`}
              >
                <span className="flex w-4 justify-center text-[13px] text-ink-3">
                  {p.icon || <FileText size={13} />}
                </span>
                <span className={`flex-1 truncate ${p.title ? 'text-ink' : 'text-ink-3'}`}>
                  {p.title || 'Untitled'}
                </span>
              </button>
            ))
          )}
        </div>
      )}
      <Menu
        at={pasteMenu}
        items={pasteItems}
        onClose={() => setPasteMenu(null)}
        width={200}
        initialActive={0}
        instant
      />
    </>
  )
}
