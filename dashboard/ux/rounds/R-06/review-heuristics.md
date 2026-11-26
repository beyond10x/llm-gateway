# Heuristic review, round R-06

6 pass, 8 partial, 0 fail (2 not observed: 6, 8; a read-only monitor with no task that fixes anything
and no counterpart).

Inputs: `report.md`, `round.json`, the six sheets in `shots/` (contact and full-page), six extra shots
(`shots/review-*.png`), the findings UXF-001 to UXF-023, and replays through the round's own harness
(pinned fonts, hermetic, no recording): errors, busy, offline and token-flow at 1366x768, 1280x720 and
390x844, both themes, read as pictures and as text. The Problems links were clicked at 1280, 960 and
390 and the landing measured. A session that fades was made by pushing the last token-flow state with
one session's calls and activity 400 s old. No gateway was contacted. The two contact sheets for
offline are taken before the connection drops (header "live", no strip), so only `full-…` and
`review-offline-1366-light.png` show the lost state.

## 1. Heuristics

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | pass | the strip is the next thing: one line per kind of problem, worst first, each a link to the section; `review-problems-errors-dark-1280.png`; `report.md` gate 4, 8 of 8 tasks, 2 actions at most |
| 2 | State in words | pass | the severity is a word in each line ("error", "warning"), the lines are sentences ("Watchdog heartbeat stale: last 11m 0s ago, over 5 min; pods may not be stopped on schedule"), a lost connection is "offline" plus "Connection: the gateway does not answer; the page shows the last snapshot" (`review-offline-1366-light.png`) |
| 3 | Nothing required hidden | partial | desktop pass (strip fully on the first screen at 1366 and 1280, `contact-1366x768-light.png`, busy and errors); at 390 UXF-017 holds (Routes shows Route, Calls, Errors only, no cue) and a strip link lands with the section heading under the header (UXF-027) |
| 4 | Stable layout | partial | UXF-005 holds: `report.md` token-flow 0.039 (1366x768), offline 0.024, 0.060 at 960x1080; the strip enters and leaves above everything else, so a problem appearing moves the whole page (the offline shift is that insertion) |
| 5 | Provenance of every value | partial | "Failed calls in the last 5 min: claude-sonnet 1 of 3" is not on the screen its link opens (UXF-026, `review-problems-link-routes-busy-1366-light.png`); UXF-018 holds ("Name", "Other pods", "Read 10s ago", `review-problems-link-watchdog-dark-1280.png`); the offline line gives no age (UXF-025) |
| 6 | Errors cheap | not observed | no scenario has a fix to make; the link is one action to the section (`review-problems-link-watchdog-dark-1280.png`) but the fix itself is outside the page |
| 7 | Words the user can say | partial | the strip's sentences are sayable; UXF-018 holds; new: "5d0c7e19… ·…" at 390 (UXF-024, `review-flow-390-light.png`); "4 of 4" does not say calls (UXF-026); UXF-010, UXF-016 stay closed (headings "Subscription limits · unified", "last 60 s" with its space) |
| 8 | Counterpart's time respected | not observed | |
| 9 | Keyboard and pointer parity | pass | `report.md` 0 unreachable, 0 focus invisible, 10 of 10 per theme; each strip line is a real link; the expander is a button |
| 10 | Recognition over recall | partial | every table is a named region with a count; the strip names where to look; but its Failed calls line opens Routes, which holds all-time counts, not the failed calls (UXF-026) |
| 11 | Calm by default | partial | idle draws nothing (`contact-1366x768-light.png` idle); no strip when nothing is wrong; reduced motion 0 violations; but offline keeps the flow moving and a blue in-flight line on a dead snapshot (UXF-025, `review-offline-1366-light.png`) |
| 12 | One state vocabulary | pass | one table of state words (UXF-011 closed); the strip says "rejected" and "allowed (warning)" as the cards do (`review-problems-errors-dark-1280.png`); amber for the pod shutdown in the strip and in the Pods table (`review-problems-link-watchdog-dark-1280.png`); the header says "connecting" for the seconds between a dropped stream and the first failed poll with no strip line, then "offline" (read as text, no shot) |
| 13 | A wrong cue recovered in one action | partial | offline: the flow still reads "1 active session" with a thick in-flight line; the strip above corrects it in one glance, no action (UXF-025); not observed for a wrong cue that needs an action |
| 14 | Unmistakable end state | pass | idle: "No active sessions.", "Idle.", "No routes yet."; offline: red "offline" in the header and an error line (`full-1280x720-light.png` idle and offline columns, `review-offline-1366-light.png`) |
| 15 | Readable in every theme; key data at 14 px or more | partial | axe 0, key font 14 px in both themes; `report.md` key-data gate 0 of 10 because headings are 13.3 and 12.6 px, held by UXF-021 (the Problems heading is one); session labels are legible once settled in both themes (`review-flow-fade-light.png` rows two and three); while a session joins or ends its label is dim for about 0.8 s of its 1.25 s fade, and its dots keep full colour after the label is nearly gone (`review-flow-fade-light.png` top row); not filed, it costs the reader no action |
| 16 | Focus follows the work | pass | a strip link scrolls its heading to 56 px, below the 39 px header at 1366, 1280 and 960 (measured for all four lines; `review-problems-link-watchdog-dark-1280.png`); at 390 it does not (UXF-027) |

