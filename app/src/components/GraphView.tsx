import { drag } from 'd3-drag'
import {
  type SimulationLinkDatum,
  type SimulationNodeDatum,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
} from 'd3-force'
import { select } from 'd3-selection'
import { type ZoomTransform, zoom, zoomIdentity } from 'd3-zoom'
import { ListTree, PanelLeftOpen, Scan, Waypoints, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { WorkspaceView } from '../lib/hooks'
import { type GraphData, api } from '../lib/ipc'
import { key } from '../lib/platform'
import { useApp } from '../lib/store'
import { Button, IconButton } from './ui'

type Node = SimulationNodeDatum & {
  id: string
  /// Page links in and out; sets the size.
  degree: number
  /// Lines drawn to this node (links, plus tree lines when shown).
  shown: number
  r: number
  /// Zoom from which the label shows without hovering.
  labelAt: number
}

type Edge = SimulationLinkDatum<Node> & { tree: boolean }

type Line = { from: string; to: string; tree: boolean }

/// Ticks before a large graph stops moving even if it has not fully cooled.
const MAX_TICKS = 400
/// Graphs this small label every page.
const ALL_LABELS = 60
const HIERARCHY_KEY = 'kawruh.graph.hierarchy'

function loadHierarchy() {
  try {
    return localStorage.getItem(HIERARCHY_KEY) === '1'
  } catch {
    return false
  }
}

function saveHierarchy(on: boolean) {
  try {
    localStorage.setItem(HIERARCHY_KEY, on ? '1' : '0')
  } catch {
    // Storage may be unavailable; the toggle just won't persist.
  }
}

const label = (title: string, icon: string) => {
  const t = title.trim() || 'Untitled'
  const short = t.length > 36 ? `${t.slice(0, 34)}…` : t
  return icon ? `${icon} ${short}` : short
}

/// Pages as dots and page links as lines. Click a dot to open the page, drag
/// it to move it, scroll or pinch to zoom. d3 owns the SVG's contents; React
/// only rebuilds them when the set of pages or lines changes.
export default function GraphView({
  workspaceId,
  view,
  currentId,
}: {
  workspaceId: string
  view: WorkspaceView
  currentId: string | null
}) {
  const { openPage, showGraph, sidebarOpen, toggleSidebar } = useApp()
  const [data, setData] = useState<GraphData | null>(null)
  const [hierarchy, setHierarchy] = useState(loadHierarchy)
  const svgRef = useRef<SVGSVGElement>(null)
  // Survive rebuilds (refresh, hierarchy toggle) so the graph doesn't jump.
  const positions = useRef(new Map<string, { x: number; y: number }>())
  const camera = useRef<{ touched: boolean; fit: () => void }>({ touched: false, fit: () => {} })
  const currentRef = useRef(currentId)
  currentRef.current = currentId

  const liveKey = useMemo(() => view.live.map((p) => p.id).join(','), [view.live])
  const treeKey = useMemo(() => view.live.map((p) => `${p.id}>${p.parent}`).join(','), [view.live])
  const titles = useMemo(() => new Map(view.live.map((p) => [p.id, label(p.title, p.icon)])), [view.live])
  const titlesRef = useRef(titles)
  titlesRef.current = titles

  // Links come from Rust's page index: on open, when pages come or go, and
  // every so often for links typed meanwhile.
  useEffect(() => {
    let stale = false
    const load = () =>
      api
        .workspaceGraph(workspaceId)
        .then((g) => {
          if (stale) return
          setData((prev) => (prev && JSON.stringify(prev) === JSON.stringify(g) ? prev : g))
        })
        .catch(() => {})
    load()
    const timer = setInterval(load, 20_000)
    return () => {
      stale = true
      clearInterval(timer)
    }
  }, [workspaceId, liveKey])

  const graph = useMemo(() => {
    if (!data) return null
    const ids = data.nodes.map((n) => n.id)
    const known = new Set(ids)
    const lines: Line[] = data.edges.map((e) => ({ ...e, tree: false }))
    if (hierarchy) {
      for (const pair of treeKey.split(',')) {
        const [id, parent] = pair.split('>')
        if (parent && known.has(id) && known.has(parent)) lines.push({ from: parent, to: id, tree: true })
      }
    }
    return { ids, lines, links: data.edges.length }
  }, [data, hierarchy, treeKey])

  useEffect(() => {
    const svg = svgRef.current
    if (!svg || !graph || graph.ids.length === 0) return
    const saved = positions.current
    const resumed = graph.ids.some((id) => saved.has(id))
    const nodes: Node[] = graph.ids.map((id) => ({ id, degree: 0, shown: 0, r: 0, labelAt: 0, ...saved.get(id) }))
    const byId = new Map(nodes.map((n) => [n.id, n]))
    const near = new Map<string, Set<string>>(nodes.map((n) => [n.id, new Set()]))
    for (const l of graph.lines) {
      const a = byId.get(l.from) as Node
      const b = byId.get(l.to) as Node
      a.shown++
      b.shown++
      if (!l.tree) {
        a.degree++
        b.degree++
      }
      near.get(l.from)?.add(l.to)
      near.get(l.to)?.add(l.from)
    }
    for (const n of nodes) {
      n.r = Math.min(4 + Math.sqrt(n.degree) * 2.4, 18)
      n.labelAt = nodes.length <= ALL_LABELS ? 0.3 : 9 / n.r
    }
    const edges: Edge[] = graph.lines.map((l) => ({ source: l.from, target: l.to, tree: l.tree }))

    const root = select(svg)
    root.selectAll('*').remove()
    delete svg.dataset.focus
    const scene = root.append('g')
    const edgeSel = scene
      .append('g')
      .selectAll('line')
      .data(edges)
      .join('line')
      .attr('class', (e) => (e.tree ? 'kw-edge tree' : 'kw-edge'))
    const nodeSel = scene
      .append('g')
      .selectAll<SVGCircleElement, Node>('circle')
      .data(nodes)
      .join('circle')
      .attr('class', (n) => (n.shown ? 'kw-node' : 'kw-node orphan'))
      .attr('data-id', (n) => n.id)
      .attr('r', (n) => n.r)
    const labels = scene.append('g')
    const labelSel = labels
      .selectAll<SVGTextElement, Node>('text')
      .data(nodes)
      .join('text')
      .attr('class', 'kw-label')
      .attr('data-id', (n) => n.id)
      .text((n) => titlesRef.current.get(n.id) ?? 'Untitled')

    let k = 1
    let dragging = false
    let ticks = 0

    const draw = () => {
      edgeSel
        .attr('x1', (e) => (e.source as Node).x ?? 0)
        .attr('y1', (e) => (e.source as Node).y ?? 0)
        .attr('x2', (e) => (e.target as Node).x ?? 0)
        .attr('y2', (e) => (e.target as Node).y ?? 0)
      nodeSel.attr('cx', (n) => n.x ?? 0).attr('cy', (n) => n.y ?? 0)
      // Labels keep a steady on-screen size, growing a little as you zoom in.
      const size = Math.min(Math.max(12 * k ** 0.3, 10.5), 15) / k
      labels.attr('font-size', size).attr('stroke-width', 3 / k)
      labelSel.attr('x', (n) => n.x ?? 0).attr('y', (n) => (n.y ?? 0) + n.r + size * 1.05)
    }

    const showLabels = () => labelSel.classed('hide', (n) => k < n.labelAt && n.id !== currentRef.current)

    const zoomer = zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.08, 4])
      .on('zoom', (ev: { transform: ZoomTransform; sourceEvent: unknown }) => {
        if (ev.sourceEvent) camera.current.touched = true
        const changed = ev.transform.k !== k
        k = ev.transform.k
        scene.attr('transform', ev.transform.toString())
        if (changed) {
          draw()
          showLabels()
        }
      })
    root.call(zoomer).on('dblclick.zoom', null)

    /// The view that frames every node, never closer than 1.5×.
    const fitted = () => {
      const w = svg.clientWidth || 800
      const h = svg.clientHeight || 600
      let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity]
      for (const n of nodes) {
        x0 = Math.min(x0, (n.x ?? 0) - n.r)
        x1 = Math.max(x1, (n.x ?? 0) + n.r)
        y0 = Math.min(y0, (n.y ?? 0) - n.r)
        y1 = Math.max(y1, (n.y ?? 0) + n.r + 18)
      }
      const pad = 56
      const s = Math.max(0.08, Math.min(1.5, (w - pad * 2) / (x1 - x0 || 1), (h - pad * 2) / (y1 - y0 || 1)))
      return zoomIdentity.translate(w / 2 - (s * (x0 + x1)) / 2, h / 2 - (s * (y0 + y1)) / 2).scale(s)
    }
    /// Eases the camera toward framing the graph while it settles.
    const follow = (rate: number) => {
      if (camera.current.touched) return
      const from = root.property('__zoom') as ZoomTransform
      const to = fitted()
      const mix = (a: number, b: number) => a + (b - a) * rate
      root.call(zoomer.transform, zoomIdentity.translate(mix(from.x, to.x), mix(from.y, to.y)).scale(mix(from.k, to.k)))
    }
    camera.current.fit = () => {
      camera.current.touched = false
      let frame = 0
      const step = () => {
        follow(0.25)
        if (++frame < 16 && !camera.current.touched) requestAnimationFrame(step)
      }
      requestAnimationFrame(step)
    }

    const sim = forceSimulation(nodes)
      .force(
        'link',
        forceLink<Node, Edge>(edges)
          .id((n) => n.id)
          .distance((e) => (e.tree ? 46 : 64))
      )
      .force('charge', forceManyBody<Node>().strength(-150).distanceMax(480))
      .force('collide', forceCollide<Node>((n) => n.r + 3))
      .force('x', forceX(0).strength(0.05))
      .force('y', forceY(0).strength(0.05))
      .alphaDecay(0.03)
      .stop()
    if (resumed) sim.alpha(0.35)
    sim.on('tick', () => {
      if (++ticks > MAX_TICKS && !dragging) sim.stop()
      draw()
      follow(0.12)
    })

    // First frame: frame the starting layout at once, then animate.
    const kept = root.property('__zoom') as ZoomTransform
    k = kept.k
    scene.attr('transform', kept.toString())
    if (!camera.current.touched) root.call(zoomer.transform, fitted())
    draw()
    showLabels()
    sim.restart()

    const focus = (id: string | null) => {
      if (!id) {
        delete svg.dataset.focus
        nodeSel.classed('lit', false).classed('hover', false)
        labelSel.classed('lit', false)
        edgeSel.classed('lit', false)
        return
      }
      const lit = near.get(id) ?? new Set()
      svg.dataset.focus = id
      nodeSel.classed('lit', (n) => n.id === id || lit.has(n.id)).classed('hover', (n) => n.id === id)
      labelSel.classed('lit', (n) => n.id === id || lit.has(n.id))
      edgeSel.classed('lit', (e) => (e.source as Node).id === id || (e.target as Node).id === id)
    }

    nodeSel
      .on('pointerenter', (_, n) => !dragging && focus(n.id))
      .on('pointerleave', () => !dragging && focus(null))
      .on('click', (_, n) => openPage(n.id))
      .call(
        drag<SVGCircleElement, Node>()
          .clickDistance(4)
          .on('start', (ev: { active: number }, n) => {
            dragging = true
            camera.current.touched = true
            ticks = 0
            if (!ev.active) sim.alphaTarget(0.25).restart()
            n.fx = n.x
            n.fy = n.y
          })
          .on('drag', (ev: { x: number; y: number }, n) => {
            n.fx = ev.x
            n.fy = ev.y
          })
          .on('end', (ev: { active: number; sourceEvent: PointerEvent }, n) => {
            dragging = false
            if (!ev.active) sim.alphaTarget(0)
            n.fx = null
            n.fy = null
            const over = document.elementFromPoint(ev.sourceEvent.clientX, ev.sourceEvent.clientY)
            focus(over instanceof SVGCircleElement ? (over.dataset.id ?? null) : null)
          })
      )

    const resize = new ResizeObserver(() => follow(1))
    resize.observe(svg)

    return () => {
      sim.stop()
      resize.disconnect()
      root.on('.zoom', null)
      for (const n of nodes) positions.current.set(n.id, { x: n.x ?? 0, y: n.y ?? 0 })
    }
  }, [graph, openPage])

  // Renames, new icons and the open page change in place.
  useEffect(() => {
    const svg = svgRef.current
    if (!svg) return
    select(svg)
      .selectAll<SVGTextElement, Node>('text.kw-label')
      .text((n) => titles.get(n.id) ?? 'Untitled')
  }, [titles, graph])

  useEffect(() => {
    const svg = svgRef.current
    if (!svg) return
    const root = select(svg)
    root.selectAll<SVGCircleElement, Node>('circle.kw-node').classed('current', (n) => n.id === currentId)
    root
      .selectAll<SVGTextElement, Node>('text.kw-label')
      .classed('current', (n) => n.id === currentId)
      .filter((n) => n.id === currentId)
      .classed('hide', false)
  }, [currentId, graph])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !useApp.getState().dialog) showGraph(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [showGraph])

  const toggleHierarchy = () => {
    setHierarchy(!hierarchy)
    saveHierarchy(!hierarchy)
  }

  const pages = data?.nodes.length ?? 0
  const empty = data !== null && pages === 0

  return (
    <main className="flex h-full min-w-0 flex-1 flex-col bg-paper" data-testid="graph-view">
      <header
        data-tauri-drag-region
        className={`flex h-11 shrink-0 items-center gap-2 pr-3 ${sidebarOpen ? 'pl-3' : 'pl-[78px]'}`}
      >
        {!sidebarOpen && (
          <IconButton label={`Show sidebar (${key('\\')})`} onClick={toggleSidebar}>
            <PanelLeftOpen size={15} />
          </IconButton>
        )}
        <div data-tauri-drag-region className="flex min-w-0 flex-1 items-baseline gap-2 px-1 text-[13px]">
          <span className="text-ink">Graph</span>
          {data && pages > 0 && (
            <span className="truncate text-[12px] text-ink-3 tabular-nums">
              {pages} {pages === 1 ? 'page' : 'pages'} · {graph?.links ?? 0}{' '}
              {graph?.links === 1 ? 'link' : 'links'}
            </span>
          )}
        </div>
        {pages > 0 && (
          <>
            <Button
              variant="ghost"
              className={`h-7 px-2.5 ${hierarchy ? 'bg-accent-soft text-ink hover:bg-accent-soft' : ''}`}
              aria-pressed={hierarchy}
              onClick={toggleHierarchy}
              data-testid="graph-hierarchy"
            >
              <ListTree size={14} />
              Show hierarchy
            </Button>
            <IconButton label="Fit to window" onClick={() => camera.current.fit()}>
              <Scan size={15} />
            </IconButton>
          </>
        )}
        <IconButton label="Close graph (Esc)" onClick={() => showGraph(false)} data-testid="graph-close">
          <X size={16} />
        </IconButton>
      </header>

      <div className="relative min-h-0 flex-1">
        {empty ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 pb-24 text-center">
            <Waypoints size={28} strokeWidth={1.5} className="text-ink-3" />
            <p className="font-serif text-[22px] text-ink-2">No pages to connect yet.</p>
            <p className="max-w-[320px] text-[13px] text-ink-3">
              Each page becomes a dot here. Type [[ in a page to link another, and the link shows up as a line.
            </p>
          </div>
        ) : (
          <>
            <svg ref={svgRef} className="kw-graph absolute inset-0 size-full" data-testid="graph-svg" />
            {data && pages > 1 && data.edges.length === 0 && !hierarchy && (
              <p className="pointer-events-none absolute inset-x-0 bottom-5 text-center text-[12px] text-ink-3">
                No links between pages yet. Type [[ in a page to link another.
              </p>
            )}
          </>
        )}
      </div>
    </main>
  )
}
