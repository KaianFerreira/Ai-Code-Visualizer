<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import ForceGraph from 'force-graph'
import { Search, X } from 'lucide-vue-next'

/** Enriched graph JSON shape (subset). */
interface EnrichedFile {
  id?: string
  path: string
  name: string
  directory?: string
  relative_path?: string
  depth?: number
  folder_group?: string
  line_count: number
  functions: string[]
  classes: string[]
  metadata?: {
    layer?: string
    security_priority?: string
  }
}

interface GraphEdge {
  source: string
  target: string
  edge_type?: string
}

interface EnrichedPayload {
  graph: {
    files: EnrichedFile[]
    edges: GraphEdge[]
  }
}

function normalizeGraphJson(raw: unknown): EnrichedPayload {
  if (!raw || typeof raw !== 'object') {
    throw new Error('Invalid JSON: expected an object')
  }
  const r = raw as Record<string, unknown>

  const graph = r['graph']
  if (graph && typeof graph === 'object') {
    const g = graph as Record<string, unknown>
    const files = g['files']
    const edges = g['edges']
    if (Array.isArray(files)) {
      return {
        graph: {
          files: files as EnrichedFile[],
          edges: Array.isArray(edges) ? (edges as GraphEdge[]) : [],
        },
      }
    }
  }

  const nodes = r['nodes']
  const edgesTop = r['edges']
  if (Array.isArray(nodes)) {
    return {
      graph: {
        files: nodes as EnrichedFile[],
        edges: Array.isArray(edgesTop) ? (edgesTop as GraphEdge[]) : [],
      },
    }
  }

  throw new Error('Invalid graph JSON: expected graph.files or top-level nodes array')
}

interface GraphNode {
  id: string
  name: string
  path: string
  line_count: number
  layer: string
  security_priority: string
  functions: string[]
  classes: string[]
  /** Folder depth from repo root (0 = file at root). */
  depth: number
  /** Clustering key: parent folder path, `"."` at repo root. */
  folder_group: string
  x?: number
  y?: number
}

/** force-graph link; `violation` = UI → Infrastructure import (architectural smell). */
interface GraphLink {
  source: string
  target: string
  violation?: boolean
}

/** Cyberpunk professional palette (layer → neon). */
const LAYER_COLORS: Record<string, string> = {
  Application: '#00e5ff',
  UI: '#f472b6',
  Infrastructure: '#c026fc',
  Domain: '#34d399',
  reference: '#64748b',
  unknown: '#94a3b8',
}

const LAYER_DESCRIPTIONS: Record<string, string> = {
  Infrastructure:
    'Electric-purple tier: persistence, SDKs, env/config, and raw I/O that other layers consume.',
  Domain:
    'Emerald business core: entities, invariants, and domain language with minimal framework noise.',
  Application:
    'Neon-cyan orchestration: use-cases and services wiring domain rules to infrastructure and UI.',
  UI: 'Magenta-hot presentation shell: views, routing, and components users touch directly.',
  reference: 'External or generated leaf referenced by the graph without full workspace analysis.',
  unknown: 'Layer not assigned by the architect agent.',
}

const graphHost = ref<HTMLElement | null>(null)
const graphInstance = shallowRef<InstanceType<typeof ForceGraph> | null>(null)

const loadError = ref<string | null>(null)
/** True until data is loaded, graph initialized, and viewport/layout have settled (no flicker). */
const bootstrapping = ref(true)

const searchQuery = ref('')
const hoveredBlastId = ref<string | null>(null)
const selectedNode = ref<GraphNode | null>(null)

const nodeById = shallowRef<Map<string, GraphNode>>(new Map())
const outDegree = shallowRef<Map<string, number>>(new Map())
const inDegree = shallowRef<Map<string, number>>(new Map())
let adjacency: Map<string, Set<string>> = new Map()

function displayPath(p: string): string {
  return p.replace(/^\\\\\?\\/i, '').replace(/\\/g, '/')
}

