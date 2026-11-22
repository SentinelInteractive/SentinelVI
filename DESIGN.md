# Design

SentinelVI is an archive inspector. The screen is a ledger of measurements: names, offsets, sizes and counts, laid out so a person can compare two numbers without hunting. Decoration is not part of the product.

## Rules

**Space before decoration.** The spacing scale is 8 px: 8, 16, 24, 32, 48, 64. Sections are separated by 32 or 48 px, rows breathe at 8 px, and a screen ends with 64 px of empty space. When something feels crowded the fix is more space, not a border.

**Hierarchy comes from type and rules, not from boxes.** There are no cards, no shadows, no gradients and no rounded panels. A hairline `1px #262626` rule separates a table head from its rows and the sidebar block from the navigation. Nothing else gets a border unless it is a control.

**One accent, one primary action.** The accent `#EDEDED` appears on the active navigation item (a 2 px inset rule), the focus ring, and the single primary button of a screen. There is exactly one primary button per screen; everything else is a plain outlined button or a text link.

**Three columns at most.** Sidebar, content, inspector. The inspector only exists while something is selected, and it is a borderless column separated by a hairline on its left. Below 1100 px it moves under the content; below 860 px it is hidden until a row is selected.

**Tables only for dense data.** Library rows, texture entries, audio banks, animation clips and jobs are tables or row lists because the reader compares fields across rows. Anything with fewer than three items is a list of sentences or a definition list.

**Empty states are sentences.** An empty screen says what is missing, why, and the exact command that fills it. No illustrations, no slogans, no exclamation marks. A missing index is a normal state, not an error.

**Keyboard first.** `/` focuses the search field, arrow keys move through rows, Enter opens the selection, Escape clears the search or the selection. Every focusable element has a visible 1 px outline at 2 px offset. Skip link to content is the first focusable element.

**375 px must not overflow.** The sidebar becomes a wrapped row of text links, the counts block is dropped, tables hide their non-deciding columns, and definition lists stack key over value.

## Type

- **IBM Plex Sans** for interface text: navigation, headings, prose, buttons, labels.
- **JetBrains Mono** for data: names, offsets, hashes, sizes, counts, coordinates, and the search field itself, because search input is a name fragment.
- Screen titles: 20 px, weight 500, sentence case, no tracking. Mobile 18 px.
- Body and rows: 13 px. Table cells: 12 px. Secondary metadata and confidence: 11 px, colour `#9A9A9A`.
- No uppercase headings, no wide tracking, no monospace display type. Monospace marks a value, never a headline.

## Colour

| Token | Value | Used for |
| --- | --- | --- |
| background | `#161616` | page |
| surface | `#1E1E1E` | hover, selected row, navigation background |
| surface 2 | `#2A2A2A` | pressed button |
| field | `#1A1A1A` | inputs, hex preview, map frame fallback |
| border | `#262626` | hairline rules |
| border strong | `#6A6A6A` | control outlines, active search underline |
| text | `#FAFAFA` | primary text |
| muted | `#B4B4B4` | prose, secondary values (7.1:1 on background) |
| subtle | `#9A9A9A` | labels, confidence words (5.4:1 on background) |
| accent | `#EDEDED` | active nav marker, focus ring, primary button fill |
| success | `#7EE2A8` | job finished |
| warning | `#D9A13B` | reserved for a degraded index |
| danger | `#E5484D` | job failed, error message |

Every text pairing above is above 4.5:1 against `#161616`; `#9A9A9A` is the lightest text used and measures 5.4:1.

## Layout

- Rail 216 px, sticky, full height, `1px #262626` on its right edge.
- Content max width 1180 px, padding 32 px top, 48 px sides, 64 px bottom. At 860 px and below the padding is 24 px / 16 px.
- Library: archive tree 268 px, result list fluid, inspector 300 px when a row is selected. With the inspector open the tree narrows to 236 px.
- Map: frame height `min(72vh, 760px)`, minimum 360 px, canvas fills it, controls sit in the lower right corner of the frame.
- Textures: file list 268 px, entries table and preview in the remaining column.
- Rows are 44 px minimum, so a pointer and a thumb can both hit them.

## Motion

None beyond hover and focus colour changes. There is no entry animation, no fade, no pulse. The map redraws when the viewport changes because that is its job.

## What this design refuses

Gradient hero, stat cards, bento grid, glass, glow, capsule badges, icon sets, left colour stripes, fake terminal windows, emoji, counters that animate, skeletons, and any number that was not measured.
