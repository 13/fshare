# Mobile listing redesign — design

**Date:** 2026-07-13
**Status:** approved

## Problem

On phones in portrait (≤480px) the directory listing hid the Modified column
entirely. Users need name, full date + HH:MM, and size visible on small
screens, in a modern layout.

## Decision

Two-line rows plus a sort-chips bar on mobile. Desktop table unchanged.

## Design

### Markup (`src/listing.html`)

- New mobile-only sort bar between `{{upload}}` and the table:
  `<nav class="sortbar">Sort: <button data-k="0">Name</button>
  <button data-k="1">Size</button> <button data-k="2">Date</button></nav>`
- Table DOM unchanged: same `td.n / td.s / td.d` cells and `data-s` sort
  attributes. `src/listing.rs` row rendering unchanged (keeps the
  `<span class="tm">HH:MM</span>` wrapper; it is no longer hidden anywhere).

### CSS (≤480px)

- `thead` hidden; `.sortbar` shown as pill buttons. Active pill gets accent
  color and an ↑/↓ arrow reflecting sort direction.
- Each `tr` becomes a flex-wrap row bordered at the bottom:
  - Line 1: name, full width, single line with ellipsis for long names.
  - Line 2: `YYYY-MM-DD HH:MM · size`, muted, `.8em`. The `·` separator is a
    CSS `::before` on a non-empty size cell, so directories without a size
    show no dangling dot.
- Empty cells (`../` row) collapse via `td:empty { display:none }`.
- Row vertical padding ≈ .55em for a comfortable touch target.
- Dark mode inherited from existing CSS variables.

### JS

- Single shared sort routine; both `th` and `.sortbar button` clicks use it
  (keyed by `data-k`, toggling asc/desc via `data-dir`).
- Chip click updates active highlight and arrow. Header clicks don't touch
  chip state (the two controls are never visible at the same time).

### Tests (`tests/http.rs`)

- `listing_dates_visible_on_mobile` asserts: no full-column hide rule, the
  `.tm` time span present, and the sortbar present in rendered HTML.

## Out of scope

- Card/chip visual theme, thumbnails, virtual scrolling.
- Any server-side or JSON API change.