function layerColor(layer: string): string {
  return LAYER_COLORS[layer] ?? LAYER_COLORS['unknown']!
}

function layerDescription(layer: string): string {
  return LAYER_DESCRIPTIONS[layer] ?? LAYER_DESCRIPTIONS['unknown']!
}

function nodeSize(node: GraphNode): number {
  return Math.max(3.2, Math.sqrt(Math.max(1, node.line_count)) * 0.48)
}

function isSecurityRiskNode(node: GraphNode): boolean {
  return node.security_priority === 'high' || node.security_priority === 'critical'
}

function linkEndpointId(endpoint: string | number | GraphNode | undefined): string {
  if (endpoint === undefined || endpoint === null) return ''
  if (typeof endpoint === 'object' && 'id' in endpoint) {
    return String((endpoint as GraphNode).id)
  }
  return String(endpoint)
}

function linkAsGraphLink(link: { source?: unknown; target?: unknown }): GraphLink {
  return {
    source: linkEndpointId(link.source as string | GraphNode),
    target: linkEndpointId(link.target as string | GraphNode),
    violation: !!(link as GraphLink).violation,
  }
}

function buildAdjacency(links: GraphLink[]): Map<string, Set<string>> {
  const m = new Map<string, Set<string>>()
  const add = (a: string, b: string) => {
    if (!m.has(a)) m.set(a, new Set())
    if (!m.has(b)) m.set(b, new Set())
    m.get(a)!.add(b)
    m.get(b)!.add(a)
  }
  for (const l of links) {
    add(l.source, l.target)
  }
  return m
}

function buildDegreeMaps(links: GraphLink[]): {
  out: Map<string, number>
  inn: Map<string, number>
} {
  const out = new Map<string, number>()
  const inn = new Map<string, number>()
  for (const l of links) {
    out.set(l.source, (out.get(l.source) ?? 0) + 1)
    inn.set(l.target, (inn.get(l.target) ?? 0) + 1)
  }
  return { out, inn }
}

/** Heuristic 0–100 “maintainability” from structure (client-side audit proxy). */
function maintainabilityScore(node: GraphNode, out: number, inn: number): number {
  let s = 94
  s -= Math.min(20, Math.max(0, node.functions.length - 12) * 1.1)
  s -= Math.min(24, Math.max(0, out - 3) * 2.2)
  s -= Math.min(14, Math.max(0, inn - 5) * 1.1)
  if (isSecurityRiskNode(node)) s -= 14
  if (node.layer === 'UI' && out > 10) s -= 6
  if (node.line_count > 400) s -= 8
  return Math.max(0, Math.min(100, Math.round(s)))
}

/** Two-sentence narrative from graph signals (no live LLM — reads like an audit brief). */
function fileCodeSummary(node: GraphNode, out: number, inn: number): string {
  const tier =
    node.layer === 'Infrastructure'
      ? 'systems edge'
      : node.layer === 'Domain'
        ? 'business core'
        : node.layer === 'Application'
          ? 'application orchestration'
          : node.layer === 'UI'
            ? 'presentation surface'
            : 'general module'
  const s1 = `${node.name} anchors the ${tier} (${node.layer}): it concentrates ${node.functions.length} callable unit(s) and ${node.classes.length} type definition(s) within ${node.line_count} lines, positioning it as a hinge between upstream consumers and downstream dependencies.`
  const s2 = `Structural coupling shows ${out} outbound import edge(s) and ${inn} inbound dependenc(ies); ${out > inn ? 'it pushes complexity outward into the graph' : inn > out ? 'it absorbs pressure from many callers' : 'influence is roughly balanced'}, which ${maintainabilityScore(node, out, inn) >= 70 ? 'supports sustainable evolution' : 'warrants refactors to reduce entanglement'} in a production codebase.`
  return `${s1} ${s2}`
}

