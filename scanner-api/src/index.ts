import { spawn } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
/** Repository root (parent of `scanner-api/`). */
const workspaceRoot = path.resolve(__dirname, '..', '..')

const PORT = Number.parseInt(process.env.SCANNER_API_PORT ?? '8787', 10)

function readRequestBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    req.on('data', (c: Buffer) => chunks.push(c))
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
    req.on('error', reject)
  })
}

function resolveBackendExecutable(root: string): string | null {
  const win = process.platform === 'win32'
  const name = win ? 'backend.exe' : 'backend'
  const debug = path.join(root, 'backend', 'target', 'debug', name)
  const release = path.join(root, 'backend', 'target', 'release', name)
  if (existsSync(debug)) return debug
  if (existsSync(release)) return release
  return null
}

function runBackendScan(repoUrl: string, backendDir: string, exe: string | null): Promise<{ code: number | null; stderr: string }> {
  return new Promise((resolve) => {
    let stderr = ''
    const child =
      exe != null
        ? spawn(exe, [repoUrl], {
            cwd: backendDir,
            windowsHide: true,
          })
        : spawn('cargo', ['run', '--', repoUrl], {
            cwd: backendDir,
            shell: process.platform === 'win32',
            windowsHide: true,
          })

    child.stderr?.setEncoding('utf8')
    child.stderr?.on('data', (d: string) => {
      stderr += d
    })
    child.on('error', (err: unknown) => {
      stderr += err instanceof Error ? err.message : String(err)
      resolve({ code: -1, stderr })
    })
    child.on('close', (code: number | null) => resolve({ code, stderr }))
  })
}

function sendJson(res: ServerResponse, status: number, body: unknown) {
  const payload = JSON.stringify(body)
  res.statusCode = status
  res.setHeader('Content-Type', 'application/json; charset=utf-8')
  res.end(payload)
}

function applyCors(req: IncomingMessage, res: ServerResponse) {
  const configured = process.env.CORS_ORIGIN?.trim()
  const origin = req.headers.origin
  if (configured && configured !== '*') {
    res.setHeader('Access-Control-Allow-Origin', configured)
  } else if (origin) {
    res.setHeader('Access-Control-Allow-Origin', origin)
  } else {
    res.setHeader('Access-Control-Allow-Origin', '*')
  }
  res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type')
}

async function handleScan(req: IncomingMessage, res: ServerResponse) {
  applyCors(req, res)

  let raw: string
  try {
    raw = await readRequestBody(req)
  } catch {
    sendJson(res, 400, { success: false, error: 'could not read request body' })
    return
  }

  let repoUrl = ''
  try {
    const parsed = raw ? JSON.parse(raw) : {}
    repoUrl = String(
      (parsed as { repoUrl?: string; url?: string }).repoUrl ?? (parsed as { url?: string }).url ?? '',
    ).trim()
  } catch {
    sendJson(res, 400, { success: false, error: 'invalid JSON body' })
    return
  }

  if (!repoUrl) {
    sendJson(res, 400, { success: false, error: 'repoUrl is required' })
    return
  }

  const backendDir = path.join(workspaceRoot, 'backend')
  if (!existsSync(path.join(backendDir, 'Cargo.toml'))) {
    sendJson(res, 500, {
      success: false,
      error: `backend not found at ${backendDir}`,
    })
    return
  }

  const fromEnv = process.env.RUST_SCANNER_BIN?.trim()
  const exe = fromEnv && fromEnv.length > 0 ? fromEnv : resolveBackendExecutable(workspaceRoot)

  const { code, stderr } = await runBackendScan(repoUrl, backendDir, exe)

  if (code !== 0) {
    sendJson(res, 500, {
      success: false,
      error: stderr.trim() || `scanner exited with code ${code}`,
    })
    return
  }

  const outFile = path.join(backendDir, 'graph_output.json')
  if (!existsSync(outFile)) {
    sendJson(res, 500, {
      success: false,
      error: `expected ${outFile} after scan; file missing`,
    })
    return
  }

  try {
    const graphJson = readFileSync(outFile, 'utf8')
    const data = JSON.parse(graphJson) as unknown
    sendJson(res, 200, data)
  } catch (e) {
    sendJson(res, 500, {
      success: false,
      error: e instanceof Error ? e.message : 'failed to read graph_output.json',
    })
  }
}

const server = createServer((req, res) => {
  const pathname = req.url?.split('?')[0] ?? ''

  if (req.method === 'OPTIONS') {
    applyCors(req, res)
    res.statusCode = 204
    res.end()
    return
  }

  if (pathname === '/api/scan' && req.method === 'POST') {
    void handleScan(req, res)
    return
  }

  if (pathname === '/health' && req.method === 'GET') {
    applyCors(req, res)
    sendJson(res, 200, { ok: true, service: 'scanner-api' })
    return
  }

  applyCors(req, res)
  sendJson(res, 404, { success: false, error: 'not found' })
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`scanner-api listening on http://127.0.0.1:${PORT}`)
  console.log(`  POST /api/scan  body: { "repoUrl": "..." }`)
})
