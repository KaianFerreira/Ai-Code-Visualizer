<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import ForceGraph from 'force-graph'
import { Clock, Loader2, Search, X } from 'lucide-vue-next'

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
  imports?: string[]
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

const RECENT_SCANS_KEY = 'ai-native-visualizer-recent-scans'
const MAX_RECENT_SCANS = 12

interface RecentScanEntry {
  repoUrl: string
  at: number
  payload: string
}

function loadRecentScansFromStorage(): RecentScanEntry[] {
  try {
    const raw = localStorage.getItem(RECENT_SCANS_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter(
        (x): x is RecentScanEntry =>
          !!x &&
          typeof x === 'object' &&
          typeof (x as RecentScanEntry).repoUrl === 'string' &&
          typeof (x as RecentScanEntry).payload === 'string' &&
          typeof (x as RecentScanEntry).at === 'number',
      )
      .slice(0, MAX_RECENT_SCANS)
  } catch {
    return []
  }
}

function persistRecentScans(entries: RecentScanEntry[]) {
  localStorage.setItem(RECENT_SCANS_KEY, JSON.stringify(entries.slice(0, MAX_RECENT_SCANS)))
}

const SCAN_TICKER_MESSAGES = [
  'Handshaking with scanner API…',
  'Allocating analysis workspace…',
  'Resolving repository reference…',
  'Materializing shallow clone (when remote)…',
  'Walking source tree & applying filters…',
  'Invoking tree-sitter grammars…',
  'Extracting imports & building edges…',
  'Serializing graph payload…',
]

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
  /** Grouping key from JSON `directory` (normalized); hulls and directory force use this. */
  directoryKey: string
  /** Label for hull (e.g. `src/store`). */
  directoryLabel: string
  /** Parsed import count from graph JSON (AST imports). */
  importCount: number
  x?: number
  y?: number
  vx?: number
  vy?: number
  /** When set, d3-force keeps the node fixed at this graph position. */
  fx?: number
  fy?: number
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

/** Primary bar: repository URL or path; Enter runs /api/scan. */
const scannerInput = ref('')
const scanAnalyzing = ref(false)
const scanLogLines = ref<string[]>([])
const scanProgress = ref(0)
let scanTickerTimer: ReturnType<typeof setInterval> | null = null
let scanProgressTimer: ReturnType<typeof setInterval> | null = null

const recentScans = ref<RecentScanEntry[]>([])

const scanProgressDisplay = computed(() => Math.round(scanProgress.value))

const infoModal = ref<{ open: boolean; title: string; message: string }>({
  open: false,
  title: '',
  message: '',
})

/** Maintenance heatmap: hide low-degree nodes, size by complexity, highlight god objects. */
const heatmapMode = ref(false)

/** Filters highlighted nodes in the graph (separate from repository scan). */
const searchQuery = ref('')
const hoveredBlastId = ref<string | null>(null)
const selectedNode = ref<GraphNode | null>(null)

const nodeById = shallowRef<Map<string, GraphNode>>(new Map())
const outDegree = shallowRef<Map<string, number>>(new Map())
const inDegree = shallowRef<Map<string, number>>(new Map())
let adjacency: Map<string, Set<string>> = new Map()
let graphResizeHooked = false

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

function nodeConnectionCount(nodeId: string): number {
  return (outDegree.value.get(nodeId) ?? 0) + (inDegree.value.get(nodeId) ?? 0)
}

function heatmapNodeVisible(node: GraphNode): boolean {
  if (!heatmapMode.value) return true
  return nodeConnectionCount(node.id) >= 3
}

/** God object: very large + many imports (maintenance risk). */
function isGodObjectNode(node: GraphNode): boolean {
  return node.line_count > 500 && node.importCount > 10
}

function heatmapComplexity(node: GraphNode): number {
  return node.line_count + node.importCount
}

/** Radius for layout + hit tests (heatmap uses LOC + imports). */
function nodeSizeForDraw(node: GraphNode): number {
  if (heatmapMode.value) {
    const c = heatmapComplexity(node)
    return Math.max(3.5, Math.sqrt(Math.max(1, c)) * 0.4)
  }
  return nodeSize(node)
}

function heatmapMaintenanceFill(node: GraphNode): string {
  if (isGodObjectNode(node)) return '#ff781a'
  const c = heatmapComplexity(node)
  const t = Math.min(1, c / 1400)
  const r = Math.round(34 + t * (251 - 34))
  const g = Math.round(211 + t * (191 - 211))
  const b = Math.round(238 + t * (36 - 238))
  return `rgb(${r},${g},${b})`
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

/** Prefer JSON `directory`; fall back to computed `folder_group` for older payloads. */
function directoryClusterFromJson(
  f: EnrichedFile,
  folderGroup: string,
): { directoryKey: string; directoryLabel: string } {
  const normalizedDir = (f.directory ?? '').replace(/\\/g, '/').replace(/\/+$/, '').trim()
  if (normalizedDir.length > 0) {
    return { directoryKey: normalizedDir, directoryLabel: normalizedDir }
  }
  const fg = folderGroup && folderGroup !== '' ? folderGroup : '.'
  return { directoryKey: fg, directoryLabel: fg }
}

interface HullPoint {
  x: number
  y: number
}

/** Latest folder hulls + label rects for double-click zoom (updated every frame). */
interface ClusterPickTarget {
  directoryKey: string
  hull: HullPoint[]
  labelRect: { left: number; right: number; top: number; bottom: number }
}

let clusterPickSnapshot: ClusterPickTarget[] = []
let hostClusterDblCleanup: (() => void) | null = null

function hullCross(o: HullPoint, a: HullPoint, b: HullPoint): number {
  return (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

function convexHull(points: HullPoint[]): HullPoint[] {
  if (points.length <= 1) return points.slice()
  const pts = [...points].sort((a, b) => a.x - b.x || a.y - b.y)
  const lower: HullPoint[] = []
  for (const p of pts) {
    while (
      lower.length >= 2 &&
      hullCross(lower[lower.length - 2]!, lower[lower.length - 1]!, p) <= 0
    ) {
      lower.pop()
    }
    lower.push(p)
  }
  const upper: HullPoint[] = []
  for (let i = pts.length - 1; i >= 0; i--) {
    const p = pts[i]!
    while (
      upper.length >= 2 &&
      hullCross(upper[upper.length - 2]!, upper[upper.length - 1]!, p) <= 0
    ) {
      upper.pop()
    }
    upper.push(p)
  }
  upper.pop()
  lower.pop()
  return lower.concat(upper)
}

function polygonCentroid(pts: HullPoint[]): HullPoint {
  if (pts.length === 0) return { x: 0, y: 0 }
  if (pts.length === 1) return { ...pts[0]! }
  let a = 0
  let cx = 0
  let cy = 0
  const n = pts.length
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n
    const crossv = pts[i]!.x * pts[j]!.y - pts[j]!.x * pts[i]!.y
    a += crossv
    cx += (pts[i]!.x + pts[j]!.x) * crossv
    cy += (pts[i]!.y + pts[j]!.y) * crossv
  }
  a *= 0.5
  if (Math.abs(a) < 1e-8) {
    let sx = 0
    let sy = 0
    for (const p of pts) {
      sx += p.x
      sy += p.y
    }
    return { x: sx / pts.length, y: sy / pts.length }
  }
  return { x: cx / (6 * a), y: cy / (6 * a) }
}

function directoryClusterHue(key: string): number {
  let h = 0
  for (let i = 0; i < key.length; i++) {
    h = (h * 31 + key.charCodeAt(i)) >>> 0
  }
  return h % 360
}

/** Path-style label for folder clusters (e.g. `/src/components`, `/` at repo root). */
function clusterDisplayLabel(directoryKey: string): string {
  if (!directoryKey || directoryKey === '.') return '/'
  return `/${directoryKey.replace(/^\/+/, '')}`
}

function pointInPolygon(x: number, y: number, poly: HullPoint[]): boolean {
  if (poly.length < 3) return false
  let inside = false
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const xi = poly[i]!.x
    const yi = poly[i]!.y
    const xj = poly[j]!.x
    const yj = poly[j]!.y
    const denom = yj - yi || 1e-10
    const intersect =
      (yi > y) !== (yj > y) && x < ((xj - xi) * (y - yi)) / denom + xi
    if (intersect) inside = !inside
  }
  return inside
}

function graphCoordsFromClient(
  host: HTMLElement,
  clientX: number,
  clientY: number,
  fg: InstanceType<typeof ForceGraph>,
) {
  const rect = host.getBoundingClientRect()
  return fg.screen2GraphCoords(clientX - rect.left, clientY - rect.top)
}

function findNodeNearGraphCoords(
  gx: number,
  gy: number,
  fg: InstanceType<typeof ForceGraph>,
  padFactor = 1.35,
): GraphNode | null {
  const { nodes } = fg.graphData() as unknown as { nodes: GraphNode[] }
  for (const n of nodes) {
    if (n.x === undefined || n.y === undefined || !Number.isFinite(n.x) || !Number.isFinite(n.y)) continue
    const R = nodeSizeForDraw(n) * padFactor
    if ((gx - n.x) ** 2 + (gy - n.y) ** 2 <= R * R) return n
  }
  return null
}

function pickClusterDirectoryKey(gx: number, gy: number): string | null {
  for (const t of clusterPickSnapshot) {
    const r = t.labelRect
    if (gx >= r.left && gx <= r.right && gy >= r.top && gy <= r.bottom) return t.directoryKey
  }
  for (const t of clusterPickSnapshot) {
    if (pointInPolygon(gx, gy, t.hull)) return t.directoryKey
  }
  return null
}

function clusterFootprintRing(node: GraphNode): HullPoint[] {
  const R = nodeSizeForDraw(node) + 12
  const ringN = 10
  const out: HullPoint[] = []
  const x = node.x!
  const y = node.y!
  for (let i = 0; i < ringN; i++) {
    const a = (i / ringN) * Math.PI * 2
    out.push({ x: x + Math.cos(a) * R, y: y + Math.sin(a) * R })
  }
  return out
}

function forceDirectoryCluster(strength: number) {
  let simNodes: GraphNode[] = []
  const force = (alpha: number) => {
    const cents = new Map<string, { x: number; y: number; c: number }>()
    for (const n of simNodes) {
      if (
        n.x === undefined ||
        n.y === undefined ||
        !Number.isFinite(n.x) ||
        !Number.isFinite(n.y)
      ) {
        continue
      }
      const k = n.directoryKey
      let e = cents.get(k)
      if (!e) {
        e = { x: 0, y: 0, c: 0 }
        cents.set(k, e)
      }
      e.x += n.x
      e.y += n.y
      e.c += 1
    }
    for (const e of cents.values()) {
      if (e.c > 0) {
        e.x /= e.c
        e.y /= e.c
      }
    }
    for (const n of simNodes) {
      if (n.x === undefined || n.y === undefined) continue
      const e = cents.get(n.directoryKey)
      if (!e || e.c === 0) continue
      const dx = e.x - n.x
      const dy = e.y - n.y
      n.vx = (n.vx ?? 0) + dx * strength * alpha
      n.vy = (n.vy ?? 0) + dy * strength * alpha
    }
  }
  force.initialize = (ns: GraphNode[]) => {
    simNodes = ns
  }
  return force
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
    const { directoryKey, directoryLabel } = directoryClusterFromJson(f, folder_group)
    const importCount = Array.isArray(f.imports) ? f.imports.length : 0
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
      directoryKey,
      directoryLabel,
      importCount,
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

/** Lock layout: stop drift when zooming or filtering (d3 `fx`/`fy`). */
function pinAllGraphNodes(fg: InstanceType<typeof ForceGraph>) {
  const data = fg.graphData() as unknown as { nodes: GraphNode[] }
  for (const n of data.nodes) {
    if (n.x !== undefined && n.y !== undefined && Number.isFinite(n.x) && Number.isFinite(n.y)) {
      n.fx = n.x
      n.fy = n.y
      n.vx = 0
      n.vy = 0
    }
  }
}

function initForceGraph(nodes: GraphNode[], links: GraphLink[]) {
  const host = graphHost.value
  if (!host) return

  graphInstance.value?._destructor()
  hostClusterDblCleanup?.()
  hostClusterDblCleanup = null

  const { w: initialW, h: initialH } = measureGraphHostSize()

  const clusterLabels: {
    x: number
    y: number
    directoryKey: string
    displayLabel: string
    hue: number
    hull: HullPoint[]
  }[] = []

  const fg = new ForceGraph(host)
    .width(initialW)
    .height(initialH)
    .graphData({ nodes, links })
    .d3Force('directoryCluster', forceDirectoryCluster(0.17) as never)
    .backgroundColor('rgba(0,0,0,0)')
    .autoPauseRedraw(false)
    .enableNodeDrag(false)
    .nodeVisibility((n) => heatmapNodeVisible(n as GraphNode))
    .linkVisibility((link) => {
      if (!heatmapMode.value) return true
      const L = linkAsGraphLink(link as GraphLink)
      const a = nodeById.value.get(L.source)
      const b = nodeById.value.get(L.target)
      return !!(a && b && heatmapNodeVisible(a) && heatmapNodeVisible(b))
    })
    .linkColor((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      const o = linkDimmed(link as { source?: unknown; target?: unknown })
      if (heatmapMode.value) {
        if (L.violation) {
          return `rgba(249, 115, 22, ${0.2 + o * 0.35})`
        }
        return `rgba(100, 116, 139, ${0.08 + o * 0.28})`
      }
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
    .linkDirectionalParticles(0)
    .linkLabel((link) => {
      const L = linkAsGraphLink(link as GraphLink)
      return L.violation ? 'Architectural Violation' : ''
    })
    .nodeLabel('name')
    .cooldownTicks(120)
    .warmupTicks(160)
    .d3VelocityDecay(0.38)
    .d3AlphaMin(0.001)
    .onEngineStop(() => {
      pinAllGraphNodes(fg)
    })
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
      const R = nodeSizeForDraw(node)
      const dim = nodeDimmed(node)
      const heatmap = heatmapMode.value
      const fill = layerColor(node.layer)
      const risk = !heatmap && isSecurityRiskNode(node)
      const pulse = risk ? 0.55 + 0.45 * Math.sin(performance.now() / 280) : 1
      const isSearchHit = searchQuery.value.trim() && matchesSearch(node, searchQuery.value)
      const god = heatmap && isGodObjectNode(node)
      const heatFill = heatmap ? heatmapMaintenanceFill(node) : fill

      ctx.save()
      ctx.globalAlpha = dim

      if (heatmap) {
        if (god) {
          ctx.shadowColor = 'rgba(255, 140, 60, 0.92)'
          ctx.shadowBlur = 28 / globalScale
        } else {
          ctx.shadowColor = hexToRgba(heatFill, 0.45)
          ctx.shadowBlur = 8 / globalScale
        }
      } else if (risk) {
        ctx.shadowColor = `rgba(255, 40, 60, ${0.75 * pulse})`
        ctx.shadowBlur = (22 * pulse) / globalScale
      } else {
        ctx.shadowColor = hexToRgba(fill, 0.55)
        ctx.shadowBlur = 10 / globalScale
      }

      hexPath(ctx, x, y, R)
      ctx.fillStyle = heatmap
        ? hexToRgba(heatFill, god ? 0.96 : 0.9)
        : risk
          ? hexToRgba('#ff0033', 0.88 * (0.85 + 0.15 * pulse))
          : hexToRgba(fill, 0.9)
      ctx.fill()
      ctx.shadowBlur = 0

      hexPath(ctx, x, y, R)
      if (heatmap) {
        ctx.strokeStyle = god ? 'rgba(255, 200, 120, 0.95)' : hexToRgba(heatFill, 0.92)
        ctx.lineWidth = god ? 2.2 / globalScale : 1.1 / globalScale
      } else if (risk) {
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
      const R = nodeSizeForDraw(n) + 3 / globalScale
      ctx.fillStyle = color
      hexPath(ctx, x, y, R)
      ctx.fill()
    })
    .onRenderFramePre((ctx, globalScale) => {
      clusterLabels.length = 0
      const { nodes: frameNodes } = fg.graphData() as unknown as { nodes: GraphNode[] }
      const byDir = new Map<string, GraphNode[]>()
      for (const n of frameNodes) {
        if (
          n.x === undefined ||
          n.y === undefined ||
          !Number.isFinite(n.x) ||
          !Number.isFinite(n.y)
        ) {
          continue
        }
        if (!heatmapNodeVisible(n)) continue
        const g = byDir.get(n.directoryKey) ?? []
        g.push(n)
        byDir.set(n.directoryKey, g)
      }

      for (const [dirKey, groupNodes] of byDir) {
        const pts: HullPoint[] = []
        for (const n of groupNodes) {
          pts.push(...clusterFootprintRing(n))
        }
        if (pts.length === 0) continue

        let hull = convexHull(pts)
        if (hull.length < 3) {
          let minX = Infinity
          let minY = Infinity
          let maxX = -Infinity
          let maxY = -Infinity
          for (const p of pts) {
            minX = Math.min(minX, p.x)
            maxX = Math.max(maxX, p.x)
            minY = Math.min(minY, p.y)
            maxY = Math.max(maxY, p.y)
          }
          const pad = 8
          hull = [
            { x: minX - pad, y: minY - pad },
            { x: maxX + pad, y: minY - pad },
            { x: maxX + pad, y: maxY + pad },
            { x: minX - pad, y: maxY + pad },
          ]
        }

        const hue = directoryClusterHue(dirKey)
        ctx.beginPath()
        ctx.moveTo(hull[0]!.x, hull[0]!.y)
        for (let i = 1; i < hull.length; i++) {
          ctx.lineTo(hull[i]!.x, hull[i]!.y)
        }
        ctx.closePath()
        ctx.fillStyle = `hsla(${hue}, 44%, 50%, 0.11)`
        ctx.fill()
        ctx.strokeStyle = `hsla(${hue}, 55%, 62%, 0.32)`
        ctx.lineWidth = 1.45 / globalScale
        ctx.stroke()

        const c = polygonCentroid(hull)
        const hullCopy = hull.map((p) => ({ x: p.x, y: p.y }))
        clusterLabels.push({
          x: c.x,
          y: c.y,
          directoryKey: dirKey,
          displayLabel: clusterDisplayLabel(dirKey),
          hue,
          hull: hullCopy,
        })
      }
    })
    .onRenderFramePost((ctx, globalScale) => {
      const picks: ClusterPickTarget[] = []
      const font = `${11 / globalScale}px ui-monospace, SFMono-Regular, Menlo, monospace`
      ctx.font = font

      for (const cl of clusterLabels) {
        const text =
          cl.displayLabel.length > 44 ? `${cl.displayLabel.slice(0, 42)}…` : cl.displayLabel
        const metrics = ctx.measureText(text)
        const textW = metrics.width
        const lineH = 14 / globalScale
        const baselineY = cl.y - 12 / globalScale
        const pad = 5 / globalScale
        picks.push({
          directoryKey: cl.directoryKey,
          hull: cl.hull,
          labelRect: {
            left: cl.x - textW / 2 - pad,
            right: cl.x + textW / 2 + pad,
            top: baselineY - lineH,
            bottom: baselineY + pad * 0.35,
          },
        })

        if (globalScale < 0.28) continue

        ctx.save()
        ctx.textAlign = 'center'
        ctx.textBaseline = 'bottom'
        ctx.lineWidth = 4 / globalScale
        ctx.strokeStyle = 'rgba(2, 6, 23, 0.94)'
        ctx.fillStyle = `hsla(${cl.hue}, 58%, 74%, 0.95)`
        ctx.strokeText(text, cl.x, baselineY)
        ctx.fillText(text, cl.x, baselineY)
        ctx.restore()
      }
      clusterPickSnapshot = picks
    })

  const onHostDblClick = (ev: MouseEvent) => {
    if (scanAnalyzing.value || bootstrapping.value) return
    const inst = graphInstance.value
    if (!inst || inst !== fg) return
    const { x: gx, y: gy } = graphCoordsFromClient(host, ev.clientX, ev.clientY, inst)
    if (findNodeNearGraphCoords(gx, gy, inst)) return
    const dirKey = pickClusterDirectoryKey(gx, gy)
    if (dirKey == null) return
    ev.preventDefault()
    ev.stopImmediatePropagation()
    inst.zoomToFit(520, 68, (n) => (n as GraphNode).directoryKey === dirKey)
  }
  host.addEventListener('dblclick', onHostDblClick, { capture: true })
  hostClusterDblCleanup = () => {
    host.removeEventListener('dblclick', onHostDblClick, { capture: true })
    hostClusterDblCleanup = null
  }

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

  if (!graphResizeHooked) {
    graphResizeHooked = true
    window.addEventListener('resize', resizeGraph)
  }
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
  recentScans.value = loadRecentScansFromStorage()

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

      await applyGraphPayload(raw)
      console.log('Mapped graph for force-graph from static JSON')
    } catch (e) {
      loadError.value = e instanceof Error ? e.message : String(e)
      console.error('Graph load/init failed:', e)
    } finally {
      bootstrapping.value = false
    }
  })()
})