const selectedHealth = computed(() => {
  const n = selectedNode.value
  if (!n) return null
  const out = outDegree.value.get(n.id) ?? 0
  const inn = inDegree.value.get(n.id) ?? 0
  return {
    score: maintainabilityScore(n, out, inn),
    summary: fileCodeSummary(n, out, inn),
    out,
    inn,
  }
})

function matchesSearch(node: GraphNode, q: string): boolean {
  const n = q.trim().toLowerCase()
  if (!n) return false
  return (
    node.name.toLowerCase().includes(n) ||
    node.path.toLowerCase().includes(n) ||
    displayPath(node.path).toLowerCase().includes(n)
  )
}

function blastActiveNodeIds(hoverId: string | null): Set<string> | null {
  if (!hoverId) return null
  const out = new Set<string>([hoverId])
  const nbr = adjacency.get(hoverId)
  if (nbr) {
    for (const x of nbr) out.add(x)
  }
  return out
}

function nodeDimmed(node: GraphNode): number {
  const blast = blastActiveNodeIds(hoveredBlastId.value)
  const q = searchQuery.value.trim()
  if (blast) {
    return blast.has(node.id) ? 1 : 0.08
  }
  if (q) {
    return matchesSearch(node, q) ? 1 : 0.2
  }
  return 1
}

function linkDimmed(link: { source?: unknown; target?: unknown }): number {
  const s = linkEndpointId(link.source as string | GraphNode)
  const t = linkEndpointId(link.target as string | GraphNode)
  const blast = hoveredBlastId.value
  if (blast) {
    return s === blast || t === blast ? 1 : 0.05
  }
  const q = searchQuery.value.trim()
  if (q) {
    const a = nodeById.value.get(s)
    const b = nodeById.value.get(t)
    const hit =
      (a && matchesSearch(a, q)) ||
      (b && matchesSearch(b, q)) ||
      s.toLowerCase().includes(q.toLowerCase()) ||
      t.toLowerCase().includes(q.toLowerCase())
    return hit ? 0.85 : 0.12
  }
  return 0.45
}

function hexToRgba(hex: string, alpha: number): string {
  const h = hex.replace('#', '')
  const r = Number.parseInt(h.slice(0, 2), 16)
  const g = Number.parseInt(h.slice(2, 4), 16)
  const b = Number.parseInt(h.slice(4, 6), 16)
  return `rgba(${r},${g},${b},${alpha})`
}

/** Flat-top hexagon path centered at (x,y). */
/** Matches backend `folder_hierarchy_from_relative_path` for older JSON without these fields. */
function folderHierarchyFromRelativePath(relativePath: string): { depth: number; folder_group: string } {
  const normalized = relativePath.trim().replace(/\\/g, '/')
  const pathOnly = normalized.replace(/^\/+/, '').replace(/\/+$/, '')
  if (!pathOnly) return { depth: 0, folder_group: '.' }
  const idx = pathOnly.lastIndexOf('/')
  if (idx === -1) return { depth: 0, folder_group: '.' }
  const parent = pathOnly.slice(0, idx).trim().replace(/\/+$/, '')
  if (!parent) return { depth: 0, folder_group: '.' }
  const depth = parent.split('/').filter(Boolean).length
  return { depth, folder_group: parent }
}

function resolveFolderHierarchy(f: EnrichedFile): { depth: number; folder_group: string } {
  if (f.depth !== undefined && f.folder_group !== undefined && f.folder_group !== '') {
    return { depth: f.depth, folder_group: f.folder_group }
  }
  if (f.relative_path?.trim()) {
    const d = folderHierarchyFromRelativePath(f.relative_path)
    if (f.depth !== undefined) d.depth = f.depth
    if (f.folder_group !== undefined && f.folder_group !== '') d.folder_group = f.folder_group
    return d
  }
  const dir = (f.directory ?? '').replace(/\\/g, '/').replace(/\/+$/, '').trim()
  if (!dir) return { depth: 0, folder_group: '.' }
  return { depth: dir.split('/').filter(Boolean).length, folder_group: dir }
}

