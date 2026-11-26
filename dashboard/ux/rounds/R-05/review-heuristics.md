# Heuristic review, round R-05

6 pass, 5 partial, 0 fail (5 not observed: 1, 6, 8, 13, 16, a read-only monitor with no task in the
scenarios).

Inputs: `report.md`, `notes.md`, the four contact sheets, six extra shots (`shots/review-*.png`), and
full-page replays of busy, errors, idle and token-flow through the round's own harness (pinned fonts,
hermetic, both themes, 1366x768, 1280x720, 390x844), read as text and as pictures. R-04's and R-02's
sheets were read for what the move changed. No recordings. Nothing live was contacted.

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | not observed | no task in any scenario |
| 2 | State in words | pass | stale heartbeat says "7m 0s ago · stale (over 5 min)" (`review-busy-provider-1366-light.png`, `review-errors-dark-1366.png`); In flight says "waiting for retry, 23 s left"; the flow caption says what lines and dots mean (`review-busy-top-1366-light.png`) |
| 3 | Nothing required hidden | partial | desktop pass: every column of Routes and Calls in view at 1366 and 1280 (`review-busy-provider-1366-light.png`, `review-errors-attempts-1280-light.png`); at 390 In flight shows no State and Routes no more than Errors, with no cue (`review-narrow-390-light.png`, UXF-017) |
| 4 | Stable layout | partial | UXF-005 holds: `report.md` token-flow 0.039 (1366x768) and 0.035 (1280x720) per state, errors 0.006; idle and busy 0 |
| 5 | Provenance of every value | partial | UXF-010 holds (limit blocks named by `3fa9c1d2`, `b07e44a9`); the session id runs into its label (UXF-016, `review-errors-dark-1366.png` Sessions); "Name", "Other pods", "Read 21s ago" name no source (UXF-018, `review-busy-provider-1366-light.png`) |
| 6 | Errors cheap | not observed | |
| 7 | Words the user can say | partial | UXF-010 holds: "▸ 2" / "▾ 2" in the Attempts column (`review-errors-attempts-1280-light.png`), raw header names `anthropic-ratelimit-input-tokens-remaining` as labels (`review-busy-top-1366-light.png`); new: "tok/slast 60 s", "claude-code3f9a1c0e" (UXF-016) and "Other pods" (UXF-018); UXF-006 stays closed ("allowed (warning)" in one line, 1366 and 1280, both themes) |
| 8 | Counterpart's time respected | not observed | |
| 9 | Keyboard and pointer parity | pass | `report.md`: 0 unreachable, 0 focus invisible, 8 of 8 replays per theme; the expander is a button; the flow canvas is not a target |
| 10 | Recognition over recall | pass | every table is a named region with titled columns; counts on headings ("In flight 3", "Recent calls 40"); the attempts panel names its columns (`review-errors-attempts-1280-light.png`); help is on dotted-underlined labels |
| 11 | Calm by default | pass | idle draws nothing in the flow (`review-idle-1280-dark.png`); busy lines stay in the panel, dots only for calls (`review-busy-top-1366-light.png`); `report.md` reduced motion 0 violations |
| 12 | One state vocabulary | partial | UXF-011 holds: "streaming", "waiting-upstream", "waiting for retry, 23 s left", "RUNNING", "STARTING", "five_hour", "out_of_credits" in one view (`review-busy-provider-1366-light.png`, `review-busy-top-1366-light.png`); "allowed (warning)" is now a sentence |
| 13 | A wrong cue recovered in one action | not observed | |
| 14 | Unmistakable end state | pass | idle says "No active sessions.", "Idle.", "No routes yet." (`review-idle-1280-dark.png`); an empty provider shows no frame and says nothing, not observed with a provider configured |
| 15 | Readable in every theme; key data at 14 px or more | pass | `report.md` key font 14 px minimum, 0 deficit, axe 0 in both themes; tones, badges and the stale red legible in dark (`review-errors-dark-1366.png`) and light |
| 16 | Focus follows the work | not observed | |

## What the move to the renderer changed for the reader

- Headings: every section is a framed card with a 14 px bold heading and, for tables, a count chip
  ("In flight 3", "Recent calls 40"). Consistent and calm; the heading is the same size as table
  cells, so the page has one level of emphasis below the header. Not a defect. Better than R-02,
  where Provider was named in its heading as "Provider: runpod".
- Labels: small grey labels over values, dotted underline where help exists. The Provider frame lost
  the provider's name from its heading and shows a figure labelled "Name" (UXF-018).
- Cards: Subscription limits cards look as before (same tones, same heading with the hash, UXF-010).
  The second card's raw header dump and "Status –" are unchanged.
- Whitespace: the renderer drops the space before a second line, so words run together in tiles,
  sessions and the In flight state (UXF-016). R-04's sheet had the spaces.
- Calls detail: the expanded attempts panel is clear (attempt, status, duration, wait, Retry-After,
  error body, `review-errors-attempts-1280-light.png`); its header "Retry-After" and the row's flag
  "retry-after 30" differ in case only. The expander still reads "▸ 2".
- Empty states: idle names each empty region in words; the flow panel keeps 250 px for one line.
- Themes: same drawing, both legible. At 1280x720 the Provider balance tile wraps alone to a second
  row of Overview (seen in a scratch shot, not kept; it fits one row at 1366); not filed.
- Narrow: UXF-017.

## Open findings re-judged by sight

| id | still holds | where seen |
|---|---|---|
| UXF-005 | yes, 0.039 / 0.035 per state in token-flow, 0.006 in errors; no motion judged by eye | `report.md` metrics table |
| UXF-010 | yes: hash in both limits headings, raw header names as labels, "▸ 2" and "▾ 2" | `review-busy-top-1366-light.png`, `review-errors-attempts-1280-light.png` |
| UXF-011 | yes | `review-busy-provider-1366-light.png`, `review-busy-top-1366-light.png` |

## Findings filed

| id | title | severity | class | sessions | cost |
|---|---|---|---|---|---|
| UXF-016 | The renderer drops the space before a second line, so words run together ("tok/slast 60 s", "claude-code3f9a1c0e") | S3 | copy | all four | S |
| UXF-017 | At 390 px the tables show their first columns only, with no cue that the rest is to the right | S3 | affordance | all four, 390 only | M |
| UXF-018 | The Provider frame labels its figures "Name" and "Other pods", which no one can say without the frame | S3 | copy | busy, token-flow, errors | S |