watch(heatmapMode, async () => {
  await nextTick()
  const sel = selectedNode.value
  if (sel && !heatmapNodeVisible(sel)) selectedNode.value = null
  graphInstance.value?.zoomToFit(420, 56)
})

onBeforeUnmount(() => {
  stopScanAnimation()
  hostClusterDblCleanup?.()
  if (graphResizeHooked) {
    graphResizeHooked = false
    window.removeEventListener('resize', resizeGraph)
  }
  graphInstance.value?._destructor()
  graphInstance.value = null
})

function closeSidebar() {
  selectedNode.value = null
}

function showInfoModal(title: string, message: string) {
  infoModal.value = { open: true, title, message }
}

function closeInfoModal() {
  infoModal.value = { ...infoModal.value, open: false }
}

function formatScanLogTime(): string {
  const d = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

function pushScanLog(line: string) {
  const entry = `[${formatScanLogTime()}] ${line}`
  scanLogLines.value = [...scanLogLines.value.slice(-48), entry]
}

function stopScanAnimation() {
  if (scanTickerTimer !== null) {
    clearInterval(scanTickerTimer)
    scanTickerTimer = null
  }
  if (scanProgressTimer !== null) {
    clearInterval(scanProgressTimer)
    scanProgressTimer = null
  }
}

function startScanAnimation() {
  stopScanAnimation()
  scanLogLines.value = []
  scanProgress.value = 6
  pushScanLog('Session opened — awaiting graph bundle…')
  let tick = 0
  scanTickerTimer = window.setInterval(() => {
    pushScanLog(SCAN_TICKER_MESSAGES[tick % SCAN_TICKER_MESSAGES.length]!)
    tick++
  }, 820)
  scanProgressTimer = window.setInterval(() => {
    scanProgress.value = Math.min(88, scanProgress.value + 1.2 + Math.random() * 4)
  }, 220)
}

function addRecentScanEntry(repoUrl: string, payload: unknown) {
  try {
    const payloadStr = JSON.stringify(payload)
    const rest = loadRecentScansFromStorage().filter((e) => e.repoUrl !== repoUrl)
    const next: RecentScanEntry[] = [
      { repoUrl, at: Date.now(), payload: payloadStr },
      ...rest,
    ].slice(0, MAX_RECENT_SCANS)
    persistRecentScans(next)
    recentScans.value = next
  } catch (e) {
    console.warn('Could not persist recent scan:', e)
  }
}

function removeRecentEntry(repoUrl: string) {
  const next = loadRecentScansFromStorage().filter((e) => e.repoUrl !== repoUrl)
  persistRecentScans(next)
  recentScans.value = next
}

async function applyGraphPayload(raw: unknown): Promise<void> {
  const data = normalizeGraphJson(raw)
  if (!data.graph.files.length) {
    throw new Error('Graph data is empty (no files/nodes).')
  }

  const { nodes, links } = parseGraphPayload(data)
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
}

async function applyRecentScan(entry: RecentScanEntry) {
  loadError.value = null
  try {
    const raw = JSON.parse(entry.payload) as unknown
    await applyGraphPayload(raw)
    scannerInput.value = entry.repoUrl
    selectedNode.value = null
  } catch (e) {
    showInfoModal('Could not restore scan', e instanceof Error ? e.message : String(e))
  }
}

async function startScan() {
  const url = scannerInput.value.trim()
  if (!url) {
    showInfoModal(
      'Repository scanner',
      'Enter a git HTTPS/SSH URL or a local path, then press Enter. Start scanner-api (cd scanner-api && npm run dev) and build the Rust backend (cd backend && cargo build).',
    )
    return
  }

  scanAnalyzing.value = true
  loadError.value = null
  selectedNode.value = null
  startScanAnimation()

  try {
    pushScanLog(`POST /api/scan — repoUrl: ${url.length > 64 ? `${url.slice(0, 62)}…` : url}`)
    const res = await fetch('/api/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ repoUrl: url }),
    })
    const text = await res.text()
    let body: unknown = null
    try {
      body = text ? JSON.parse(text) : null
    } catch {
      body = null
    }
    if (!res.ok) {
      const msg =
        body &&
        typeof body === 'object' &&
        body !== null &&
        'error' in body &&
        (body as { error: unknown }).error != null
          ? String((body as { error: unknown }).error)
          : text || `${res.status} ${res.statusText}`
      pushScanLog(`ERROR: ${msg.slice(0, 500)}${msg.length > 500 ? '…' : ''}`)
      showInfoModal('Scan failed', msg)
      return
    }
    stopScanAnimation()
    scanProgress.value = 96
    pushScanLog('Response OK — parsing graph & warming simulation…')
    await applyGraphPayload(body)
    addRecentScanEntry(url, body)
    scanProgress.value = 100
    pushScanLog(`Indexed ${normalizeGraphJson(body).graph.files.length} file node(s). Layout stabilized.`)
    await new Promise((r) => window.setTimeout(r, 420))
  } catch (e) {
    pushScanLog(`FATAL: ${e instanceof Error ? e.message : String(e)}`)
    showInfoModal('Scan failed', e instanceof Error ? e.message : String(e))
  } finally {
    stopScanAnimation()
    scanAnalyzing.value = false
    scanProgress.value = 0
  }
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
      title="Double-click a folder path label (or its hull) to zoom into that directory cluster"
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
      class="pointer-events-auto absolute left-1/2 top-4 z-20 flex w-[min(44rem,calc(100%-2rem))] -translate-x-1/2 flex-col gap-2"
    >
      <div
        class="flex items-center gap-2 rounded-lg border border-cyan-500/30 bg-slate-950/80 px-3 py-2.5 shadow-[0_0_28px_rgba(34,211,238,0.14)] backdrop-blur-xl"
      >
        <Search class="h-4 w-4 shrink-0 text-cyan-400" aria-hidden="true" />
        <input
          v-model="scannerInput"
          type="text"
          placeholder="Repository URL or path — Enter to scan"
          class="min-w-0 flex-1 bg-transparent text-xs text-slate-100 placeholder:text-slate-600 focus:outline-none"
          autocomplete="off"
          :disabled="scanAnalyzing"
          @keydown.enter.prevent="startScan"
        />
        <button
          type="button"
          class="inline-flex shrink-0 items-center gap-1 rounded border border-cyan-500/40 bg-cyan-500/12 px-2.5 py-1 text-[10px] font-bold uppercase tracking-wider text-cyan-200/95 transition hover:bg-cyan-500/22 disabled:pointer-events-none disabled:opacity-40"
          :disabled="scanAnalyzing"
          title="Run scan"
          @click="startScan"
        >
          <Loader2 v-if="scanAnalyzing" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          <span v-else>Run</span>
        </button>
      </div>

      <div
        v-if="recentScans.length"
        class="flex flex-wrap items-center gap-x-2 gap-y-1.5 rounded-lg border border-slate-700/50 bg-slate-950/65 px-3 py-2 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)] backdrop-blur-md"
      >
        <div class="flex items-center gap-1 text-[9px] font-bold uppercase tracking-[0.2em] text-slate-500">
          <Clock class="h-3 w-3 text-violet-400/90" aria-hidden="true" />
          Recently scanned
        </div>
        <div class="flex min-w-0 flex-1 flex-wrap gap-1.5">
          <div
            v-for="entry in recentScans"
            :key="entry.repoUrl"
            class="group flex max-w-full items-center gap-0.5 rounded border border-slate-600/45 bg-slate-900/80 pl-2 pr-0.5"
          >
            <button
              type="button"
              class="max-w-[14rem] truncate py-1 text-left text-[10px] text-cyan-200/90 transition hover:text-cyan-100"
              :title="entry.repoUrl"
              @click="applyRecentScan(entry)"
            >
              {{ entry.repoUrl.length > 42 ? `${entry.repoUrl.slice(0, 40)}…` : entry.repoUrl }}
            </button>
            <button
              type="button"
              class="rounded p-1 text-slate-500 opacity-70 transition hover:bg-red-950/50 hover:text-red-300 group-hover:opacity-100"
              title="Remove from list"
              aria-label="Remove from recently scanned"
              @click.stop="removeRecentEntry(entry.repoUrl)"
            >
              <X class="h-3 w-3" />
            </button>
          </div>
        </div>
      </div>

      <div
        class="flex items-center gap-2 rounded-lg border border-slate-600/35 bg-slate-950/55 px-3 py-2 backdrop-blur-md"
      >
        <span class="shrink-0 text-[9px] font-bold uppercase tracking-wider text-slate-500">Filter</span>
        <input
          v-model="searchQuery"
          type="search"
          placeholder="Highlight nodes by path / name…"
          class="min-w-0 flex-1 bg-transparent text-[11px] text-slate-200 placeholder:text-slate-600 focus:outline-none"
          autocomplete="off"
        />
      </div>

      <div
        class="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-lg border border-orange-500/20 bg-slate-950/60 px-3 py-2 backdrop-blur-md"
      >
        <label
          class="flex cursor-pointer select-none items-center gap-2 text-[11px] text-slate-200"
        >
          <input
            v-model="heatmapMode"
            type="checkbox"
            class="h-3.5 w-3.5 rounded border-orange-500/50 bg-slate-900 text-orange-500 focus:ring-orange-500/40"
          />
          <span class="font-semibold text-orange-200/95">Heatmap mode</span>
        </label>
        <p class="text-[9px] leading-snug text-slate-500">
          Hides files with fewer than 3 graph links. Node size = lines + imports. Bright orange glow =
          <span class="text-orange-400/90">god object</span>
          (over 500 lines and over 10 imports).
        </p>
      </div>
    </div>

    <Transition name="scan-fade">
      <div
        v-if="scanAnalyzing"
        class="absolute inset-0 z-[102] flex items-center justify-center bg-[#020617]/88 p-4 backdrop-blur-[2px]"
        role="status"
        aria-live="polite"
        aria-busy="true"
      >
        <div
          class="w-full max-w-[26rem] overflow-hidden rounded-xl border border-cyan-500/30 bg-slate-950/96 shadow-[0_0_60px_rgba(34,211,238,0.12),0_25px_50px_rgba(0,0,0,0.5)]"
        >
          <div
            class="border-b border-cyan-500/15 bg-gradient-to-r from-cyan-500/10 via-violet-500/5 to-transparent px-5 py-4"
          >
            <p class="text-[10px] font-bold uppercase tracking-[0.42em] text-cyan-400/95">
              Analyzing repository
            </p>
            <p class="mt-1 text-sm font-semibold tracking-tight text-slate-100">
              Deep scan pipeline
            </p>
            <p class="mt-0.5 text-[10px] text-slate-500">
              Secure channel to scanner-api · streaming status below
            </p>
          </div>
          <div class="px-5 pb-5 pt-4">
            <div
              class="mb-1 flex justify-between text-[9px] font-mono uppercase tracking-wider text-slate-500"
            >
              <span>Progress</span>
              <span class="tabular-nums text-cyan-400/80">{{ scanProgressDisplay }}%</span>
            </div>
            <div class="relative mb-4 h-2 w-full overflow-hidden rounded-full bg-slate-800/90 ring-1 ring-cyan-500/10">
              <div
                class="scan-progress-bar absolute inset-y-0 left-0 rounded-full bg-gradient-to-r from-cyan-400 via-violet-400 to-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.45)] transition-[width] duration-200 ease-out"
                :style="{ width: `${Math.min(100, Math.round(scanProgress))}%` }"
              />
            </div>
            <div
              class="scan-terminal max-h-52 overflow-y-auto rounded-lg border border-slate-700/70 bg-[#050b14] px-3 py-2.5 font-mono text-[10px] leading-[1.55] shadow-[inset_0_0_0_1px_rgba(34,211,238,0.06)]"
            >
              <div
                v-for="(line, idx) in scanLogLines"
                :key="idx"
                class="whitespace-pre-wrap break-all text-emerald-400/95"
              >
                {{ line }}
              </div>
            </div>
          </div>
        </div>
      </div>
    </Transition>

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

    <Teleport to="body">
      <div
        v-if="infoModal.open"
        class="fixed inset-0 z-[200] flex items-center justify-center p-4"
        role="dialog"
        aria-modal="true"
        aria-labelledby="info-modal-title"
      >
        <button
          type="button"
          class="absolute inset-0 bg-black/65 backdrop-blur-sm"
          aria-label="Dismiss"
          @click="closeInfoModal"
        />
        <div
          class="relative z-10 w-full max-w-lg rounded-lg border border-cyan-500/35 bg-slate-950/96 p-5 shadow-[0_0_48px_rgba(34,211,238,0.18)]"
        >
          <h2
            id="info-modal-title"
            class="mb-2 text-sm font-bold uppercase tracking-[0.2em] text-cyan-400"
          >
            {{ infoModal.title }}
          </h2>
          <p class="max-h-[50vh] overflow-y-auto whitespace-pre-wrap break-words text-xs leading-relaxed text-slate-300">
            {{ infoModal.message }}
          </p>
          <button
            type="button"
            class="mt-4 w-full rounded border border-slate-600/60 bg-slate-900/85 py-2.5 text-xs font-semibold text-slate-200 transition hover:border-cyan-500/45 hover:text-cyan-100"
            @click="closeInfoModal"
          >
            OK
          </button>
        </div>
      </div>
    </Teleport>
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

.scan-fade-enter-active,
.scan-fade-leave-active {
  transition: opacity 220ms ease;
}
.scan-fade-enter-from,
.scan-fade-leave-to {
  opacity: 0;
}

.scan-progress-bar {
  animation: scan-progress-shimmer 1.8s ease-in-out infinite;
}
@keyframes scan-progress-shimmer {
  0%,
  100% {
    filter: brightness(1);
  }
  50% {
    filter: brightness(1.15);
  }
}

.scan-terminal {
  scrollbar-width: thin;
  scrollbar-color: rgba(34, 211, 238, 0.35) transparent;
}
</style>