function hexPath(ctx: CanvasRenderingContext2D, x: number, y: number, R: number): void {
  ctx.beginPath()
  for (let i = 0; i < 6; i++) {
    const a = (Math.PI / 180) * (60 * i - 90)
    const px = x + R * Math.cos(a)
    const py = y + R * Math.sin(a)
    if (i === 0) ctx.moveTo(px, py)
    else ctx.lineTo(px, py)
  }
  ctx.closePath()
}

function parseGraphPayload(data: EnrichedPayload): { nodes: GraphNode[]; links: GraphLink[] } {
  const nodes: GraphNode[] = data.graph.files.map((f) => {
    const id = f.path || f.id
    if (!id) {
      throw new Error('Each file/node must have a path or id')
    }
    const { depth, folder_group } = resolveFolderHierarchy(f)
    return {
      id,
      name: f.name || String(id).split(/[/\\]/).pop() || id,
      path: f.path || id,
      line_count: f.line_count ?? 0,
      layer: f.metadata?.layer ?? 'unknown',
      security_priority: f.metadata?.security_priority ?? 'none',
      functions: f.functions ?? [],
      classes: f.classes ?? [],
      depth,
      folder_group,
    }
  })
  const idToNode = new Map(nodes.map((n) => [n.id, n]))
  const ids = new Set(nodes.map((n) => n.id))
  const links: GraphLink[] = data.graph.edges
    .filter((e) => e && typeof e.source === 'string' && typeof e.target === 'string')
    .filter((e) => ids.has(e.source) && ids.has(e.target))
    .map((e) => {
      const src = idToNode.get(e.source)
      const tgt = idToNode.get(e.target)
      const violation = !!(src && tgt && src.layer === 'UI' && tgt.layer === 'Infrastructure')
      return { source: e.source, target: e.target, violation }
    })
  return { nodes, links }
}

function measureGraphHostSize(): { w: number; h: number } {
  const el = graphHost.value
  const rect = el?.getBoundingClientRect()
  const w = Math.max(
    320,
    el?.clientWidth ?? rect?.width ?? 0,
    typeof window !== 'undefined' ? window.innerWidth : 320,
  )
  const h = Math.max(
    240,
    el?.clientHeight ?? rect?.height ?? 0,
    typeof window !== 'undefined' ? window.innerHeight : 240,
  )
  return { w, h }
}

function resizeGraph() {
  const el = graphHost.value
  const g = graphInstance.value
  if (!el || !g) return
  const { w, h } = measureGraphHostSize()
  g.width(w)
  g.height(h)
}

