# Heuristic review, round R-07

7 pass, 7 partial, 0 fail (2 not observed: 6, 8; a read-only monitor with no task that fixes anything
and no counterpart).

Inputs: `report.md`, `round.json`, the six sheets in `shots/` (contact and full-page), six extra shots
(`shots/review-*.png`), the findings UXF-001 to UXF-027, and replays through the round's own harness
(pinned fonts, hermetic, no recording) on a copy of the built page whose digest equals the report's
(`a07c861bbc3a`): errors, busy, offline, token-flow and idle at 1366x768, 1280x720 and 390x844, both
themes, read as pictures and as text. Problems links were clicked at 390 and the landing measured.
No gateway was contacted. The contact sheets show the top of the last state only; the lost state is
in `full-1280x720-light.png` and in a scratch shot of offline at 1366 (not kept).

## 1. Heuristics

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | pass | the strip is the next thing, one line per kind of problem, worst first, each a link; `review-strip-errors-1366-dark.png`; `report.md` gate 4, 8 of 8 tasks, 0 actions, glance 1 |
| 2 | State in words | pass | "error"/"warning" are words in each line; "Failed calls in the last 5 min: claude-sonnet 4 of 4 failed, 429 rate limited; qwen-coder 1 of 1 failed, 504 timed out" names route, count, window and cause (`review-strip-errors-1366-dark.png`); the watchdog reads "11m 0s ago · stale (over 5 min)" in red (`review-provider-390-dark.png`); lost connection is "offline" plus a sentence |
| 3 | Nothing required hidden | partial | 390: UXF-027 holds (every strip link lands its heading 56 to 77 px, the sticky header ends at 75 px; `review-link-failed-390-light.png` opens on "scroll for more" and the table's column row, "Routes" is under the header); new UXF-028 (one pod row 186 px, `review-provider-390-dark.png`); the sideways tables now carry a cue (see UXF-017 below), so what was hidden is signposted; desktop first screen holds the strip, flow, Overview (`contact-1366x768-light.png`) |
| 4 | Stable layout | partial | UXF-005 holds: `report.md` token-flow 0.039 (1366x768, dark), offline 0.023, 0.060 at 960x1080 (measured only); the strip enters above everything, so a problem appearing moves the page |
| 5 | Provenance of every value | partial | the Failed calls line and the Routes "Failed, last 5 min" column now agree (4 of 4 and 1 of 1, `review-routes-errors-1366-dark.png`) and the header says why (UXF-026 holds closed); but Calls and Errors beside it name no window on the screen, only a tooltip (UXF-029); the offline line gives no age (UXF-025) |
| 6 | Errors cheap | not observed | no scenario has a fix to make; the link is one action to the section, but the fix is outside the page |
| 7 | Words the user can say | partial | strip sentences, "Failed, last 5 min", "Watchdog heartbeat", "Pods not run by this gateway" read out unchanged (`review-provider-busy-1280-light.png`); UXF-024 holds ("5d0c7e19… ·…" at 390, scratch shot of token-flow); a model name cut into eleven pieces (UXF-028) |
| 8 | Counterpart's time respected | not observed | |
| 9 | Keyboard and pointer parity | pass | `report.md` 0 unreachable, 0 focus invisible, 10 of 10 per theme; the strip lines are real links; the scrolling tables are focusable scroll boxes |
| 10 | Recognition over recall | pass | a named, counted region per table; the failed-calls link now opens a table whose first column after the route holds the number the line gave (`review-routes-errors-1366-dark.png`); the Provider frame names its figures |
| 11 | Calm by default | partial | idle draws nothing and the strip is absent when nothing is wrong (`contact-1366x768-light.png`); reduced motion 0 violations; offline keeps the flow moving on a dead snapshot (UXF-025, `full-1280x720-light.png` offline column) |
| 12 | One state vocabulary | pass | "rejected", "allowed (warning)", "running", "starting", "stale" read the same in strip, cards and tables; amber for the pod shutdown in the strip and in the Pods table (`review-provider-busy-1280-light.png`) |
| 13 | A wrong cue recovered in one action | partial | offline: the flow still reads "1 active session" with a thick in-flight line; the strip corrects it in one glance (UXF-025); a wrong cue that needs an action not observed |
| 14 | Unmistakable end state | pass | idle: "No active sessions.", "Idle.", "No routes yet." (scratch shot of idle 1366 dark); offline: red "offline" and an error line (`full-1280x720-light.png`) |
| 15 | Readable in every theme; key data at 14 px or more | pass | `report.md` key data 10 of 10 in both themes, key 14 px, headings 0 px short, axe 0; the Routes header and strip are legible in dark (`review-routes-errors-1366-dark.png`) |
| 16 | Focus follows the work | partial | at 1366, 1280 and 960 a strip link puts the heading below the 39 px header; at 390 it does not (UXF-027, `review-link-failed-390-light.png`; measured for all four lines of errors and of busy: heading top 55.5 to 56.2 px, header bottom 74.8 px) |

## Looked at in particular

- Failed-calls line: complete, sorted worst first, with the cause in words. 112 characters in errors; at 390 each line is two lines high (measured, 39 px). It is the longest line of the strip and it is the first.
- "Failed, last 5 min" column: second column of Routes, right after the route, in red when non-zero, tooltip with the definition; at 390 it is still on the first screen of the table (`review-routes-390-light.png`). It sits beside two all-time counts that carry no window (UXF-029).
- Provider section: the frame labels are words now ("Pods not run by this gateway", "Watchdog heartbeat", "Account read"); the stale heartbeat is red and says "stale (over 5 min)" (`review-provider-390-dark.png`, `review-provider-busy-1280-light.png`). At 390 the Pods table is the one that breaks (UXF-028).
- Scroll cues at 390: each sideways table has "scroll for more →" above it and an edge fade on the cut side (`.scroller.more-right`); after scrolling Routes to its end the text goes and a left fade shows (measured: cue text empty, `more-left`, left fade opacity 1, right 0). Five sideways tables at 390 on busy, errors and token-flow, three on idle. The cue is right-aligned, 12 px muted text on its own line above each table.
- Heading sizes: every section heading is 15.4 px (the header's title too), the limit cards' h3 14 px, the table cells 14 px bold for the first column; the gate's floor is met. The hierarchy is by weight and about 10 percent of size, with the page title the same size as a section heading. Passes the gate; not filed, no one lost time to it in a scenario.
- Made worse by the last rounds: nothing the gates see. Two things the gates cannot see are UXF-028 (a layout the key-data and overflow probes do not measure, since a wrapped cell is not an overflow) and UXF-029. The sticky header is 75 px of 844 at 390 (9 percent of the screen); with UXF-027 it also covers the heading of every link landing.

## 2. Findings filed

| id | title | severity | class | sessions | cost |
|---|---|---|---|---|---|
| UXF-028 | At 390 px the Pods table gives its Model column 52 px, so one pod's model name breaks into eleven lines and its row is 186 px tall | S3 | layout | busy, errors, token-flow at 390 | S |
| UXF-029 | The Routes table puts a 5 minute count beside two counts since the gateway started and names only the 5 minute window on the screen | S3 | copy | busy, errors, token-flow, every viewport | S |

## Open findings re-judged by sight

| id | still holds | where seen |
|---|---|---|
| UXF-005 | yes: 0.039 per state in token-flow at 1366x768, 0.023 offline, 0.060 at 960x1080 | `report.md` metrics table |
| UXF-013 | in part: the 390 half is fixed (flow box one height); at 1366 idle still gives the panel about 250 px for the line "No active sessions." above an Overview of zeros | scratch shot of idle 1366 dark, `contact-1366x768-light.png` idle |
| UXF-024 | yes: "5d0c7e19… ·…", "e83f10b6… ·…", "1a7b2fe0… ·…" at 390 | scratch shot of token-flow 390 light |
| UXF-025 | yes: offline still draws the flow with a thick in-flight line and moving dots, "1 active session", no age in the strip line | scratch shot of offline 1366 light, `full-1280x720-light.png` |
| UXF-027 | yes: all eight strip links of errors and busy land the heading at 55.5 to 56.2 px under a header ending at 74.8 px | `review-link-failed-390-light.png` |
| UXF-017 (closed) | stays closed: a cue and an edge fade at 390 (`review-routes-390-light.png`, `review-provider-390-dark.png`) | |
| UXF-026 (closed) | stays closed: line and column agree in busy (1 of 3) and errors (4 of 4, 1 of 1) | `review-routes-errors-1366-dark.png`, `review-strip-errors-1366-dark.png` |

## 3. Decision

no decision this round

## 4. Not observed

- 6 Errors cheap: needs a scenario with a fix on the page (a button that restarts or retires a pod, a key to set); the strip's link is one action, the repair is outside the page.
- 8 The counterpart's time respected: needs a counterpart (a second person waiting on the operator); the page has none.
- No recording was replayed; the round has none, and the harness replays were read as pictures and text. The end of a session fading (R-06) was not re-shot this round.
