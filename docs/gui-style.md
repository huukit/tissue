# GUI style rules (#168)

The GUI is plain, dependency-free HTML/CSS/TS (`src/gui/`). Keep it that way.
These rules keep every view consistent; PRs touching `src/gui/` should
follow them (owner reviews the result before merge).

## Layout

- Shell: top bar + sidebar (nav, project picker, status filter) + content
  column (`max-width: 60rem`, centered). Cards are `var(--card)` on
  `var(--bg)` with a `var(--border)` 1px border and 6px radius.
- One toolbar per list view: search grows, the primary action sits at the
  right end (`.list-toolbar`).

## Controls

- **Buttons** share `.btn` (padding, radius, border). Variants:
  - `.btn-primary` — accent background, white text: the main action of a
    view/form (Create issue, Create release, Save settings, Save token,
    Create project).
  - `.btn` — secondary/default (Cancel-adjacent actions, toggles).
  - `.btn-ghost` — quiet (sidebar "+ New project", Clear).
  - `.btn-approve` / `.btn-reject` — proposal decisions only (green/red).
  - `.btn-danger` — destructive.
  - `:disabled` dims (opacity 0.5, no pointer).
- **Text controls** (`input[type=text|password|search|number]`, `select`,
  `textarea`) share one look: card background, border, 6px radius, same
  padding; `:focus` gets a 2px accent outline. Every interactive element
  (links, buttons, summaries, controls) shows a visible `:focus-visible`
  ring — keyboard navigation must be obvious.
- **Forms**: label above control (wrapped `<label>`), controls full width,
  validation errors inline in `.edit-result` (aria-live) right under the
  form, plus the human-readable message.

## States

- Empty: `.placeholder` — dashed-border card, muted text.
- Loading: `.loading` — centered muted text.
- Error: `.error` — banner (light red bg, red border, left-aligned); used
  for full-view failures and inline failures alike.
- Status/last-touch badges: pill with `currentColor` border; review badge
  uses the amber `--badge-review*` pair.

## Out of scope

No frameworks, no dark mode/theming, no mobile redesign, no behavior
changes — CSS + small markup only.