function initForceGraph(nodes: GraphNode[], links: GraphLink[]) {
  const host = graphHost.value
  if (!host) return

  graphInstance.value?._destructor()

  const { w: initialW, h: initialH } = measureGraphHostSize()

  const fg = new ForceGraph(host)
    .width(initialW)
    .height(initialH)
    .graphData({ nodes, links })
    .backgroundColor('rgba(0,0,0,0)')
    .autoPauseRedraw(false)
    .linkColor((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      const o = linkDimmed(link as { source?: unknown; target?: unknown })
      if (L.violation) {
        return `rgba(249, 115, 22, ${0.35 + o * 0.55})`
      }
      const s = linkEndpointId((link as GraphLink).source)
      const src = nodeById.value.get(s)
      const c = src ? layerColor(src.layer) : '#00e5ff'
      const { r, g, b } = hexToRgb(c)
      return `rgba(${r},${g},${b},${0.12 + o * 0.45})`
    })
    .linkWidth((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      const o = linkDimmed(link as { source?: unknown; target?: unknown })
      const base = L.violation ? 1.1 : 0.45
      return base + o * 0.9
    })
    .linkDirectionalArrowLength(3.5)
    .linkDirectionalArrowRelPos(1)
    .linkDirectionalArrowColor((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      if (L.violation) return 'rgba(249, 115, 22, 0.9)'
      return 'rgba(0, 229, 255, 0.55)'
    })
    .linkDirectionalParticles(2)
    .linkDirectionalParticleSpeed(0.006)
    .linkDirectionalParticleWidth(1.4)
    .linkDirectionalParticleColor((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      if (L.violation) return '#fb923c'
      return '#22d3ee'
    })
    .linkLabel((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      return L.violation ? 'Architectural Violation' : ''
    })
    .nodeLabel('name')
    .cooldownTicks(120)
    .warmupTicks(160)
    .d3VelocityDecay(0.38)
    .d3AlphaMin(0.001)
    .onNodeClick((node) => {
      selectedNode.value = node as GraphNode
    })
    .onBackgroundClick(() => {
      selectedNode.value = null
    })
    .onNodeHover((node) => {
      hoveredBlastId.value = node ? (node as GraphNode).id : null
    })
    .nodeCanvasObjectMode(() => 'replace')
    .nodeCanvasObject((raw, ctx, globalScale) => {
      const node = raw as GraphNode
      if (
        node.x === undefined ||
        node.y === undefined ||
        !Number.isFinite(node.x) ||
        !Number.isFinite(node.y)
      ) {
        return
      }
      const x = node.x
      const y = node.y
      const R = nodeSize(node)
      const dim = nodeDimmed(node)
      const fill = layerColor(node.layer)
      const risk = isSecurityRiskNode(node)
      const pulse = risk ? 0.55 + 0.45 * Math.sin(performance.now() / 280) : 1
      const isSearchHit = searchQuery.value.trim() && matchesSearch(node, searchQuery.value)

      ctx.save()
      ctx.globalAlpha = dim

      if (risk) {
        ctx.shadowColor = `rgba(255, 40, 60, ${0.75 * pulse})`
        ctx.shadowBlur = (22 * pulse) / globalScale
      } else {
        ctx.shadowColor = hexToRgba(fill, 0.55)
        ctx.shadowBlur = 10 / globalScale
      }

      hexPath(ctx, x, y, R)
      ctx.fillStyle = risk ? hexToRgba('#ff0033', 0.88 * (0.85 + 0.15 * pulse)) : hexToRgba(fill, 0.9)
      ctx.fill()
      ctx.shadowBlur = 0

      hexPath(ctx, x, y, R)
      if (risk) {
        ctx.strokeStyle = `rgba(255, 80, 100, ${0.85 + 0.15 * pulse})`
        ctx.lineWidth = (2.6 * pulse) / globalScale
      } else {
        ctx.strokeStyle = hexToRgba(fill, 0.95)
        ctx.lineWidth = 1.15 / globalScale
      }
      ctx.stroke()

      if (isSearchHit && dim > 0.5) {
        hexPath(ctx, x, y, R + 1.2 / globalScale)
        ctx.strokeStyle = 'rgba(250, 250, 250, 0.9)'
        ctx.lineWidth = 1.2 / globalScale
        ctx.stroke()
      }

      ctx.restore()

      if (globalScale > 0.5 && R * globalScale > 4) {
        ctx.font = `${10 / globalScale}px ui-monospace, SFMono-Regular, Menlo, monospace`
        ctx.textAlign = 'center'
        ctx.textBaseline = 'top'
        ctx.fillStyle = 'rgba(226, 232, 240, 0.88)'
        const label = node.name.length > 16 ? `${node.name.slice(0, 14)}…` : node.name
        ctx.fillText(label, x, y + R + 2 / globalScale)
      }
    })
    .nodePointerAreaPaint((node, color, ctx, globalScale) => {
      const n = node as GraphNode
      if (
        n.x === undefined ||
        n.y === undefined ||
        !Number.isFinite(n.x) ||
        !Number.isFinite(n.y)
      ) {
        return
      }
      const x = n.x
      const y = n.y
      const R = nodeSize(n) + 3 / globalScale
      ctx.fillStyle = color
      hexPath(ctx, x, y, R)
      ctx.fill()
    })

  graphInstance.value = fg
  resizeGraph()
  /** Duration `0` = instant fit — avoids nodes flashing in a screen corner during zoom animation. */
  const fit = (ms: number) => fg.zoomToFit(ms, 32)
  fit(0)
  requestAnimationFrame(() => {
    resizeGraph()
    fit(0)
    requestAnimationFrame(() => {
      resizeGraph()
      fit(0)
    })
  })

  window.addEventListener('resize', resizeGraph)
}

