# Dashboard Primitives — Direction C (HUD / Mission Control)

Authoritative design tokens and principles for the Dashboard primitive library
shipped under `apps/dashboard/src/ui/`. Picked from
`docs/design/primitives-exploration/` after the HITL review for issue #33.

The Control Plane is a console; each Project is a craft you monitor. Frosted
glass panels float over a gradient mesh. **Cyan** is the primary signal,
**coral** is danger, **mint** is verified, **amber** is in-flight.

## Palette

Tokens live in `apps/dashboard/tailwind.css` under `@theme`. Reference them via
Tailwind utilities (`bg-void`, `text-cyan`, `border-stroke`) — never hardcode
hex in components.

| Token            | Hex / Value                       | Use                                     |
| ---------------- | --------------------------------- | --------------------------------------- |
| `--color-void`   | `#06081A`                         | App background                          |
| `--color-void-2` | `#0B0E22`                         | Secondary surfaces                      |
| `--color-panel`  | `rgba(15, 21, 48, 0.62)`          | Frosted card surface (with backdrop blur) |
| `--color-stroke` | `rgba(91, 233, 255, 0.16)`        | Default 1px border                      |
| `--color-ink`    | `#DCE8F7`                         | Primary text                            |
| `--color-ink-2` | `#98A8C4`                         | Secondary text                          |
| `--color-ink-dim`| `#5B6A8C`                         | Tertiary / placeholder text             |
| `--color-cyan`   | `#5BE9FF`                         | Primary signal, links, focus            |
| `--color-cyan-2` | `#00BCD4`                         | Deeper cyan accent                      |
| `--color-mint`   | `#6EE7B7`                         | Success / verified                      |
| `--color-amber`  | `#FFC857`                         | Pending / in-flight                     |
| `--color-coral`  | `#FF6E6E`                         | Danger / error                          |
| `--color-magenta`| `#C792FF`                         | Secondary accent (empty-state variant)  |

## Typography

- **Display** — IBM Plex Sans Condensed, weights 500/600/700. ALL CAPS, tracked
  `letter-spacing: 0.18em`. Used for headings, status pills, button labels.
- **Body** — Inter Tight, weights 400/500. Default UI text, paragraphs.
- **Mono / telemetry** — JetBrains Mono, weights 400/500. IDs, timestamps,
  numeric readouts, problem-document blocks.

Fonts loaded from Google Fonts in `index.html` head; no self-hosting (yet).

## Density

Medium. **Notched corners** on every interactive surface via `clip-path`
(8px notch on buttons, 10px on toasts/cards). 1px strokes throughout, with a
cyan glow on focus / hover.

Card padding: header/footer 12–14px vertical, body 18px. Telemetry grid uses
`grid-template-columns: 1fr 1fr` with `gap: 14px 22px`.

## Motion

| Surface          | Trigger | Duration | Curve         | Effect                                   |
| ---------------- | ------- | -------- | ------------- | ---------------------------------------- |
| ActionButton     | hover   | 120ms    | ease-out      | accent-color flood + soft glow           |
| StatusPill `run` | always  | 1.2s     | ease-in-out   | dot pulse (scale 0.85↔1, opacity 0.4↔1)  |
| ActionButton `pending` | always | 1.2s | linear        | light-bar sweep left→right               |
| LoadingSkeleton  | always  | 1.8s     | linear        | cyan scanline sweep                      |
| Toast enter      | mount   | 200ms    | ease-out      | translate-y from +8px, opacity 0→1       |

No motion exceeds 1.8s. All loops respect `prefers-reduced-motion: reduce`
(degrade to static accent state — implementer responsibility).

## Iconography

Geometric glyphs over icon fonts:

- Section markers: 14×14 cyan square rotated 45°, half-filled gradient.
- Empty state glyph: 64×64 diamond outline with inset 12px inner square.
- Toast / error category: 2px glowing colored bar pinned to the left edge.

Avoid: gradient cliché (purple/blue saas gradient), generic Lucide icons in
primary surfaces. JetBrains Mono diamond/bracket motifs preferred.

## Primitives

Each primitive ships **explicit** loading, empty, and error states (not
generic placeholders). API summary; see `apps/dashboard/src/ui/` for source.

- **`Card`** — frosted glass panel with bracket corner glyphs. Slots:
  `header`, `body`, `footer`.
- **`StatusPill`** — variants: `Idle`, `Pending`, `Running`, `Verified`,
  `Stale`, `Failed`. Glow dot + tracked caps.
- **`ActionButton`** — bound to `ProjectStore` via `MutationKey`. Variants:
  `Default`, `Primary`, `Destructive`. State derived from store: idle vs
  pending vs error-adjacent. Inline error renders RFC 7807 `title` + `detail`
  in a coral-bordered panel directly below the button.
- **`ToastRegion`** — replaces the prototype region in `main.rs:78`. Notched
  glass tile, glowing colored left-bar, close affordance, optional retry slot.
- **`EmptyState`** — diamond glyph + tracked-caps title + conversational body
  + optional CTA. Variant accent color (`cyan` default, `magenta` alt).
- **`ErrorState`** — coral-framed panel with inlaid `ERROR` cartouche. Renders
  RFC 7807 problem document as inline JetBrains Mono telemetry.
- **`LoadingSkeleton`** — cyan-tinted rectangles with scanline sweep. Composed
  of `SkeletonHeading` (22px tall, 60% width) + `SkeletonLine` (12px tall,
  variable width).

## Constraints

- ADR 0016 — Tailwind v4 with local Dioxus components. Add tokens via
  `@theme` in `apps/dashboard/tailwind.css`. No CSS-in-JS.
- ADR 0033 — server reads built Dashboard assets from
  `apps/dashboard/dist/`. `dx build` regenerates `assets/tailwind.css` —
  never edit that file by hand.
- `backdrop-filter` requires a browser target that supports it (Chromium 76+,
  Safari 9+, Firefox 103+). Acceptable for the local Dashboard.

## Sandbox

Route `/design` mounts a sandbox demoing every primitive × every state.
Required by issue #33 acceptance criteria.
