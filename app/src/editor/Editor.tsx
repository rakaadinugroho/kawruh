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
import { collab, collabServiceCtx } from '@milkdown/plugin-collab'
import { useEffect, useRef } from 'react'
import type { DocHandle } from '../lib/docs'
import { newId } from '../lib/workspace'
import { canvasView } from './canvas-view'
import { renderMermaidPreview } from './mermaid'
import { canvasSchema, kawruhSchema } from './schema'

const diagramIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="6" rx="1.5"/><rect x="14" y="15" width="7" height="6" rx="1.5"/><path d="M6.5 9v3.5a2 2 0 0 0 2 2H17.5V15"/></svg>`
const canvasIcon = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19c3-1 4.5-6 8-6s3 4 8 3"/><path d="m14.5 4.5 5 5L11 18l-5.5.5L6 13z"/></svg>`

const MERMAID_SAMPLE = 'graph TD\n  A[Idea] --> B[Draft]\n  B --> C[Share]'

type Props = {
  handle: DocHandle
  readonly: boolean
  onOpenCanvas: (canvasId: string) => void
}

/// Milkdown Crepe bound to the page's Y.Doc (prosemirror fragment + awareness).
export function Editor({ handle, readonly, onOpenCanvas }: Props) {
  const root = useRef<HTMLDivElement>(null)
  const crepeRef = useRef<Crepe | null>(null)
  const openCanvas = useRef(onOpenCanvas)
  openCanvas.current = onOpenCanvas
  // Creation is async; the role may change meanwhile, so read it when done.
  const readonlyRef = useRef(readonly)
  readonlyRef.current = readonly

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
      crepe.editor
        .config((ctx) => {
          // Pasted image files would become blob: URLs nobody else can load.
          ctx.update(uploadConfig.key, (prev) => ({ ...prev, uploader: async () => [] }))
        })
        .use(kawruhSchema)
        .use(canvasView(handle.doc, (id) => openCanvas.current(id)))
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

  return <div ref={root} className="kw-editor" data-testid="editor" />
}