function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const h = hex.replace('#', '')
  return {
    r: Number.parseInt(h.slice(0, 2), 16),
    g: Number.parseInt(h.slice(2, 4), 16),
    b: Number.parseInt(h.slice(4, 6), 16),
  }
}

/** Wait for layout + instant zoom passes so the camera matches the host (no corner flash). */
async function waitForChartLayoutSettled(): Promise<void> {
  await nextTick()
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        window.setTimeout(resolve, 320)
      })
    })
  })
  graphInstance.value?.zoomToFit(0, 32)
}

onMounted(() => {
  bootstrapping.value = true
  loadError.value = null

  void (async () => {
    try {
      await nextTick()
      if (!graphHost.value) {
        await nextTick()
      }

      const url = `${window.location.origin}/enriched_graph.json`
      const res = await fetch(url)
      if (!res.ok) {
        throw new Error(`Failed to load ${url} (${res.status} ${res.statusText})`)
      }
      const raw = await res.json()
      console.log('enriched_graph.json loaded:', raw)

      const data = normalizeGraphJson(raw)
      if (!data.graph.files.length) {
        throw new Error('Graph data is empty (no files/nodes).')
      }

      const { nodes, links } = parseGraphPayload(data)
      console.log('Mapped graph for force-graph:', {
        nodeCount: nodes.length,
        linkCount: links.length,
        violations: links.filter((l) => l.violation).length,
      })

      const m = new Map<string, GraphNode>()
      for (const n of nodes) m.set(n.id, n)
      nodeById.value = m
      const deg = buildDegreeMaps(links)
      outDegree.value = deg.out
      inDegree.value = deg.inn
      adjacency = buildAdjacency(links)

      await nextTick()
      if (!graphHost.value) {
        throw new Error('Graph container ref is not attached to the DOM.')
      }

      initForceGraph(nodes, links)
      await waitForChartLayoutSettled()
    } catch (e) {
      loadError.value = e instanceof Error ? e.message : String(e)
      console.error('Graph load/init failed:', e)
    } finally {
      bootstrapping.value = false
    }
  })()
})

watch([searchQuery, hoveredBlastId], () => {
  graphInstance.value?.d3ReheatSimulation()
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', resizeGraph)
  graphInstance.value?._destructor()
  graphInstance.value = null
})

function closeSidebar() {
  selectedNode.value = null
}

function healthBarClass(score: number): string {
  if (score >= 75) return 'from-emerald-500/80 to-cyan-400/60'
  if (score >= 50) return 'from-amber-500/80 to-cyan-500/50'
  return 'from-red-500/80 to-orange-500/60'
}
</script>

