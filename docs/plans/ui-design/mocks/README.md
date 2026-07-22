# Activity stream UI mocks

Static HTML mocks exploring the user-facing UI for rusty-photon. Each file is fully self-contained: vanilla CSS, system fonts, inline SVG, no JavaScript dependencies, no CDN. Open any file directly in a browser.

These are **reference artifacts only** — no production code yet. They exist to lock in the chosen UX paradigm, visual direction, and implementation approach before any code is written, so future implementation work can be evaluated against a concrete target rather than re-derived from scratch.

> **Implementation status.** The [config-actions plan](../../archive/config-actions.md)
> built the settings surface first (Phases 1–3); Phase 5 ports **this chosen
> direction** (`7-stream-fold.html`) into the **`ui-htmx`** BFF service
> ([`docs/services/ui-htmx.md`](../../../services/ui-htmx.md)) as the live
> `/stream` page — the narrative feed, the sticky fold strip/panel, and the
> night-vision toggle — fed by rp's real SSE event stream, alongside the
> `/equipment` roster page. The guider graph and trend-chart cards await a
> telemetry-history source and remain mock-only for now.

## Files

| File | Status | Direction | Familiar pattern |
|------|--------|-----------|------------------|
| `1-gallery.html` | rejected | Image-first, hero astrophoto with right metadata rail and bottom filmstrip | Lightroom / Apple Photos |
| `2-dashboard.html` | rejected | Multi-panel grid with sidebar nav, dedicated status card, image card, history accordion | Linear / Notion / GitHub Projects |
| `3-stream.html` | foundational | Single-column narrative feed: hero status + latest image + collapsible past sessions | Vercel deploys / Stripe activity |
| `4-stream-inline.html` | foundational | Stream + a big always-on live telemetry card under the hero | — |
| `5-stream-sticky.html` | foundational | Stream + a thin sticky telemetry strip pinned under the nav | — |
| `6-live.html` | superseded | Standalone full-screen telemetry view (replaced by the inline fold panel in #7) | Trading apps / Grafana |
| **`7-stream-fold.html`** | **chosen** | Stream + collapsible-by-default sticky telemetry strip that folds open into a full-width inline panel. CSS-only night-vision toggle. | Stripe activity + collapsible drawers |
| `8-plugin-polish.html` | rejected | Warm-dark "instrument-panel + editorial" polish pass on #7 (serif headlines, reticle corners, grain texture, synchronized heartbeat pulse, amber "armed" bar) | — |
| `9-plugin-fresh.html` | rejected | Brutalist mono-terminal generated from scratch (strict grayscale, single phosphor-green accent, all-caps mono everywhere) | — |
| `10-lcars.html` | **easter-egg theme** | Star Trek: The Next Generation LCARS interpretation. Bold colored elbows, condensed all-caps type, pill buttons, stardates, decorative reference codes. | Enterprise-D bridge displays |

## Chosen direction (`7-stream-fold.html`)

A narrative activity stream is the spine of the UI — taking image after image is essentially a story being told over time, and the stream paradigm matches that natively. Live telemetry is layered on top via a sticky strip that's:

- **Collapsed by default**: a 44-px tall pinned bar showing key live numbers (RA / Dec sparklines, HFR, CCD temp, sky brightness) and current operation.
- **Expandable in place**: clicking the strip folds open a full-width panel containing the full PHD2-style guider graph (with ±1.0″ / ±0.5″ reference bands and time axis), four trend-chart cards (HFR, CCD temp, sky brightness, dew margin), an equipment LED list, and the ordered "tonight's plan" stage list.
- **Animated**: the panel rolls down using the CSS Grid `0fr → 1fr` trick (cross-browser since 2023) — no JavaScript, no `interpolate-size` requirement.

A single fold-out panel obviates the need for a separate "punch out" full-screen view. `6-live.html` is kept for historical reference but is not reachable from the chosen UI.

## LCARS easter-egg theme (`10-lcars.html`)

A full-fidelity Star Trek: The Next Generation interpretation in the iconic LCARS visual language designed by Michael Okuda. Hits every LCARS grammatical element: L-shaped colored frame with rounded inside-corner elbows, pill buttons in the canonical Okudagram palette (amber, peach, violet, magenta, blue, yellow), all-caps condensed type, stardate and decorative reference codes (`M-04-2380`, `047-Δ`, `RP-NCC-04-2380-Δ-G`), multi-block colored section headers, and pill-shaped action buttons.

Preserved as a future *theme*, not the production direction — LCARS is iconic but visually loud and is **not** what users learn from modern web tools, which conflicts with the "borrow familiar idioms" principle. As an opt-in theme accessible via a hidden config flag (or a Konami-code Easter egg), it costs nothing to maintain since the underlying structure and content are identical to `7-stream-fold.html`.

**Bonus**: the night-vision toggle in `10-lcars.html` is thematically perfect — the LCARS amber → red shift is exactly how Enterprise displays transitioned from yellow to red alert.

## Other rejected explorations (`#8`, `#9`)

Both generated using the `frontend-design` plugin to test alternative aesthetic directions; both rejected after review:

- **`8-plugin-polish.html` (warm-dark instrument-panel polish)** — too condensed and visually blocky compared to the chosen direction. The serif headlines, corner reticles, film grain, synchronized heartbeat pulse, and amber "armed" bar added gravitas at the cost of the breathing room that makes `#7` feel calm. Useful as a reference for *what not to do*: don't optimize for instrument-panel feel at the expense of legibility and white space.
- **`9-plugin-fresh.html` (brutalist mono-terminal)** — beautiful in its own right but reads as a desktop developer tool (closer to k9s or a Bloomberg terminal than to a web app), which conflicts with the "feels like a familiar web tool" goal.

The plugin's value here was mostly in stretching the design space — both rejections sharpened understanding of what *not* to do, even though neither output landed as the chosen direction.

## Night-vision mode

Implemented in `7-stream-fold.html` (and inherited by `10-lcars.html`). A pure-CSS toggle using a hidden `<input type="checkbox">` + `body:has(:checked)` selector applies a single page-level filter:

```css
body:has(#night-vision:checked) {
  filter: grayscale(1) sepia(1) hue-rotate(-50deg) saturate(7) brightness(0.7);
}
```

Effects:
- Desaturates astrophotos to true B&W (no residual color).
- Re-tints the entire UI to red shades, preserving dark adaptation.
- Dims overall brightness for night use.

**Known limitation**: the filter collapses all hues to red, so green/amber/red severity LEDs become brightness differences instead of color differences. A real implementation should supplement with explicit overrides (or pulse animation) on critical alert states.

## Animation technique (CSS Grid `0fr → 1fr`)

The fold panel in `7-stream-fold.html` uses the CSS Grid `0fr → 1fr` trick for the roll-down animation, which works in every modern browser since late 2023 (Chrome / Firefox / Safari 117+) without requiring any JavaScript:

```css
.fold-body {
  display: grid;
  grid-template-rows: 0fr;
  transition: grid-template-rows 320ms cubic-bezier(.4, 0, .2, 1);
}
.fold-body > .panel {
  min-height: 0;
  overflow: hidden;
}
.fold-state:checked ~ .fold-body { grid-template-rows: 1fr }
```

A wrapper div is a CSS Grid container whose single row animates between `0fr` (collapsed, content has zero height) and `1fr` (expanded, content stretches to its natural height). The inner content needs `min-height: 0` and `overflow: hidden` so it clips smoothly during the transition.

A more modern alternative is `::details-content` + `interpolate-size: allow-keywords` (CSS 2024), but support landed later in Firefox and the grid trick is the reliable cross-browser baseline.

The same `<input type="checkbox">` + `<label for>` + sibling-selector pattern is used for the night-vision toggle, keeping every mock JavaScript-free.

## Key design decisions

- **Stream over dashboard.** Capturing is inherently a narrative; the dashboard layout was strong but the stream tells the story better.
- **Web-first, not desktop.** Targets a remote-access pattern (browser on a laptop in the warm room, server on the Pi) rather than NINA/SGP-style desktop apps that need a large monitor.
- **Borrow familiar idioms.** Patterns from web tools users already know (Stripe activity, Vercel deploys, GitHub Actions) so onboarding cost is low — the goal is "users have already learned the interactions elsewhere."
- **Telemetry is layered, not central.** The stream stays narrative; live telemetry is a transient panel that's mini by default, expandable on demand. Avoids the "constantly-watching cockpit" feel that would compete with reading session history.
- **Dark mode default + night-vision overlay.** Dark is non-negotiable for astronomy; night-vision is a one-tap layer on top, not a separate theme.
- **Foldable over punch-out.** A foldable in-place panel preserves narrative context. Punching out to a separate fullscreen view (`6-live.html`) was tried and rejected for breaking the stream's continuity.

## What's not decided yet

- **Idle state** — what does the page look like when no session is running? The strip and panel are session-only. The stream below should probably show a "Start a session" CTA in place of the hero.
- **When the strip auto-expands.** Closed-by-default for routine sessions; open-by-default if telemetry crosses a threshold (HFR climbing, guide RMS spiking) is a candidate.
- **Severity color encoding under night vision.** See limitation above.
- **Mobile / phone layout.** Mocks are sized for desktop / tablet. Phone (chair-side glance) is a separate exercise.

## Implementation plan

Target stack (chosen for minimal tooling, single Rust binary, no npm or Node dependencies):

| Concern | Choice | Rationale |
|---------|--------|-----------|
| Templates | [Maud](https://maud.lambda.xyz/) — compile-time HTML inside Rust with type-safety | The `7-stream-fold.html` markup ports near-directly into Maud's `html!` macro |
| HTTP server | axum | Already in use in `services/sentinel/src/dashboard.rs` |
| Interactivity | [HTMX](https://htmx.org/) — single ~14KB JS file dropped in via `<script>`, declarative `hx-*` attributes, server returns HTML fragments | No build step, no `node_modules`. Server stays in Rust |
| Live telemetry updates | Server-Sent Events (SSE) | axum has built-in support; server pushes new HTML fragments for strip values + chart sparklines on a regular interval, HTMX's SSE extension swaps them into target elements by ID |
| Static assets | Embed via `include_str!()` / `include_bytes!()` (or `rust-embed`) | One executable ships everything (CSS, HTMX bundle, fonts) — fits the Pi 5 deployment story |
| Foldable panel + night-vision | Stay pure CSS | Already working in mocks; no client-side JS needed |
| Collapsible history items | Native `<details>` | No JS needed |

### Rejected alternatives

- **Tailwind via npm** — adds a Node toolchain, ruled out by the "no npm" preference.
- **Tailwind standalone CLI** (Go binary, no Node) — viable but unnecessary; the chosen direction uses ~600 lines of vanilla CSS that's easier to maintain by hand at this scale.
- **Leptos / Dioxus (Rust → WASM)** — powerful but adds a WASM build step, a frontend framework to learn, and is overkill for what is fundamentally a server-rendered control panel with a few live-updating fields.
- **Hand-rolled HTML strings in Rust** (the current `services/sentinel/src/dashboard.rs` pattern) — fine for one small page, but `7-stream-fold.html` is large enough that Maud's structure and type-safety pay off.

### Suggested implementation order

When implementation begins:

1. Port `7-stream-fold.html` markup into Maud with placeholder data
2. Extract the inline `<style>` block into a single CSS file, embed via `include_str!()`
3. Wire up axum routes and existing event sources to push live fragments via SSE
4. Verify night-vision and fold-panel still work identically (they should — both are pure CSS)
5. Decide on idle-state behavior (the open question above) and implement
6. *(Optional)* implement the LCARS theme as a CSS variant, toggleable via a hidden config flag — nice-to-have, not blocking
