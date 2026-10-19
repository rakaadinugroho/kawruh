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
import { Image, Link2, PanelTop, Type } from 'lucide-react'
import { type MouseEvent, useEffect, useRef, useState } from 'react'
import { Menu, type MenuItem } from '../components/ui'
import type { DocHandle } from '../lib/docs'
import { api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { newId } from '../lib/workspace'
import { canvasView } from './canvas-view'
import { ensureMeta, readMeta } from './link-meta'
import { linkCardView } from './link-view'
import { renderMermaidPreview } from './mermaid'
import { blockPastedLink, keepPastedLink, pasteLink, titlePastedLink } from './paste-link'
import { type LinkView, canvasSchema, kawruhSchema } from './schema'

const MAX_IMAGE_BYTES = 10 * 1024 * 1024

const diagramIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="6" rx="1.5"/><rect x="14" y="15" width="7" height="6" rx="1.5"/><path d="M6.5 9v3.5a2 2 0 0 0 2 2H17.5V15"/></svg>`
const canvasIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19c3-1 4.5-6 8-6s3 4 8 3"/><path d="m14.5 4.5 5 5L11 18l-5.5.5L6 13z"/></svg>`

const MERMAID_SAMPLE = 'graph TD\n  A[Idea] --> B[Draft]\n  B --> C[Share]'

type Props = {
  handle: DocHandle
  workspaceId: string
  readonly: boolean
  onOpenCanvas: (canvasId: string) => void
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
export function Editor({ handle, workspaceId, readonly, onOpenCanvas }: Props) {
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

  /// Links in the text open in the browser on ⌘/Ctrl-click (or any click when
  /// the page is read-only). Cards handle their own clicks.
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = (e.target as HTMLElement).closest('a[href]')
    if (!a || a.closest('.kw-link')) return
    const href = a.getAttribute('href') ?? ''
    if (!/^(https?:|mailto:)/i.test(href) || !(e.metaKey || e.ctrlKey || readonly)) return
    e.preventDefault()
    void openUrl(href)
  }

  return (
    <>
      <div ref={root} className="kw-editor" data-testid="editor" onClick={onClick} />
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
