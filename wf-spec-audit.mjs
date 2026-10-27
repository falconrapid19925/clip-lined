export const meta = {
  name: 'spec-fidelity-audit',
  description: 'Blind independent audit: does GH issue #1 spec match the grilling decisions in session transcript?',
  phases: [
    { title: 'Audit', detail: '3 blind judges compare spec vs session transcript' },
    { title: 'Synthesize', detail: 'merge verdicts' },
  ],
}

const SCHEMA = {
  type: 'object',
  properties: {
    missing: { type: 'array', items: { type: 'string' }, description: 'Decisions/requirements from the session absent or watered down in the spec' },
    overengineered: { type: 'array', items: { type: 'string' }, description: 'Spec items with no basis in session decisions, or gold-plating' },
    distorted: { type: 'array', items: { type: 'string' }, description: 'Spec items that contradict or misstate a session decision' },
    verdict: { type: 'string', enum: ['faithful', 'minor-gaps', 'major-divergence'] },
    notes: { type: 'string' },
  },
  required: ['missing', 'overengineered', 'distorted', 'verdict', 'notes'],
  additionalProperties: false,
}

const SPEC_PATH = 'C:/Users/asd/Documents/rea_clipdiary/issue1_copy.md'
const SESSION_PATH = 'C:/Users/asd/Documents/rea_clipdiary/session_user_messages.txt'

const COMMON = `You are a blind, independent spec auditor. You have NO prior context on this project. Judge only from the two files below. Your output is machine-consumed; finish by calling StructuredOutput. This machine's Bash passes through an "rtk" filter that corrupts output — use the Read tool for files, never cat.

SPEC (a GitHub issue describing planned work): ${SPEC_PATH}
SESSION TRANSCRIPT (user messages from the design conversation that produced the spec — includes user answers to design questions Q1-Q17, skill instructions the user invoked, and requirements; note skill preamble blocks like "Base directory for skill" are injected context, the user's actual words are their command arguments and replies): ${SESSION_PATH}

Your job: decide whether the spec is 1:1 with what the user actually decided and asked for. Three buckets:
- missing: user decisions/requirements that the spec omits, defers without user signoff, or waters down. Check the user's answers to Q1-Q17 and every explicit want (e.g. platforms, Rust, public repo, look-and-function parity, subagent usage, spec-on-paper rule, grilling flow). Also check spec-template completeness: Problem Statement, Solution, User Stories, Implementation Decisions, Testing Decisions, Out of Scope, Further Notes.
- overengineered: spec items nobody asked for, or scope inflated beyond the decisions (e.g. features marked as core that the session treated as later-phase, invented requirements, excessive process).
- distorted: spec statements that contradict a session decision (wrong priority, wrong platform support claim, wrong name/license, wrong testing approach, wrong module/ownership rule).

Be specific: cite the user message and the spec line for every item. If a bucket is empty, return an empty array — do not invent findings to look thorough. verdict: faithful if all three buckets empty; minor-gaps if only minor omissions; major-divergence if any decision is contradicted or a locked decision is missing.`

phase('Audit')
const LENSES = [
  'Focus: the locked decisions Q1-Q17. Reconstruct the decision table from the transcript, then check every cell against the spec.',
  'Focus: the user\u2019s explicit wants across the whole transcript: platforms ("mac linux ... all compatibility os"), Rust, public repo, look and function same, subagent speed, spec-on-paper rule, grilling skill usage, name and license choices, anything about the GH repo (modernization, CI/CD, README, Pages, community files, release). Check each landed in the spec in the right form and phase.',
  'Focus: spec quality as an agent work order per the to-spec template (Problem/Solution/Stories/Implementation/Testing/Out of Scope/Further Notes) AND scope honesty: is anything in the spec gold-plating or inventing work the user never approved? Are user stories traceable to session decisions or invented filler?',
]
const results = await parallel(LENSES.map((lens, i) => () => agent(`${COMMON}\n\nYour lens: ${lens}`, { label: `judge-${i + 1}`, phase: 'Audit', schema: SCHEMA })))

phase('Synthesize')
const valid = results.filter(Boolean)
const merged = {
  missing: [...new Set(valid.flatMap(r => r.missing))],
  overengineered: [...new Set(valid.flatMap(r => r.overengineered))],
  distorted: [...new Set(valid.flatMap(r => r.distorted))],
  verdicts: valid.map(r => r.verdict),
  notes: valid.map(r => r.notes).join('\n---\n'),
}
return merged
