export const meta = {
  name: 'clipline-review',
  description: 'Adversarial review of clip-line workspace before first public push',
  phases: [
    { title: 'Find', detail: '4 lenses over the diff' },
    { title: 'Verify', detail: '3-skeptic panel per finding' },
  ],
}

const FINDING = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          line: { type: 'number' },
          severity: { type: 'string', enum: ['critical', 'major', 'minor'] },
          desc: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['file', 'severity', 'desc', 'fix'],
      },
    },
  },
  required: ['findings'],
}

const VERDICT = {
  type: 'object',
  properties: { real: { type: 'boolean' }, reason: { type: 'string' } },
  required: ['real', 'reason'],
}

const ROOT = 'C:\\Users\\asd\\Documents\\rea_clipdiary\\clip-line'

const COMMON = `Repo: ${ROOT} (Rust workspace, about to go public on GitHub). Read the files yourself. Your output is machine-consumed; finish by calling StructuredOutput. This machine's Bash output passes through an "rtk" filter that corrupts multi-line output — prefer Read/Grep/Glob tools; if you must run a command, redirect to a unique file in ${ROOT}\\..\\ and Read it.`

phase('Find')

const LENSES = [
  { name: 'correctness', prompt: `${COMMON}
Lens: CORRECTNESS. Review crates/clip-line-core/src (store.rs, envelope.rs, capture.rs, config.rs) and crates/clip-line-win/src for real bugs: SQL injection via string-built queries, envelope packing/unpacking mismatches vs docs/interop.md, UTF-16 handling, gzip edge cases, mutex poisoning paths, dedup logic flaws, import_legacy escaping (ATTACH path quoting). Only report defects that change behavior; no style.` },
  { name: 'windows-api', prompt: `${COMMON}
Lens: WIN32 API USE. Review crates/clip-line-win/src + tests for unsafe correctness: handle leaks (GlobalAlloc'd HGLOBAL after SetClipboardData success/failure — who owns it?), message-loop lifetime, CreateWindowExW failure when RegisterClassW failed, GetClipboardData null-handle semantics under windows crate 0.62 Result mapping, SendInput struct sizes, clipboard open/close pairing on every path.` },
  { name: 'ci-release', prompt: `${COMMON}
Lens: CI + RELEASE. Review .github/workflows/*.yml against current GitHub Actions reality: action versions used (checkout@v7.0.1, cache@v6.1.0, upload-artifact@v7.0.2, upload-pages-artifact@v5.0.0, deploy-pages@v5.0.1, configure-pages@v6.0.0, download-artifact@v7.0.1 — check these exist: run "gh api repos/<owner>/<repo>/releases/latest --jq .tag_name" via a redirected file if needed), permissions blocks, Pages job needs, cache path on Windows runner, release packaging paths (does target/release/clip-line-headless exist on Windows as .exe — does the unix cp fail on macos for headless?), workflow_dispatch triggers.` },
  { name: 'docs-oss', prompt: `${COMMON}
Lens: DOCS + OSS HYGIENE. Review README.md, docs/site/index.html, docs/interop.md, CONTRIBUTING.md, SECURITY.md, CHANGELOG.md: broken links (relative + absolute), badges (crates.io badge for a crate that may not be published yet — misleading?), instructions that cannot work on a fresh clone (cargo install --path path correctness, prereqs), Pages HTML asset paths, license/attribution claims, .gitignore missing entries (demo.db committed in docs/assets — should it be?), anything factually wrong.` },
]

const rounds = await parallel(LENSES.map(l => () => agent(l.prompt, { label: l.name, phase: 'Find', schema: FINDING })))
const all = rounds.filter(Boolean).flatMap(r => r.findings)
const seen = new Set()
const fresh = all.filter(f => {
  const k = `${f.file}|${f.line || 0}|${f.desc.slice(0, 60)}`
  if (seen.has(k)) return false
  seen.add(k)
  return true
})
log(`${all.length} raw findings, ${fresh.length} after dedup`)

phase('Verify')
const judged = await parallel(fresh.map(f => () =>
  parallel([1, 2, 3].map(i => () => agent(
    `${COMMON}
You are skeptic #${i}. Try to REFUTE this code-review finding by reading the actual file. Finding: [${f.severity}] ${f.file}:${f.line || '?'} — ${f.desc}. Proposed fix: ${f.fix}. Is it a real defect that matters for a public first release, or a false positive / works-as-intended / trivial? real=true only if a maintainer should fix it BEFORE the first public push.`,
    { label: `verify:${(f.file || '').split(/[\\\/]/).pop()}`, phase: 'Verify', schema: VERDICT }
  ))).then(vs => ({ f, votes: vs.filter(Boolean).filter(v => v.real).length, total: vs.filter(Boolean).length }))
))
const confirmed = judged.filter(j => j.votes >= 2).map(j => ({ severity: j.f.severity, file: j.f.file, line: j.f.line, desc: j.f.desc, fix: j.f.fix, votes: `${j.votes}/${j.total}` }))
const rejected = judged.filter(j => j.votes < 2).map(j => j.f.desc.slice(0, 80))
log(`${confirmed.length} confirmed, ${rejected.length} rejected`)
return { confirmed, rejected }
