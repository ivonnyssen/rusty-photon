# session-runner — Generic Workflow Orchestrator Design

## Overview

`session-runner` is an orchestrator plugin that executes **workflow
documents** — declarative JSON descriptions of an imaging session — against
`rp`'s MCP tool catalog. One generic engine replaces the need for a
hand-written Rust orchestrator per session type: the deep-sky night, the
flat-calibration run, and the twilight sky-flat session become documents,
not binaries. Rust orchestrators (such as today's `calibrator-flats`)
remain first-class citizens of the unchanged plugin protocol; the DSL is an
addition, not a replacement.

Decision record and phase plan:
[`docs/plans/workflow-dsl.md`](../plans/workflow-dsl.md).

### Tenets

1. **Documents describe procedure and reaction, never choice or safety.**
   Target/filter selection is delegated to `rp`'s planner tools; safety
   enforcement belongs exclusively to `rp` and cannot be expressed, delayed,
   or overridden by a document.
2. **Position is derived, not stored.** The engine never persists "where it
   is" in the tree. Resume re-executes the document from the root against
   the persisted blackboard and live device state; documents are written
   re-entrant (see [Re-entrancy Contract](#re-entrancy-contract)).
3. **Everything validates before anything moves.** A document is checked
   against the workflow schema, and every tool call in it is checked against
   `rp`'s live tool catalog, before the first instruction runs. Authoring
   errors surface at load, at the `/validate` endpoint, or at session start
   — never in the middle of the night.
4. **Expressions are pure — with one sanctioned exception.** The bounded
   expression language reads document state and computes values. It cannot
   perform I/O, call tools, or loop. The single sanctioned observation of
   the outside world is `seconds_until()`, which reads the engine clock at
   evaluation time — dawn/flip math is impossible without it (see
   [Expressions](#expressions)). Everything else effectful is an
   instruction.
5. **The document format is the API.** The published JSON Schema is the
   contract for hand-authors, the future `ui-htmx` editor, and LLM
   generation alike. The engine's internals may change; the format versions
   deliberately.

## Architecture

`session-runner` is a standalone HTTP service following the orchestrator
plugin protocol defined in [`rp.md`](rp.md): `rp` POSTs `/invoke` when a
session starts, the plugin connects back to `rp`'s MCP server and drives the
session with tool calls, and posts a completion when it finishes. In
addition, `session-runner` subscribes to `rp`'s SSE event stream to feed
trigger evaluation.

```
  rp (equipment gateway)              session-runner (generic orchestrator)
  ┌─────────────────────┐             ┌──────────────────────────────┐
  │                     │ POST /invoke│  load document               │
  │  session start ─────┼────────────►│  validate vs schema + tools  │
  │                     │             │  load/restore blackboard     │
  │  MCP server    ◄────┼─────────────┤  execute procedure tree      │
  │  /mcp               │  tool calls │   ├─ instructions            │
  │                     │             │   └─ trigger actions         │
  │  SSE            ────┼────────────►│  evaluate triggers           │
  │  /api/events/       │   events    │                              │
  │  subscribe          │             │                              │
  │                     │             │                              │
  │  REST API      ◄────┼─────────────┤  post completion             │
  │  /api/plugins/      │  completion │                              │
  │  {wf_id}/complete   │             │                              │
  └─────────────────────┘             └──────────────────────────────┘
```

### Port

11171 (configurable) — in the orchestrator-plugin range next to
`calibrator-flats` (11170).

## Workflow Documents

A workflow document is a single JSON file. Top-level structure:

```jsonc
{
  "version": 1,
  "name": "deep-sky",
  "description": "Classic multi-target deep-sky imaging session",
  "parameters": {
    "camera_id":   { "type": "string", "required": true },
    "focuser_id":  { "type": "string", "required": true },
    "dither_every": { "type": "integer", "default": 3 }
  },
  "estimated_duration": "8h",
  "max_duration": "12h",
  "triggers": [ /* trigger objects — see Triggers */ ],
  "root": { "sequence": [ /* instructions — see Instructions */ ] }
}
```

| Field | Meaning |
|-------|---------|
| `version` | Format version. The engine rejects documents whose version it does not implement, naming the supported version(s) in the error. |
| `name` / `description` | Identification for logs, events, and the completion payload. |
| `parameters` | Declared invocation parameters. Each has a `type` (`string`, `integer`, `number`, `boolean`, `duration`, `array`), and either `required: true` or a `default`. Values supplied at invocation are type-checked against the declaration; missing required parameters fail validation before anything runs. Available to expressions as `params.*`. Names beginning with `_` are reserved for the engine — declaring one is a validation error. An `array` parameter is an opaque JSON array in v1 — no element shape is declared, so element errors surface as loud expression errors at run time (typed element declarations are deferred; see [MVP Scope](#mvp-scope)). Durations stay humantime strings — expressions read them via `seconds()`. |
| `estimated_duration` / `max_duration` | The acknowledgment durations returned to `rp` from `/invoke` (humantime strings). Optional; engine defaults apply when absent (see [Invocation](#invocation)). |
| `triggers` | Document-global reactive rules, evaluated alongside the procedure tree. |
| `root` | The procedure tree — conventionally a `sequence` container, though any instruction is structurally valid as the root. |

### Instructions

The v1 instruction vocabulary. Every instruction is a JSON object with
exactly one *discriminant* key (`tool`, `sequence`, `repeat`, `if`, `set`,
`try`, `fail`, `wait`, `log` — plus `script`, reserved but rejected in v1)
plus the optional common keys `id` (a string used in
logs and error messages) and `once` (see
[Re-entrancy Contract](#re-entrancy-contract)). Unknown keys are a
validation error — misspellings must not silently no-op.

#### `tool` — call an MCP tool

```jsonc
{ "tool": "capture",
  "args": { "camera_id": { "$expr": "params.camera_id" },
            "duration": "300s" },
  "retry": { "max_attempts": 3, "backoff": "10s" } }
```

- `args` values are **literal JSON by default**. A value that must be
  computed is wrapped as `{ "$expr": "<expression>" }`. This keeps literal
  strings (humantime durations, filter names) unambiguous and lets static
  validation type-check every literal argument against the tool's JSON
  Schema. `$expr` is recognized only as a **direct** argument value; a
  `$expr` key nested anywhere inside a literal value is a validation
  error — letting it pass as data would silently send the wrapper object
  to the tool, exactly the no-op misspelling the format forbids.
- The tool's structured result becomes `result` for the instructions that
  follow (see [`result` scoping](#result-scoping)).
- Optional `retry`: on tool error, retry up to `max_attempts` total attempts
  with a fixed `backoff` delay between attempts. Default: no retry.
- A tool error (after retries) raises a workflow error that propagates
  outward through enclosing `try` instructions (see `try`), ultimately
  failing the workflow.
- If the tool result carries a **correction** (`rp` returns
  `pending_correction` on natural completion or `status: "aborted"` +
  `correction` on an immediate correction, per `rp.md` § Corrections), the
  engine fires the synthetic `correction_requested` trigger source (see
  [Triggers](#triggers)) with the correction as the event payload, then
  continues. An `aborted` tool result is **not** a workflow error — the
  document decides how to react via a trigger.

#### `sequence` — ordered container

```jsonc
{ "sequence": [ /* instructions, executed in order */ ] }
```

There is deliberately no parallel container in v1 (research finding:
marginal value; device-level concurrency already lives inside `rp` tools
such as `capture`-while-guiding).

#### `repeat` — loop

```jsonc
{ "repeat": { "until": "abs(result.median_adu - session.target_adu) / session.target_adu <= 0.05",
              "max_iterations": 10 },
  "body": [ /* instructions */ ] }
```

- Exactly one of `until` (expression, checked **after** each pass), `while`
  (expression, checked **before** each pass), or `count` (integer or
  `$expr`) is required.
- `max_iterations` (integer or `$expr` — evaluated once at loop entry, and
  a workflow error unless it yields a positive integer) is **required**
  alongside `until`/`while` — unbounded loops are a validation error. When
  `max_iterations` is exhausted without the condition being met, the loop
  still *completes*, with `result.converged = false` (see
  [`result` scoping](#result-scoping)); the document
  decides whether that is fatal (an `if` + `fail` pattern) or a warning
  (`log`). This mirrors `calibrator-flats`' non-converged-exposure warning
  behavior.
- Bound evaluation pins: a `$expr` bound must yield an integer-valued
  number (`2.0` from a tool result counts; `2.5` or a string is a workflow
  error at loop entry); `count` may be zero (zero passes), `max_iterations`
  must be ≥ 1. A `while` condition is also evaluated once *after* the
  final permitted pass, so a condition that turns false exactly at the
  budget still completes with `result.converged = true` — `converged =
  false` means the budget ran out while the condition still requested
  another pass. On a `count` loop, `max_iterations` is a guard against a
  runaway `$expr` count: if the evaluated `count` exceeds it, the loop
  fails loudly at entry rather than silently truncating the pass count.

#### `if` — conditional

```jsonc
{ "if": "event.hfr > session.last_focus_hfr * 1.2",
  "then": [ /* instructions */ ],
  "else": [ /* optional */ ] }
```

#### `set` — write the blackboard

```jsonc
{ "set": { "session.last_focus_hfr": "result.best_hfr",
           "session.target_adu": "result.max_adu * params.target_fraction" } }
```

- Keys must be `session.*` paths; values are always expressions.
- All values are evaluated before any key is written — a `set` cannot read
  its own writes. Because of this, keys within one `set` must not
  **overlap** (no key may be a path prefix of another, e.g. `session.a`
  alongside `session.a.b`) — the write order would be ambiguous;
  validation rejects the overlap.
- Writing a nested key creates missing (or `null` — the same thing, in
  `has()`'s view) intermediate objects; an intermediate that exists as a
  **non-object** (`session.a.b` when `session.a` is a number) is a
  workflow error — silently discarding the scalar would hide a document
  bug.
- `set` is the **only** way state crosses instruction boundaries or survives
  a crash. `result` is transient by design — anything worth keeping is
  copied to the blackboard explicitly, which makes the resume semantics
  visible in the document itself.
- Each `set` persists the blackboard atomically before the next instruction
  runs (see [Blackboard](#blackboard-and-persistence)).

#### `try` — cleanup and error handling

```jsonc
{ "try": [ /* body */ ],
  "catch": [ /* optional: runs on body error; error.* in scope */ ],
  "finally": [ /* optional: always runs */ ] }
```

- Semantics follow the `calibrator-flats` cleanup guard: `finally` runs
  whether the body succeeded, failed, or was cancelled by safety — with the
  caveat that after a safety cancellation `rp` has already secured the
  equipment and torn down the MCP session, so `finally` instructions that
  call tools will themselves fail; the engine runs them best-effort, logs
  each failure, and does not let a `finally` failure mask the original
  error.
- `catch` handles the error (the workflow continues after the `try`) unless
  it re-raises via `fail`. In `catch` and `finally` (on the error path),
  expressions can read `error.message`, `error.instruction_id`, and
  `error.tool` (null when the error was not a tool error).
  `error.instruction_id` is the raising instruction's **own** `id` (null
  when it declares none), not a nearest-ancestor id.
- `error.*` names the error the nearest enclosing error path is handling:
  a `catch` (or an error-path `finally`) binds it for its block and the
  enclosing scope's value is restored afterwards; a success-path `finally`
  leaves the enclosing scope's value visible (so a `finally` nested inside
  an outer `catch` still reads the outer error). Where no error is being
  handled, `error.*` reads as `null` — `has(error.message)` is the guard.
- `finally` failure semantics: on the success path a `finally` failure is
  a real workflow error; on the error path it is logged and the original
  error propagates (never masked); a safety termination during `finally`
  supersedes everything. A safety termination also skips `catch` entirely
  — by then `rp` has secured the equipment and torn down the MCP session,
  so there is nothing left to handle; only `finally` runs (best-effort).

#### `fail` — raise a workflow error

```jsonc
{ "fail": { "message": "'exposure never converged'" } }
```

Accepted anywhere an instruction is (`catch`, `then`, `else`, a `repeat`
body, …) and raises a workflow error deliberately; inside `catch` it
re-raises, propagating the failure outward. `message` is an expression —
quote it (as above) for a fixed string. A non-string message value is
rendered as compact JSON (an error message is terminal output, not data).

#### `wait` — pause at a safe point

```jsonc
{ "wait": { "duration": "30s" } }
{ "wait": { "until_event": "guide_settled", "timeout": "5m" } }
{ "wait": { "until": "seconds_until(session.flip_at) <= 0", "poll_interval": "10s", "timeout": "2h" } }
```

- Exactly one of `duration`, `until_event` (an `rp` event name), or `until`
  (expression re-evaluated every `poll_interval`, default `"10s"`).
- `until_event` and `until` require a `timeout`; expiry raises a workflow
  error. Triggers keep firing during a `wait` — a `wait` is one long safe
  point.
- An `until` condition is evaluated on entry, after each `poll_interval`,
  and one final time exactly when the timeout expires (the last sleep is
  clamped to the remaining budget) — only then does expiry raise. The
  timeout budget is measured by accumulated sleep time (monotonic), so a
  wall-clock adjustment (NTP step) can neither fire a timeout early nor
  extend a wait; the wall clock feeds only `seconds_until()`, where
  calendar time is the point.

#### `log` — operator-visible message

```jsonc
{ "log": { "level": "info",
           "message": "exposure converged",
           "values": { "duration": "session.duration" } } }
```

`level` is `debug` (default) or `info`, matching the workspace logging rule
(`info` only where the operator derives clear value). `values` entries are
expressions, rendered into the structured log record.

#### Reserved: `script`

`{ "script": … }` is **reserved** for a future sandboxed Luau handler node
(decision D3 in the plan). v1 validation rejects it with an explicit
"reserved for a future format version" error rather than "unknown key", so
documents written against a future version fail comprehensibly on an old
engine.

### `result` scoping

`result` is always the structured result of the most recently completed
result-producing instruction on the current execution path. Concretely:

- `tool` calls produce their structured result. A completed `repeat`
  produces a loop summary — `result.iterations`, plus `result.converged`
  for `until`/`while` loops (`true` when the condition was met, `false`
  when `max_iterations` ran out).
- `set`, `log`, and `wait` produce no result and leave `result` unchanged;
  containers (`sequence`, `if`, `try`) leave whatever the last instruction
  executed inside them left. In particular, the first instruction of a
  `then`/`else`/`catch`/`finally` block sees the `result` that was in
  scope when the branch was taken — an `if` condition reads `result`
  without consuming it.
- A `repeat`'s `until` expression is evaluated with the `result` left by
  the pass that just completed; `while` is evaluated with the `result` in
  scope before the upcoming pass.
- `result` is `null` at the start of a session and at the start of a
  trigger `do` block.

### Triggers

Triggers are the reactive overlay: cross-cutting rules that fire while the
procedure tree runs. They are declared at the document top level.

```jsonc
{
  "id": "refocus-on-hfr-degradation",
  "on": { "event": "exposure_complete" },
  "when": "session.last_focus_hfr != null",
  "while": "session.imaging == true",
  "cooldown": "15m",
  "do": [
    { "tool": "measure_basic", "args": { "document_id": { "$expr": "event.document_id" } } },
    { "if": "result.hfr != null && result.hfr > session.last_focus_hfr * 1.2",
      "then": [
        { "tool": "auto_focus", "args": { /* … */ } },
        { "set": { "session.last_focus_hfr": "result.best_hfr" } } ] }
  ]
}
```

(`exposure_complete`'s payload carries `document_id` / `file_path` only —
`rp.md` § Events — so the trigger measures HFR itself before deciding; the
`when` gate keeps it idle until the first `auto_focus` has seeded
`session.last_focus_hfr`, and `cooldown` bounds how often the measurement
itself runs.)

| Field | Meaning |
|-------|---------|
| `id` | Required, unique within the document. Names the trigger in logs and in the `session._triggers.*` bookkeeping state. |
| `on` | The source. `{ "event": "<rp event name>" }` — an envelope from the SSE stream; the envelope's `payload` becomes `event.*`. `{ "poll": { "tool": "<name>", "args": { … }, "interval": "30s" } }` — the engine calls the tool on the interval and the result becomes `event.*`. `{ "event": "correction_requested" }` — the synthetic source fired when a tool result carries a correction (payload: `event.action`, `event.reason`, `event.source`, plus engine-synthesized `event.delivery` — `"immediate"` when the tool result was `aborted`, `"after_current"` for `pending_correction`). |
| `when` | Optional expression over `event.*` + the usual namespaces. The trigger fires only when it evaluates to `true`. Absent = always fire. |
| `while` | Optional expression gate evaluated at fire time; lets a document scope a trigger to a phase via a blackboard flag (e.g. set `session.imaging = true` when the capture loop starts). This is the v1 substitute for container-scoped triggers, which are deferred. |
| `cooldown` | Optional minimum interval between firings (humantime). Last-fired timestamps live in the blackboard, so cooldowns survive resume. |
| `once` | Optional boolean: fire at most once per session (recorded in the blackboard). |
| `do` | Instructions, same vocabulary as the tree (nested triggers excepted). |

**Interleaving contract (safe points).** Trigger `do` blocks never preempt
an in-flight instruction. When a trigger fires, its action is queued; queued
actions run at the next **safe point** — after the current instruction
completes, or continuously during a `wait`. Multiple queued triggers run in
document order. A trigger whose action is queued or running does not queue
again. Only `rp` (safety) and Sentinel (watchdog) ever abort an in-flight
operation; a document that wants "stop the current exposure when X" reacts
to the *correction* `rp` delivers rather than aborting on its own.

**Poll lifecycle.** Poll triggers start when the workflow starts and stop
when it completes or is cancelled. A poll whose tool call fails logs at
`debug!` and skips that cycle — a flaky poll must not kill the session.

### Expressions

Expressions are strings in a small, pure, CEL-style language. They appear in
`when` / `while` / `until` / `if` / `set` values / `$expr` wrappers /
`fail.message` / `log.values`.

**Namespaces** (read-only except via `set`):

| Namespace | Contents | Lifetime |
|-----------|----------|----------|
| `params.*` | Invocation parameters after defaulting/type-check; on recovery invocations the engine also injects `params._recovery.*` ([Re-entrancy Contract](#re-entrancy-contract)). | Session (immutable). |
| `session.*` | The blackboard. | Session (persisted, survives crash/resume). |
| `result.*` | Structured result of the most recent result-producing instruction ([`result` scoping](#result-scoping)). | Until the next result-producing instruction. |
| `event.*` | The triggering envelope payload (in trigger `when`/`do`) or poll result. | One trigger firing. |
| `error.*` | `message`, `instruction_id`, `tool` inside `catch`/`finally`. | One error scope. |

`event.*` is only in scope in a trigger's `when` / `while` / `do`;
`error.*` only inside `catch` / `finally` blocks. Referencing either
anywhere else is a **load-time validation error** (the engine knows
statically the value could never be non-null there), pointing at the
offending namespace root within the expression.

**Semantics:**

- Types: `null`, boolean, number (f64), string, plus JSON arrays/objects
  from tool results (member access and indexing only).
- Operators: `== != < <= > >=`, `+ - * / %`, `&& || !`, `?:` (conditional),
  parentheses.
- Functions (v1): `abs`, `min`, `max`, `clamp(x, lo, hi)`, `floor`, `ceil`,
  `round`, `seconds("1m30s")` (humantime string → f64 seconds),
  `humantime(secs)` (f64 seconds → humantime string, for building tool
  args), `has(session.x)` (path exists and is non-null),
  `seconds_until("<RFC3339>")` (evaluated against the engine's clock at
  evaluation time — the one sanctioned exception to tenet 4's purity rule,
  needed for dawn/flip math).
- **No** loops, user function definitions, assignment, tool calls, string
  interpolation, or regular expressions. Anything effectful is an
  instruction; anything algorithmic beyond this belongs in a built-in `rp`
  tool or (future) a `script` node.
- Accessing a missing path yields `null`; `null` in arithmetic or comparison
  (other than `==`/`!=`) raises an expression error → workflow error at
  that instruction. Authors guard with `has()` / `!= null` (as the trigger
  example above does). This is deliberate: silent `null` propagation in a
  system that moves telescopes is worse than a loud 2 a.m. error.
- Division or remainder by zero raises. There is no implicit type
  coercion.

**Grammar pins** (fixed by the Phase B parser spike, 2026-07-03):

- **Number literals use JSON number syntax, unsigned** (`-` is the unary
  operator): a leading digit is required (`0.5`, not `.5`), digits must
  follow the decimal point (`5.0`, not `5.`), exponents are allowed
  (`1.5e-3`), and leading zeros, hex/octal/binary forms, `_` separators,
  and bigint suffixes are rejected. A literal that overflows f64 is a
  parse error.
- **Strings** are `'…'` or `"…"` (single quotes are the ergonomic choice
  inside JSON documents) with exactly the escapes
  `\\ \' \" \n \r \t \uXXXX`; raw newlines and any other escape are
  errors.
- **Identifiers** (namespace roots and `.`-fields) are ASCII
  `[A-Za-z_][A-Za-z0-9_]*`. `null` / `true` / `false` are reserved words
  and cannot be field names — use `['null']` indexing for such keys.
  Other keys (unicode, `$`, spaces) are reachable the same way.
- **Comparison and equality operators form one non-chaining precedence
  level**: `a < b < c` and `a == b < c` are parse errors ("comparison
  operators cannot be chained"); parenthesize explicitly. Rationale: CEL
  groups `a == b < c` as `(a == b) < c` while JavaScript groups it as
  `a == (b < c)` — the format refuses the ambiguity instead of inheriting
  either convention.
- **Precedence** (tightest first): postfix (`.` `[]` call) → unary
  (`!`, `-`) → `* / %` → `+ -` → comparisons (non-chaining) → `&&` →
  `||` → `?:` (right-associative).
- **No comments**, no unary `+`, no `--`/`++` (write `-(-x)` for double
  negation), no trailing commas in argument lists, and calls only on bare
  built-in function names (no method-call syntax).
- `min` / `max` take two or more arguments; every other function's arity
  is fixed at its signature.
- **Nesting depth is capped at 64 levels** (parentheses, unary runs,
  argument lists, ternary branches). No legitimate expression comes near
  this; without the cap, adversarially deep input overflows the parser
  stack (found by the fuzz target).

**Evaluation pins** (fixed by the Phase B implementation, 2026-07-04):

- **Path traversal is total** — it never raises. Member/index access
  through `null`, a missing key, an out-of-range / negative / non-integer
  array index, a non-string object key, or a value of the wrong shape
  yields `null`. `has(path)` is true iff the path resolves to non-null
  (an explicit JSON `null` value counts as absent). The loudness comes
  when the `null` reaches an operator or function, per the null rule
  above.
- **Arithmetic and ordered comparisons are numbers-only.** `+` does not
  concatenate strings; `< <= > >=` on strings is a type error (string
  ordering is deliberately undefined — `==` / `!=` are the string
  comparisons).
- **Runtime overflow raises at the producing operation**: any `+ - * / %`
  result outside the finite f64 range is an evaluation error there (not
  at `set` persistence). Together with finite literals, JSON-sourced
  values, and the division/remainder-by-zero rule this makes ±Infinity
  and NaN unrepresentable — every number in the system is finite.
- **No truthiness.** `&&` / `||` / `!` and the `?:` condition require
  booleans. `&&` / `||` short-circuit left to right and `?:` evaluates
  only the taken branch — this is what makes
  `has(session.x) && session.x > 0` a sound guard.
- **Equality is deep and total.** `==` / `!=` accept any two values:
  structural for arrays/objects, numbers by numeric value regardless of
  JSON representation (a tool result's integer `5` equals the literal
  `5`), cross-type comparison is `false`, never an error.
- `%` is the f64 remainder (sign follows the dividend); `round()` rounds
  half away from zero; `clamp(x, lo, hi)` raises if `lo > hi`;
  `humantime(n)` requires a non-negative in-range number;
  `seconds_until(s)` requires an RFC 3339 string and is measured against
  the engine clock injected into the evaluation context (never the wall
  clock directly), so evaluation stays deterministic and testable.

The implementation (`src/expr/`) is a **hand-rolled lexer + Pratt parser
with a hand-written evaluator** — the parser is dependency-free; the
evaluator sits on the workspace's `serde_json` (values), `humantime`
(`seconds`/`humantime`), and `chrono` (`seconds_until`). The Phase B
spike compared this against reusing the `cel` crate's parser and an
`oxc_parser` JS-expression subset on a 178-case conformance corpus: the
cel parser silently collapses unary-operator runs (`- -x` → `-x`) and
cannot enforce the pins above from its AST; the oxc subset can, but only
with wrapper lexical checks approximating the hand lexer on top of 73
dependencies. See the plan's Phase B spike outcome for the full
evidence. The corpus ships as the module's conformance suite, alongside
proptest round-trip/no-panic properties and a cargo-fuzz target
(`services/session-runner/fuzz/`, standalone workspace, run with
`cargo +nightly fuzz run expr_parse`). Parse-time errors (lexing,
parsing, static checks: namespace roots, known functions and arities,
`has()` path arguments) and evaluation errors share one serializable
error type carrying a byte span into the expression source, for mapping
to JSON-Pointer locations in `/validate` responses.

## Blackboard and Persistence

The blackboard (`session.*`) is the workflow's only mutable state. It is a
JSON object persisted to `<state_dir>/<session_id>.json` with the workspace
atomic-write pattern (sibling temp file, fsync, rename, fsync parent
directory — same as `rp`'s exposure documents).

Writes happen after **every** mutation: each `set`, each `once` completion
marker, each trigger bookkeeping update (cooldown timestamps, once flags).
Mutations are small and infrequent (human-scale session cadence), so write
amplification is a non-issue; the invariant "the file always reflects every
completed `set`" is what makes tenet 2 sound.

Engine bookkeeping lives under reserved keys the schema forbids documents
from setting directly: `session._once.*` (completed once-markers),
`session._triggers.<id>.*` (last-fired, fired-once flags).

The file is deleted when a session completes and the completion has been
acknowledged by `rp`; a leftover file at `/invoke` time for a **new**
session (no `recovery` context) is deleted **eagerly**, before the run
starts — lazy replacement on first persist would not be enough, because a
safety termination before the first write must not leave a stale file
(stale `_once` markers included) to be mistaken for this session's state
on the recovery invocation.

## Re-entrancy Contract

Resume is re-execution: on a recovery invocation, the engine reloads the
blackboard and runs the document from the root. For this to continue the
session rather than repeat it, documents must be **re-entrant**:

> Running the document from the top with the persisted blackboard and the
> current device state must converge to *continuing* the session, not
> redoing completed work.

The format provides three tools for this, in preference order:

1. **Dispatch-driven loops.** A capture loop that asks `get_next_target`
   and records progress with `record_exposure` is naturally re-entrant —
   `rp`'s persisted progress counters *are* the resume state. This is the
   ecosystem lesson (Ekos counts frames on disk; Target Scheduler keeps a
   DB) applied through `rp`'s planner.
2. **Idempotent procedure.** Startup steps that are safe to repeat (cool
   the camera to a setpoint, unpark, connect) simply re-run.
3. **`once` markers** for steps that are *not* safe or sensible to repeat:

   ```jsonc
   { "tool": "calibrator_on", "args": { "calibrator_id": "flat-panel" },
     "once": "panel-on" }
   ```

   When the instruction completes **successfully**, `session._once["panel-on"]`
   is recorded (a failed instruction re-runs on resume); on re-execution
   the instruction is skipped. A skipped instruction produces nothing and
   leaves `result` unchanged — a following instruction that reads `result`
   must not assume the marked instruction just ran (that assumption is
   itself a re-entrancy bug). `once` keys must be unique
   within a document (validated). Use sparingly — a document that needs many
   `once` markers is usually missing a dispatch loop.

Resume behavior at `/invoke` with a non-null `recovery`:

1. Reload the blackboard for `session_id`. A missing blackboard file is not
   an error — the document starts with an empty `session.*` (first-run
   equivalent), because a crash can predate the first `set`.
2. Re-validate the document against the live tool catalog (equipment may
   have changed across the outage).
3. Log the recovery context (`reason`, interruption time) at `info!` and
   expose it as `params._recovery.*` so a document *may* branch on it
   (e.g. re-run `center_on_target` after any interruption) — but a correct
   document does not need to.
4. Execute from the root.

## Safety Behavior

On an unsafe transition `rp` — not `session-runner` — aborts exposures,
stops guiding, parks the mount, and terminates the plugin's MCP session
(per `rp.md` § Safety). From the engine's perspective: the in-flight tool
call fails with a terminated-session error. (MCP client pin: a call that
*returns* with the MCP `is_error` flag is a tool failure — retryable and
catchable; **any request-level failure** — transport loss *or* a JSON-RPC
protocol error — is the terminated-session error, never retried, never
caught. `rp` reports tool failures via `is_error` results, so a protocol
error means `rp` itself is unhealthy, and the engine's response — persist,
exit without completion, await re-invocation — is the safest generic
recovery. Tool results arrive as one JSON text content block: no content
is a `null` result; anything else — non-JSON text, a non-text block, or
multiple blocks — is a loud tool failure rather than a silently dropped
or stringified result.) The engine
then:

1. Stops trigger evaluation and abandons queued trigger actions.
2. Runs any enclosing `finally` blocks best-effort (their tool calls will
   fail; failures are logged, not raised).
3. Persists the blackboard (already current, by the write-on-mutation
   invariant).
4. Exits the run **without** posting a completion — the session is not
   over; `rp` re-invokes with recovery context on the safe transition, and
   the [re-entrancy contract](#re-entrancy-contract) takes it from there.

A document cannot subscribe to `safety_changed` to *countermand* any of
this; it may subscribe to it (e.g. to `log`), but by the time the trigger
would run, the MCP session is gone. Safety-reaction logic in documents is a
smell the authoring docs will warn about.

## Event Subscription

The engine consumes `rp`'s SSE stream (`/api/events/subscribe`) for trigger
sources. The SSE `id` is the envelope's `event_seq`; on reconnect the engine
sends `Last-Event-ID`, and replay is exact within `rp`'s retention window
(the most recent 512 envelopes — `rp.md` § Real-Time Stream). If the engine
was gone long enough that its cursor was evicted, the stream leads with a
`stream_gap` event instead: the engine logs the gap at `info!` and simply
continues — poll triggers re-observe current state on their next cycle, and
the re-entrancy contract already assumes events can be missed across an
outage. Events that arrive while no trigger matches
are discarded — the engine keeps no event history. The stream URL is derived
from the invocation's `mcp_server_url` origin unless overridden in
configuration.

Webhook delivery is not used: `session-runner` registers no
`subscribes_to`/`barrier_gates` and never blocks `rp`'s tool pipeline. It is
purely a *consumer* of the stream plus an MCP *client*.

## Invocation

`rp` POSTs `/invoke` per the orchestrator protocol:

```jsonc
{
  "workflow_id": "wf-550e8400-e29b-41d4",
  "session_id": "session-2026-07-01",
  "mcp_server_url": "http://localhost:11115/mcp",
  "recovery": null,
  "config": {
    "workflow": "deep_sky",
    "parameters": { "camera_id": "main-cam", "focuser_id": "main-foc" }
  }
}
```

- `config` is this plugin's registered `config` object, forwarded verbatim
  by `rp` (`rp.md` § Orchestrator Invocation Protocol).
- `config.workflow` names a document: `<name>.json` resolved in the
  configured `workflows_dir` (the `.json` suffix may be spelled out; it is
  appended when absent), or an absolute path. Resolution outside
  `workflows_dir` for relative names is rejected.
- `config.parameters` is validated against the document's `parameters`
  declarations (unknown parameter names are errors; missing required
  parameters are errors; types must match).
- The acknowledgment returns the document's `estimated_duration` /
  `max_duration` (engine defaults `"1h"` / `"14h"` when the document omits
  them — `max_duration` must comfortably exceed a full night because `rp`
  treats its expiry as plugin timeout).
- Any validation failure (unknown document, schema violation, unknown tool,
  bad parameters) is returned as the `/invoke` error response — the session
  fails to start loudly, before any hardware moves.
- Completion is posted to `POST /api/plugins/{workflow_id}/complete` with
  `status` (`"complete"`, or `"error"` for a failed workflow) and a result
  payload: `{ "workflow": "<name>", "outcome":
  "complete" | "failed", "error": "<message when failed>" }` plus any
  values the document placed under `session.report.*` (the conventional
  place for a document to accumulate its summary — e.g. frames per filter;
  the fixed `workflow`/`outcome`/`error` keys win on a name collision).
  Both outcomes end the session: once `rp` acknowledges the completion
  (2xx), the blackboard file is deleted. A safety termination posts
  nothing and keeps the blackboard (see [Safety
  Behavior](#safety-behavior)); an unacknowledged completion also keeps
  it, and is logged loudly (the post carries a 30 s timeout — a stalled
  `rp` counts as unacknowledged rather than wedging the session task).

## Validation

Three layers, all sharing one implementation:

1. **Schema validation** — the document against the rules published in
   `schema/workflow-v1.schema.json`: structure, discriminant keys, unknown
   keys, reserved names (`_`-prefixed parameters, `session._*` writes),
   unique trigger `id`s / `once` keys, loop-bound requirements,
   expression fields parse-checked (including the namespace-scope rule
   above), `$expr` placement, non-overlapping `set` keys, and duration
   fields checked against the published surface form **and** humantime
   (humantime alone is looser — it accepts `1day` / `1 h`, which the
   published pattern rejects; the document format is their intersection).
   Implementation note: layer 1 is a hand-rolled validation walk
   (`src/document/validate.rs`) that doubles as the typed-model builder
   (parse-don't-validate) and reports **all** findings in one pass with
   exact JSON-Pointer locations and targeted messages (raw JSON-Schema
   `oneOf` output cannot name a misspelled key or produce the `script`
   reservation error). The published schema remains the external
   contract: an agreement suite enforces that everything the walk accepts
   passes the schema — the walk is only ever *stronger*, where JSON
   Schema cannot express a rule.
2. **Catalog validation** (requires `rp`) — every `tool` node's name exists
   in `tools/list`; literal args type-check against the tool's parameter
   schema; required tool parameters are present (as literal or `$expr`);
   `$expr` argument types are checked at runtime when the call is made.
   Poll-trigger tools validate the same way. When a tool's schema pins
   `additionalProperties: false`, every argument **name** (literal or
   `$expr`) must be a declared property — a misspelled argument must not
   silently travel to the tool. Implementation
   (`src/document/catalog.rs`): literal values are checked with a real
   JSON-Schema validator against the tool's input schema (top-level
   `required` / `additionalProperties` stripped — those two are enforced
   separately so they see `$expr` arguments too); nested constraints
   inside a literal value (types, nested `required`, ranges) apply in
   full, and issue pointers extend into the literal
   (`…/args/target/ra_hours`).
3. **Parameter validation** — invocation `parameters` against the
   document's declarations.

`POST /validate` with `{ "document": { … } }` (or `{ "workflow": "<name>" }`
— exactly one) runs layers 1–2 and returns `200` with a report — the hook
for CI on shared workflow repositories, the future UI, and LLM authoring
loops:

```jsonc
{
  "valid": false,
  "errors": [ { "pointer": "/root/args/gain", "message": "…" } ],
  "catalog_validation": "checked"   // or "skipped: <reason>"
}
```

Each error is
`{ "pointer": "<RFC 6901 JSON Pointer>", "message": "…" }`, plus
`"expr_span": { "start": …, "end": … }` (byte offsets into the expression
string at that location) when the finding is inside an expression. Standalone `/validate` reaches `rp` through the
configured `mcp_server_url`; when that is unset or `rp` is unreachable, it
runs layer 1 only and says so in `catalog_validation` (`"skipped: no
mcp_server_url configured"` / `"skipped: rp unreachable (…)"`; schema
failures and workflows that cannot be loaded also skip the catalog check,
each under its own label). `4xx` is reserved for a malformed
*request* (neither/both input forms, invalid JSON).
`/invoke`, which always carries a live `mcp_server_url`, runs all three
layers before executing: validation failures return `400` with
`{ "error": "…", "issues": [ … ] }`, an unreachable `rp` returns `502`,
and only a fully validated invocation is acknowledged. A `session_id`
that could traverse outside `state_dir` (path separators, `..`) is
rejected — it names the blackboard file.

## Configuration

`session-runner`'s own config file (via `rusty-photon-config` conventions):

```jsonc
{
  "port": 11171,
  "workflows_dir": "/var/lib/rusty-photon/workflows",
  "state_dir": "/var/lib/rusty-photon/session-runner",
  "mcp_server_url": null,       // rp MCP endpoint for standalone /validate only
  "events_url": null            // override; default derives from mcp_server_url origin
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `port` | int | 11171 | HTTP listen port for `/invoke`, `/validate`, `/health` |
| `workflows_dir` | path | required | Directory of workflow documents; first-party documents ship in the package |
| `state_dir` | path | required | Blackboard persistence directory |
| `mcp_server_url` | string or null | null | `rp` MCP endpoint used only by standalone `/validate` catalog validation; invocations always use the URL delivered in the `/invoke` payload |
| `events_url` | string or null | null | Explicit SSE endpoint; null derives `<mcp origin>/api/events/subscribe` |

Unknown configuration keys are rejected at load — a misspelled field must
not silently fall back to a default. CLI: `--config <path>` (default: the
per-user platform config directory, e.g.
`~/.config/rusty-photon/session-runner.json`), `--port` (overrides the
file's `port`; `--port 0` binds an ephemeral port, printed at startup),
`--bind-address` (default `127.0.0.1`), `--log-level`.

## Example Documents

Shipped first-party documents live in `services/session-runner/workflows/`.

### `calibrator_flats.json` (the generalization proof)

The port of the existing Rust orchestrator's algorithm
([`calibrator-flats.md`](calibrator-flats.md)). The shipped file is
canonical; its BDD scenarios — the Rust orchestrator's suite re-run
against this document through the same OmniSim + `rp` + `session-runner`
topology — are the behavioral oracle, and the engine's unit suite
executes the same file against `rp`-faithful mock results to pin the
exact call sequence to the Rust loop's (per-filter exposure reset, no
rescale once converged, cleanup on failure).

The filter plan is an `array` parameter (`[ { "name": "L", "count": 20 },
… ]`) iterated with the total-traversal idiom: a blackboard index and a
`while` gate of `has(params.filters[session.filter_index])` — one past
the end reads `null`, so `has()` turns false and the loop completes.
Abridged to the load-bearing shape:

```jsonc
{
  "version": 1,
  "name": "calibrator-flats",
  "parameters": {
    "camera_id": { "type": "string", "required": true },
    "filter_wheel_id": { "type": "string", "required": true },
    "calibrator_id": { "type": "string", "required": true },
    "filters": { "type": "array", "required": true },   // [ { "name", "count" }, … ]
    "target_adu_fraction": { "type": "number", "default": 0.5 },
    "tolerance": { "type": "number", "default": 0.05 },
    "max_iterations": { "type": "integer", "default": 10 },
    "initial_duration": { "type": "duration", "default": "1s" }
  },
  "triggers": [],
  "root": { "sequence": [
    { "tool": "get_camera_info", "args": { "camera_id": { "$expr": "params.camera_id" } } },
    { "set": { "session.target_adu": "result.max_adu * params.target_adu_fraction",
               // exposure limits arrive as humantime strings — convert once,
               // do arithmetic on numbers, humantime() back at the tool call
               "session.exp_min": "seconds(result.exposure_min)",
               "session.exp_max": "seconds(result.exposure_max)",
               // has() guard: resume continues at the current filter
               "session.filter_index": "has(session.filter_index) ? session.filter_index : 0" } },
    // fail fast on a nonsensical target — before the try, so the cover
    // never closes (the Rust oracle catches this mid-search; the document
    // raises before any hardware moves)
    { "if": "session.target_adu <= 0",
      "then": [ { "fail": { "message": "'target_adu is not positive (max_adu * target_adu_fraction) — check get_camera_info and target_adu_fraction'" } } ] },
    { "try": [
        { "tool": "close_cover", "args": { "calibrator_id": { "$expr": "params.calibrator_id" } } },
        { "tool": "calibrator_on", "args": { "calibrator_id": { "$expr": "params.calibrator_id" } } },
        { "id": "filter-plan",
          "repeat": { "while": "has(params.filters[session.filter_index])", "max_iterations": 64 },
          "body": [
            { "tool": "set_filter", "args": { "filter_wheel_id": { "$expr": "params.filter_wheel_id" },
                                              "filter_name": { "$expr": "params.filters[session.filter_index].name" } } },
            { "set": { "session.duration": "seconds(params.initial_duration)" } },  // reset per filter
            { "id": "find-exposure",
              "repeat": { "until": "abs(session.median_adu - session.target_adu) / session.target_adu <= params.tolerance",
                          "max_iterations": { "$expr": "params.max_iterations" } },
              "body": [
                { "tool": "capture", "args": { "camera_id": { "$expr": "params.camera_id" },
                                               "duration": { "$expr": "humantime(session.duration)" } } },
                { "tool": "compute_image_stats", "args": { "document_id": { "$expr": "result.document_id" } } },
                { "set": { "session.median_adu": "result.median_adu" } },
                // rescale only when another pass is coming, so the duration
                // that converged is the one the flats reuse (the Rust loop's
                // exact behavior)
                { "if": "abs(session.median_adu - session.target_adu) / session.target_adu > params.tolerance",
                  "then": [ { "set": { "session.duration": "clamp(session.median_adu == 0 ? session.duration * 2 : session.duration * (session.target_adu / session.median_adu), session.exp_min, session.exp_max)" } } ] } ] },
            { "if": "result.converged == false",
              "then": [ { "log": { "level": "info", "message": "exposure did not converge, using best duration",
                                   "values": { "filter": "params.filters[session.filter_index].name" } } } ] },
            { "repeat": { "count": { "$expr": "params.filters[session.filter_index].count" } },
              "body": [
                { "tool": "capture", "args": { "camera_id": { "$expr": "params.camera_id" },
                                               "duration": { "$expr": "humantime(session.duration)" } } } ] },
            { "set": { "session.report.total_frames": "session.report.total_frames + params.filters[session.filter_index].count",
                       "session.filter_index": "session.filter_index + 1" } } ] },
        // a while loop that exhausts its budget completes with
        // result.converged == false — for this document that means an
        // absurd plan, and silently skipping filters is worse than failing
        { "if": "result.converged == false",
          "then": [ { "fail": { "message": "'the filter plan exceeds the 64-filter loop budget'" } } ] }
      ],
      "finally": [
        { "tool": "calibrator_off", "args": { "calibrator_id": { "$expr": "params.calibrator_id" } } },
        { "tool": "open_cover", "args": { "calibrator_id": { "$expr": "params.calibrator_id" } } } ] }
  ] }
}
```

### `deep_sky.json` (skeleton)

```jsonc
{
  "version": 1,
  "name": "deep-sky",
  "parameters": { "camera_id": { "type": "string", "required": true },
                  "focuser_id": { "type": "string", "required": true },
                  "guide": { "type": "boolean", "default": true },
                  "dither_every": { "type": "integer", "default": 3 } },
  "triggers": [
    { "id": "refocus-on-hfr",
      "on": { "event": "exposure_complete" },
      "when": "has(session.last_focus_hfr)",
      "while": "session.imaging == true",
      "cooldown": "15m",
      "do": [ { "tool": "measure_basic", "args": { "document_id": { "$expr": "event.document_id" } } },
              { "if": "result.hfr != null && result.hfr > session.last_focus_hfr * 1.2",
                "then": [ { "tool": "auto_focus", "args": { /* … */ } },
                          { "set": { "session.last_focus_hfr": "result.best_hfr" } } ] } ] },
    { "id": "flip-when-due",
      "on": { "poll": { "tool": "get_meridian_status", "interval": "60s" } },
      "when": "event.time_to_flip_seconds < 300",
      "while": "session.imaging == true",
      "do": [ /* stop guiding, slew (flip), re-center, re-focus, resume guiding */ ] },
    { "id": "handle-correction",
      "on": { "event": "correction_requested" },
      "do": [ /* branch on event.action: focus → auto_focus, center → center_on_target */ ] }
  ],
  "root": { "sequence": [
    /* startup: unpark, set_tracking, cool camera — idempotent, safe to re-run */
    { "repeat": { "while": "session.session_over != true", "max_iterations": 1000 },
      "body": [
        { "tool": "get_next_target" },
        { "if": "result.reason == 'end_of_session'",
          "then": [ { "set": { "session.session_over": "true" } } ],
          "else": [ { "if": "result.target == null",
              "then": [ { "wait": { "duration": "5m" } } ],  // wait_for_twilight, all_below_min_altitude, …
              "else": [
                { "set": { "session.target_name": "result.target.name",
                           "session.target_ra": "result.target.ra_hours",
                           "session.target_dec": "result.target.dec_degrees" } },
                /* slew → center_on_target → auto_focus → start_guiding,
                   set session.imaging = true, capture + record_exposure loop
                   re-asking get_next_target after each frame; dither every
                   params.dither_every frames; on target change: stop guiding,
                   session.imaging = false, continue outer loop */ ] } ] } ] },
    /* shutdown: stop_guiding, park */ ] }
}
```

The dispatch loop (`get_next_target` after every frame + `record_exposure`
progress in `rp`) is what makes this document re-entrant with **zero**
`once` markers: after a crash, the same loop simply continues from the
persisted progress.

## Error Handling Summary

| Failure | Behavior |
|---------|----------|
| Document fails schema/catalog/parameter validation | `/invoke` returns the error; nothing executes. |
| Tool call errors (after `retry`) | Workflow error → nearest `catch`, `finally` blocks run; uncaught → workflow fails, completion posted with `outcome: "failed"`. |
| Tool result carries a correction | Synthetic `correction_requested` trigger; not an error. |
| Expression error (null arithmetic, division by zero) | Workflow error at that instruction, same propagation as tool errors. |
| `wait` timeout | Workflow error. |
| Loop `max_iterations` exhausted (`until`/`while`) | Loop completes with `result.converged = false`; not an error. |
| SSE stream drops | Reconnect with `Last-Event-ID`; exact replay within `rp`'s 512-event retention; on `stream_gap`, log and continue (§ Event Subscription). |
| Poll-trigger tool call fails | `debug!` log, skip cycle. |
| MCP session terminated by `rp` (safety) | Best-effort `finally`, persist blackboard, exit without completion; await re-invocation. |
| Engine crash / power failure | Blackboard reflects every completed `set`; recovery invocation re-executes per the re-entrancy contract. |
| Blackboard write fails | Workflow error (fail loud — continuing with unpersistable state would silently break resume). |

## MVP Scope

**In scope (v1):** the instruction vocabulary above; expressions per the
semantics above; `event` / `poll` / `correction_requested` triggers with
`when`/`while`/`once`/`cooldown`; blackboard persistence + re-derive resume;
schema + catalog + parameter validation and `/validate`; SSE consumption
with replay; the two shipped documents (`calibrator_flats.json`,
`deep_sky.json`).

**Deferred:** Luau `script` nodes (schema key reserved); container-scoped
triggers (use `while` gates); parallel containers; sub-workflow
imports/templates; a `ui-htmx` document editor; typed array-element
declarations (v1 `array` parameters are opaque JSON arrays — the flats
port needs no more, and element-shape mistakes still fail loudly, as
run-time expression errors instead of load-time findings); retirement of
the Rust `calibrator-flats` service (separate decision after the port has
mileage).

## Module Structure

```
services/session-runner/
  schema/workflow-v1.schema.json   The published document schema
  workflows/                       First-party documents (installed with the service)
  src/
    main.rs            CLI entry point (rusty-photon-service-lifecycle)
    lib.rs             ServerBuilder (two-phase: build → start)
    config.rs          Service configuration
    error.rs           SessionRunnerError (thiserror)
    document/          Document model, schema-layer validation, parameter
                       binding, workflow-name resolution (catalog
                       validation joins once the MCP client exists)
    expr/              Expression parsing + evaluation (Phase B)
    blackboard.rs      session.* state + atomic persistence
    engine/            Tree execution, safe points, trigger queue, resume
    events.rs          SSE client (Last-Event-ID replay)
    mcp_client.rs      rmcp Streamable HTTP client to rp's /mcp
    routes.rs          Axum router: POST /invoke, POST /validate, GET /health
```

## Testing Strategy

Testing follows [`docs/skills/testing.md`](../skills/testing.md).

### Unit tests

- Document parsing and validation: every instruction type, every schema
  error (unknown key, missing loop bound, duplicate trigger id, reserved
  names, `script` reservation message).
- Expression evaluation: every operator/function, every namespace, null
  handling, division by zero — table-driven, exhaustive.
- Engine semantics against a mock MCP-client trait: sequencing, `result`
  scoping, `set` persistence ordering, `try`/`catch`/`finally` paths
  (including finally-does-not-mask), `retry`, loop bounds and
  `result.converged`, trigger safe-point interleaving, `once`/`cooldown`
  bookkeeping.
- Blackboard: atomic write, reload, reserved-key protection.

### BDD tests (Cucumber, rp-harness)

Full three-process topology (OmniSim + `rp` + `session-runner`) via
`bdd_infra::rp_harness`, mirroring `calibrator-flats`' suite:

| Design section | Feature file | Representative scenarios |
|----------------|--------------|--------------------------|
| Invocation + validation | `invocation.feature` | invalid document rejected at `/invoke`; unknown tool named in error; parameter type mismatch |
| Flats port equivalence | `flat_calibration.feature` | the scenarios from `calibrator-flats`' suite, run against the document — same events, frame counts, cleanup-on-failure |
| Triggers | `triggers.feature` | refocus fires between exposures, not during; cooldown respected; poll trigger fires flip action |
| Resume | `recovery.feature` | kill mid-capture-loop → re-invoke with recovery → progress continues without repeated frames; `once` marker not re-run |
| Safety | `safety.feature` | unsafe transition → engine exits without completion → safe transition → re-invocation resumes |

### Golden documents

The shipped `workflows/*.json` are validated in CI against both the
validation walk and the published schema (a unit test walks the
directory), so a format change that breaks a first-party document fails
the build. The validation corpus additionally embeds
`calibrator_flats.json` verbatim, and the engine's exec tests execute it
against `rp`-faithful mock results — the shipped artifact, not a copy, is
what the unit suites pin.

## Future Considerations

- **Luau script handlers** (`script` nodes): stateless per-event handlers
  with blackboard-only state, preserving the re-derive resume model; the
  coroutine-yield boundary is where deterministic replay would attach if
  ever needed.
- **Document editor in `ui-htmx`** driven by the JSON Schema, including an
  expression condition-builder.
- **Sub-workflow composition** (`{"call": "…"}`) once shipped documents
  show real duplication.
- **Sky-flat document** — the stress test for the expression layer's
  ceiling (per-frame exposure adaptation against a brightening/darkening
  sky); if it doesn't fit the bounded expressions, it becomes the motivating
  case for `script` nodes rather than for growing the expression language.