<template>
  <div
    class="cyber-dashboard relative h-screen w-full overflow-hidden bg-[#020617] font-mono text-slate-200"
  >
    <div
      class="absolute inset-0 transition-opacity duration-500 ease-out"
      :class="bootstrapping ? 'pointer-events-none opacity-0' : 'opacity-100'"
      :aria-hidden="bootstrapping"
    >
    <!-- Grid behind transparent canvas -->
    <div
      class="pointer-events-none absolute inset-0 z-0 bg-[#020617] bg-[linear-gradient(rgba(6,182,212,0.06)_1px,transparent_1px),linear-gradient(90deg,rgba(6,182,212,0.06)_1px,transparent_1px)] [background-size:28px_28px]"
      aria-hidden="true"
    />

    <div
      ref="graphHost"
      class="absolute inset-0 z-[1] h-full w-full min-h-0 overflow-hidden"
      aria-label="Code dependency graph"
    />

    <header
      class="pointer-events-none absolute left-4 top-4 z-20 select-none"
    >
      <p class="text-[10px] font-bold uppercase tracking-[0.35em] text-cyan-400/80">
        Cyber-Audit
      </p>
      <p class="text-xs text-slate-500">Dependency intelligence</p>
    </header>

    <div
      class="pointer-events-auto absolute left-1/2 top-4 z-20 w-[min(32rem,calc(100%-2rem))] -translate-x-1/2"
    >
      <div
        class="flex items-center gap-2 rounded-lg border border-cyan-500/25 bg-slate-950/70 px-4 py-2.5 shadow-[0_0_24px_rgba(34,211,238,0.12)] backdrop-blur-xl"
      >
        <Search class="h-4 w-4 shrink-0 text-cyan-400" aria-hidden="true" />
        <input
          v-model="searchQuery"
          type="search"
          placeholder="> scan_module — query path / symbol…"
          class="min-w-0 flex-1 bg-transparent text-xs text-slate-100 placeholder:text-slate-600 focus:outline-none"
          autocomplete="off"
        />
      </div>
    </div>

    <Transition
      enter-active-class="transition duration-200 ease-out"
      enter-from-class="translate-x-full opacity-0"
      enter-to-class="translate-x-0 opacity-100"
      leave-active-class="transition duration-150 ease-in"
      leave-from-class="translate-x-0 opacity-100"
      leave-to-class="translate-x-full opacity-0"
    >
      <aside
        v-if="selectedNode && selectedHealth"
        class="absolute bottom-0 right-0 top-0 z-30 flex w-[min(28rem,100%)] flex-col border-l border-cyan-500/20 bg-slate-950/55 shadow-[0_0_40px_rgba(0,0,0,0.55)] backdrop-blur-2xl"
        role="complementary"
        aria-label="File audit panel"
      >
        <div
          class="flex items-start justify-between gap-3 border-b border-cyan-500/15 bg-slate-950/40 p-5"
        >
          <div class="min-w-0">
            <h2 class="truncate text-base font-semibold tracking-tight text-cyan-100">
              {{ selectedNode.name }}
            </h2>
            <p class="mt-1 break-all text-[11px] leading-relaxed text-slate-500">
              {{ displayPath(selectedNode.path) }}
            </p>
          </div>
          <button
            type="button"
            class="rounded border border-slate-600/50 bg-slate-900/80 p-2 text-slate-400 transition hover:border-cyan-500/40 hover:text-cyan-200"
            aria-label="Close panel"
            @click="closeSidebar"
          >
            <X class="h-4 w-4" />
          </button>
        </div>

        <div class="flex-1 overflow-y-auto p-5 text-xs leading-relaxed">
          <section class="mb-6 border-b border-slate-800/80 pb-6">
            <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.2em] text-slate-500">
              Code summary
            </p>
            <p class="text-[13px] text-slate-300">
              {{ selectedHealth.summary }}
            </p>
          </section>

          <section class="mb-6 rounded-lg border border-emerald-500/20 bg-slate-900/40 p-4">
            <p class="mb-3 text-[10px] font-bold uppercase tracking-[0.25em] text-emerald-400/90">
              AI architecture health
            </p>
            <div class="mb-2 flex items-end justify-between gap-2">
              <span class="text-2xl font-bold tabular-nums text-cyan-300">
                {{ selectedHealth.score }}
              </span>
              <span class="text-[10px] uppercase text-slate-500">Maintainability 0–100</span>
            </div>
            <div class="h-2 overflow-hidden rounded bg-slate-800/80">
              <div
                class="h-full rounded bg-gradient-to-r transition-all duration-500"
                :class="healthBarClass(selectedHealth.score)"
                :style="{ width: `${selectedHealth.score}%` }"
              />
            </div>
            <p class="mt-3 text-[11px] text-slate-500">
              Derived from symbol count, outbound ({{ selectedHealth.out }}) / inbound ({{
                selectedHealth.inn
              }}) coupling, footprint, and security tier — heuristic audit proxy.
            </p>
          </section>

          <section class="mb-6">
            <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.2em] text-slate-500">
              Layer
            </p>
            <span
              class="inline-block rounded border px-2 py-1 text-[11px] font-semibold"
              :style="{
                borderColor: `${layerColor(selectedNode.layer)}88`,
                color: layerColor(selectedNode.layer),
                backgroundColor: `${layerColor(selectedNode.layer)}14`,
                boxShadow: `0 0 12px ${layerColor(selectedNode.layer)}33`,
              }"
            >
              {{ selectedNode.layer }}
            </span>
            <p class="mt-3 text-[12px] text-slate-400">
              {{ layerDescription(selectedNode.layer) }}
            </p>
          </section>

          <section class="mb-6">
            <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.2em] text-slate-500">
              Security tier
            </p>
            <p
              class="text-sm capitalize"
              :class="
                isSecurityRiskNode(selectedNode)
                  ? 'text-red-400 drop-shadow-[0_0_8px_rgba(248,113,113,0.5)]'
                  : 'text-slate-300'
              "
            >
              {{ selectedNode.security_priority }}
            </p>
          </section>

          <section class="mb-6">
            <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.2em] text-slate-500">
              Functions
            </p>
            <ul
              v-if="selectedNode.functions.length"
              class="max-h-40 space-y-1 overflow-y-auto text-[11px] text-slate-400"
            >
              <li
                v-for="fn in selectedNode.functions"
                :key="fn"
                class="border-l-2 border-cyan-500/30 pl-2"
              >
                {{ fn }}
              </li>
            </ul>
            <p v-else class="text-slate-600">— none indexed —</p>
          </section>

          <section>
            <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.2em] text-slate-500">
              Classes / types
            </p>
            <ul
              v-if="selectedNode.classes.length"
              class="max-h-40 space-y-1 overflow-y-auto text-[11px] text-slate-400"
            >
              <li
                v-for="cl in selectedNode.classes"
                :key="cl"
                class="border-l-2 border-purple-500/30 pl-2"
              >
                {{ cl }}
              </li>
            </ul>
            <p v-else class="text-slate-600">— none indexed —</p>
          </section>
        </div>
      </aside>
    </Transition>
    </div>

    <Transition name="boot-fade">
      <div
        v-if="bootstrapping"
        class="absolute inset-0 z-[100] flex flex-col items-center justify-center gap-4 bg-[#020617] pointer-events-auto"
        role="status"
        aria-live="polite"
        aria-busy="true"
      >
        <div
          class="h-9 w-9 animate-spin rounded-full border-2 border-cyan-500/20 border-t-cyan-400"
          aria-hidden="true"
        />
        <p class="text-xs font-bold uppercase tracking-[0.35em] text-cyan-400/95">
          Initializing audit…
        </p>
        <p class="text-[10px] uppercase tracking-wider text-slate-600">
          Loading graph &amp; stabilizing viewport
        </p>
      </div>
    </Transition>

    <div
      v-if="loadError && !bootstrapping"
      class="absolute inset-x-0 bottom-6 z-[90] mx-auto max-w-lg rounded border border-red-500/40 bg-red-950/90 px-4 py-3 text-center text-xs text-red-200 backdrop-blur-md"
      role="alert"
    >
      {{ loadError }}
    </div>
  </div>
</template>

<style scoped>
.boot-fade-enter-active,
.boot-fade-leave-active {
  transition: opacity 280ms ease;
}
.boot-fade-enter-from,
.boot-fade-leave-to {
  opacity: 0;
}
</style>
