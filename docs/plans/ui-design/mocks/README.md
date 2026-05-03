# Activity stream UI mocks

Static HTML mocks exploring the user-facing UI for rusty-photon. Each file is fully self-contained: vanilla CSS, system fonts, inline SVG, no JavaScript dependencies, no CDN. Open any file directly in a browser.

These are **reference artifacts only** — no production code yet. They exist to lock in the chosen UX paradigm and visual direction before any implementation begins, so future implementation work can be evaluated against a concrete target rather than re-derived from scratch.

## Files

| File | Direction | Familiar pattern |
|------|-----------|------------------|
| `1-gallery.html` | Image-first, hero astrophoto with right metadata rail and bottom filmstrip | Lightroom / Apple Photos |
| `2-dashboard.html` | Multi-panel grid with sidebar nav, dedicated status card, image card, history accordion | Linear / Notion / GitHub Projects |
| `3-stream.html` | Single-column narrative feed: hero status + latest image + collapsible past sessions | Vercel deploys / Stripe activity |
| `4-stream-inline.html` | Stream + a big always-on live telemetry card under the hero | — |
| `5-stream-sticky.html` | Stream + a thin sticky telemetry strip pinned under the nav | — |
| `6-live.html` | Standalone full-screen telemetry view (superseded by #7) | Trading apps / Grafana |
| **`7-stream-fold.html`** | **Chosen direction.** Stream + collapsible-by-default sticky telemetry strip that folds open into a full-width inline panel. Includes a CSS-only night-vision toggle. | Stripe activity + collapsible drawers |

## Chosen direction (`7-stream-fold.html`)

A narrative activity stream is the spine of the UI — taking image after image is essentially a story being told over time, and the stream paradigm matches that natively. Live telemetry is layered on top via a sticky strip that's:

- **Collapsed by default**: a 44-px tall pinned bar showing key live numbers (RA / Dec sparklines, HFR, CCD temp, sky brightness) and current operation.
- **Expandable in place**: clicking the strip folds open a full-width panel containing the full PHD2-style guider graph (with ±1.0″ / ±0.5″ reference bands and time axis), four trend-chart cards (HFR, CCD temp, sky brightness, dew margin), an equipment LED list, and the ordered "tonight's plan" stage list.
- **Animated**: the panel rolls down using the CSS Grid `0fr → 1fr` trick (cross-browser since 2023) — no JavaScript, no `interpolate-size` requirement.

A single fold-out panel obviates the need for a separate "punch out" full-screen view. `6-live.html` is kept for historical reference but is not reachable from the chosen UI.

## Night-vision mode

Implemented in `7-stream-fold.html`. A pure-CSS toggle (`<input type="checkbox">` + `body:has(:checked)` selector) applies a single page-level `filter: grayscale(1) sepia(1) hue-rotate(-50deg) saturate(7) brightness(0.7)` that:

- Desaturates astrophotos to true B&W (no residual color).
- Re-tints the entire UI to red shades, preserving dark adaptation.
- Dims overall brightness for night use.

**Known limitation**: the filter collapses all hues to red, so green/amber/red severity LEDs become brightness differences instead of color differences. A real implementation should supplement with explicit overrides (or pulse animation) on critical alert states.

## Key design decisions

- **Stream over dashboard.** Capturing is inherently a narrative; the dashboard layout was strong but the stream tells the story better.
- **Web-first, not desktop.** Targets a remote-access pattern (browser on a laptop, server on the Pi) rather than NINA/SGP-style desktop apps that need a large monitor.
- **Borrow familiar idioms.** Patterns from web tools users already know (Stripe activity, Vercel deploys, GitHub Actions) so onboarding cost is low — the goal is "users have already learned the interactions elsewhere."
- **Telemetry is layered, not central.** The stream stays narrative; live telemetry is a transient panel that's mini by default, expandable on demand. Avoids the "constantly-watching cockpit" feel that would compete with reading session history.
- **Dark mode default + night-vision overlay.** Dark is non-negotiable for astronomy; night-vision is a one-tap layer on top, not a separate theme.

## What's not decided yet

- **Idle state** — what does the page look like when no session is running? The strip and panel are session-only. The stream below should probably show a "Start a session" CTA in place of the hero.
- **When the strip auto-expands.** Closed-by-default for routine sessions; open-by-default if telemetry crosses a threshold (e.g., HFR climbing, guide RMS spiking) is a candidate.
- **Severity color encoding under night vision.** See limitation above.
- **Mobile / phone layout.** Mocks are sized for desktop/tablet. Phone (chair-side glance) is a separate exercise.

## Implementation pointers

When the time comes to implement, the natural Rust stack is:

- Templates: Maud or Askama (server-rendered HTML)
- Interactivity: HTMX for partial updates instead of polling
- Styling: pull the inline CSS from `7-stream-fold.html` into a stylesheet; Tailwind + DaisyUI works if a utility approach is preferred
- Live data: server-sent events or WebSocket for the strip / panel updates
- Existing baseline: `services/sentinel/src/dashboard.rs` is the current hand-rolled axum dashboard and shows the rendering style already in use
