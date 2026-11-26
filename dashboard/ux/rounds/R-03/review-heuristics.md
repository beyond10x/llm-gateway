# Heuristic review, round R-03

5 pass, 6 partial, 0 fail (5 not observed: 1, 6, 8, 13, 16, a read-only monitor with no task in the
scenarios).

Inputs: `report.md`, the four contact sheets, six extra shots of the Token flow panel
(`shots/review-flow-*.png`: 1366x768 light and dark, 390x844 light and dark, 1280x720 dark with
reduced motion, busy at 1280x720 light), and a replay probe of panel and Overview heights over every
state of idle and token-flow at 390x844 and 1366x768. No recordings. Nothing live was contacted.

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | not observed | no task in any scenario |
| 2 | State in words | partial | UXF-009 (stale heartbeat in red only) holds in `contact-1366x768-light.png` errors; flow: in-flight string is thick blue, idle grey, nothing says so (UXF-012, `review-flow-1366-light.png`) |
| 3 | Nothing required hidden | pass | UXF-007 closed: routes columns reach the edge in the token-flow contact sheets; flow gutters clean (`review-flow-1366-light.png`, `review-flow-1366-dark.png`); idle spends the first screen on an empty box (UXF-013, not hiding) |
| 4 | Stable layout | partial | UXF-005 holds: 0.059 (1366x768) and 0.043 (1280x720) per state in `report.md`, down from 0.070 and 0.074; new at 390: the panel grows 190 to 231 px when the first session appears (UXF-013) |
| 5 | Provenance of every value | partial | UXF-010 holds (limit blocks named by `3fa9c1d2` in `contact-1366x768-light.png` busy); flow session ids cut to 8 characters, no full value |
| 6 | Errors cheap | not observed | |
| 7 | Words the user can say | partial | UXF-006 holds: `allowed_warnin` / `g` in both themes of busy (`contact-1366x768-light.png`, `contact-1366x768-dark.png`); the attempts button label is not in any R-03 shot, source unchanged (`App.vue:280-281`) |
| 8 | Counterpart's time respected | not observed | |
| 9 | Keyboard and pointer parity | pass | `report.md`: 0 unreachable, 0 focus invisible, 16 of 16 replays; the flow canvas is aria-hidden and never a target |
| 10 | Recognition over recall | pass | flow caption names the counts, session and route labels are on the strings; Light/Dark/System toggle is labelled (`contact-1366x768-dark.png`) |
| 11 | Calm by default | pass | flow draws nothing idle; static and quiet under reduced motion (`review-flow-1280-reduced-dark.png`); strings stay between the label columns (UXF-008 closed); dots only for calls. Dense dot pile at 390 with 8 states of traffic (`review-flow-390-dark.png`) is noise at that width but has a cause |
| 12 | One state vocabulary | partial | UXF-011 holds: `allowed_warning`, `rejected`, `five_hour`, `RUNNING`, `streaming` side by side (`contact-1366x768-light.png` busy, token-flow, errors) |
| 13 | A wrong cue recovered in one action | not observed | |
| 14 | Unmistakable end state | pass | idle says "No active sessions.", "Idle.", "No routes yet." (`contact-1366x768-light.png` idle) |
| 15 | Readable in every theme; key data at 14 px or more | partial | both themes legible; `report.md` key font 12.25 px (badge "vLLM") in three scenarios; flow labels are 12 px canvas text outside the metric and idle strings are about 1.6:1 light, 1.8:1 dark (UXF-014, `review-flow-busy-1280-light.png`) |
| 16 | Focus follows the work | not observed | |

## Token flow panel

- Legible: yes at 1366x768 and 1280x720 in both themes; labels clear, strings and dots well
  separated from text, caption 14 px at 4.9:1 or better. At 390x844 route names are cut with an
  ellipsis ("claude-hai…", "claude-son…").
- Says what it shows: partly. "3 active sessions → 3 routes" and the two keys name the subject; the
  meaning of string weight, dot direction and "active" is missing, and under reduced motion the key
  lists dots that are never drawn (UXF-012).
- Labels collide: no collision seen in any of the six shots; session and route columns never overlap
  the strings' middle; at 390 the dots pile on top of each other near the session column in the busiest
  state, hiding the strings under them.
- Calm: yes. No motion when idle, none under reduced motion, no stray fragments; the only moving
  parts are call dots.
- Both themes: same drawing, colours from the page palette; dark reads better than light for idle
  strings.

## Open findings re-judged by sight

| id | still holds | where seen |
|---|---|---|
| UXF-005 | yes, smaller: 0.059 / 0.043 per state | `report.md` token-flow rows |
| UXF-006 | yes | `contact-1366x768-light.png` and `-dark.png`, busy, Status tile |
| UXF-009 | yes | `contact-1366x768-light.png` errors: "11m 0s ago" in red, no word |
| UXF-010 | partly seen: hash in the limits heading holds; the "▸ 2" button is in no R-03 shot, source unchanged | busy contact sheets; `App.vue:280-281` |
| UXF-011 | yes | busy and errors contact sheets |

## Findings filed

| id | title | severity | class | sessions | cost |
|---|---|---|---|---|---|
| UXF-012 | The token flow caption does not say what the strings and dots mean; under reduced motion its key points at nothing | S3 | cue | token-flow, busy | S |
| UXF-013 | The flow panel changes height when the first session appears at 390 px, and fills 190 to 230 px with one line when idle | S3 | layout | idle, token-flow (390x844) | S |
| UXF-014 | Flow labels are 12 px canvas text outside the key-font gate; idle strings are about 1.6:1 light, 1.8:1 dark | S3 | layout | busy, token-flow, errors | S |
