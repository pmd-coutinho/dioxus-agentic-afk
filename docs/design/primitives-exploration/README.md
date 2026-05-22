# Dashboard primitives — design exploration

Three distinct visual directions for the Dashboard primitive library
introduced by issue #33 (phase (b) of PRD #25). Open
`index.html` for the comparison grid, then any of the per-direction
files to see all seven primitives × loading/empty/error states rendered
with real CSS and Google Fonts.

| File | Direction | Tone | Accent |
|------|-----------|------|--------|
| `a-terminal.html` | **Terminal Brutalist** | tmux / k9s / lazygit | `#FFB000` sodium amber |
| `b-editorial.html` | **Editorial & Considered** | Stripe Press / FT weekend | `#A0421C` rust on paper |
| `c-hud.html` | **HUD // Mission Control** | Cockpit panel | `#5BE9FF` cyan on void |

## How to view

Open `docs/design/primitives-exploration/index.html` directly in a
browser — the files are static and load fonts via Google Fonts CDN.

```sh
xdg-open docs/design/primitives-exploration/index.html
# or
python3 -m http.server -d docs/design/primitives-exploration 8080
```

## What each file contains

Every direction renders the same set so comparison is on tone, not
feature parity:

- Palette swatches with hex
- **Card** — two filled variants
- **StatusPill** — Idle, Pending, Running, Verified, Stale, Failed
- **ActionButton** — idle / pending / inline validation error / disabled, in default + primary + destructive variants
- **ToastRegion** — success, transient with retry, system error
- **EmptyState** — two variants
- **ErrorState** — full-panel 503 with RFC 7807 body
- **LoadingSkeleton** — inside a Card

## Next

After the human picks A, B, or C:

1. Codify the chosen palette/typography/density/motion in
   `docs/design/dashboard-primitives.md` (project-level design note).
2. Port the primitives into `apps/dashboard/src/ui/` as Dioxus
   components against Tailwind v4 utilities + a small `:root` palette
   override in `apps/dashboard/assets/tailwind.css`.
3. Add a `/design` sandbox route in the Dashboard demoing each
   primitive in isolation (required by issue #33 acceptance criteria).
4. Switch back to `/tdd` for the `ActionButton ↔ ProjectStore` binding
   logic (pure tests over `MutationKey` → pending/error state).