## Problems strip, in short

- Wording: complete sentences with the numbers in them; "Subscription limits · unified: allowed (warning)" repeats the severity word inside a line that is already a "warning"; "claude-sonnet 4 of 4" lacks "calls".
- Order: connection, then failed calls, watchdog, limits, pods; errors before warnings in all four scenarios that hold them. A fixed order by kind, not by age or time left: a pod that stops in 54 s is the last line of the strip.
- Severity: red "error" for failed calls, stale watchdog, rejected limit, lost connection; amber "warning" for an allowed-with-warning limit and a pod about to stop. A word and a tone, both themes legible (`review-problems-errors-dark-1280.png`, `contact-1366x768-light.png`).
- Links: all four land on the right section at 1366, 1280 and 960. Pods and Watchdog both land on Provider, Limits on Overview (its tiles first, the limit cards below). At 390 the heading is under the header (UXF-027).
- Connection drop: between the stream error and the first failed poll the header says "connecting" and the strip is empty; after the poll the header says "offline" and the strip holds one error line, first; the other problems stay below it, from the old snapshot. No age is given (UXF-025).

## Token flow labels

- Settled, 1366 and 1280, both themes: whole ("5d0c7e19… · claude-code"), 14 px, high contrast (`review-flow-fade-light.png`, `contact-1366x768-dark.png`).
- Joining (dark, 1366): the new row starts at nothing and reaches full over 1.25 s; at 0.1 s the label is barely there.
- Ending (light, 1366, made by hand: no scenario ends a session, because the five minute window keeps token-flow's sessions active): the label goes pale before its strings and dots; the caption count drops at once ("2 active sessions") while three rows are still drawn.
- 390: every session label loses the client name (UXF-024). Route names stay whole (UXF-014 holds).

## Open findings re-judged by sight

| id | still holds | where seen |
|---|---|---|
| UXF-004 | no: key font 14 px, deficit 0, secondary 0 in every replay; what is left is headings (UXF-021); can be closed | `report.md` metrics table |
| UXF-005 | yes: 0.039 / 0.035 per state in token-flow, offline 0.024 and 0.060 at 960 | `report.md` metrics table |
| UXF-013 | in part: the flow box is 140 px at 390 in idle, token-flow and busy, no jump when the first session appears (measured); at 1366 idle still gives the panel about 230 px for the line "No active sessions." | `contact-1366x768-light.png` idle |
| UXF-017 | yes: at 390 Routes shows Route, Calls, Errors, Recent calls is cut at Status, no cue | a scratch shot at 390, not kept |
| UXF-018 | yes: "Name runpod", "Other pods 0", "Read 10s ago" | `review-problems-link-watchdog-dark-1280.png` |
| UXF-020 | yes: Provider balance sits alone on a second row of Overview at 1280 (busy, errors) | `full-1280x720-light.png` busy and errors |
| UXF-021 | yes: section headings are the size of the table cells; `report.md` heading deficit 1.4 px | `review-problems-link-watchdog-dark-1280.png` |
| UXF-022 | no as written: the offline scenario replays the loss, the report gates "a lost connection in words" and `full-…` shows it; the remaining gap is UXF-025 and the contact sheet taken before the drop; can be closed | `report.md` hard gates, `review-offline-1366-light.png` |

## 2. Findings filed

| id | title | severity | class | sessions | cost |
|---|---|---|---|---|---|
| UXF-024 | At 390 px every session label in the token flow is cut to its id and an ellipsis, so the client's name is never drawn | S3 | copy | busy, token-flow, errors at 390 | S |
| UXF-025 | When the connection is lost the page says so but keeps drawing the last snapshot as live, with no age | S3 | cue | offline | S |
| UXF-026 | The Failed calls line counts the last 5 minutes and links to Routes, which counts everything, so its number is not on the screen it opens | S3 | copy | busy, errors | S |
| UXF-027 | At 390 px a Problems link opens its section with the heading under the sticky header | S3 | layout | busy, errors at 390 | S |

## 3. Decision

No decision this round.

## 4. Not observed

- 6 Errors cheap: needs a scenario with a task that fixes something (a stopped pod, a refused key) and a count of actions.
- 8 Counterpart's time respected: needs a scenario with a second person waiting on the page.
- A session ending by itself in a scenario (five minutes without a call): the fade-out and the "N active sessions" caption were seen only on a hand-made state; a scenario state 6 minutes on would show it.
- Keyboard use of the strip links (Tab order, focus after the jump): the report gates reachability, not where focus is after the jump.
- Reduced motion and the strip's links together at 390.
