# R-02 heuristic review

Method: the four contact sheets (1366x768 and 1280x720, light and dark), `report.md`, the scenario
sources, the page source, and four shots taken in the hermetic harness (`shots/review-*.png`). There
are no recordings in this round. The page is the built `dashboard.html` (sha256 `82ceb6386f06`),
the one the round measured.

2 pass, 8 partial, 2 fail (4 not observed)

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | partial | contact-1280x720-light.png idle: empty tables and "Idle."; only the small "Setup" link points anywhere. No finding: it costs no time on a monitor |
| 2 | State in words | partial | review-errors-attempts-light.png: stale watchdog is red only (UXF-009); status, pod and call states are words |
| 3 | Nothing required hidden | fail | review-errors-full-dark.png and contact-1366x768-dark.png: routes table clipped at the panel edge (UXF-007); cache route table over the Throughput tile (UXF-003) |
| 4 | Stable layout | partial | report.md layout shift 0.074 (UXF-005); review-errors-attempts-light.png: expanding attempts opens in place and moves nothing above it |
| 5 | Provenance of every value | partial | review-busy-full-light.png: "seen 11s ago", "read 20s ago" and "external, not stopped by this gateway" are good; limit blocks named by a hash (UXF-010) |
| 6 | Errors cheap | not observed | the page has no editing action; the harness performs none |
| 7 | Words the user can say | partial | review-busy-full-light.png: "allowed_warni/ng" (UXF-006), "▸ 2" (UXF-010) |
| 8 | The counterpart's time respected | not observed | no counterpart in a read-only dashboard |
| 9 | Keyboard and pointer parity | partial | report.md keyboard gate 8 of 8; axe scrollable-region-focusable 12 nodes (UXF-002); theme radios and expanders are native controls |
| 10 | Recognition over recall | pass | every table has a caption and column headers; units on every value (review-busy-full-light.png) |
| 11 | Calm by default | partial | report.md reduced motion 0 violations; stray flow-layer fragments in the gutters (UXF-008, review-flow-gutter-light.png) |
| 12 | One state vocabulary | partial | review-busy-full-light.png: lower-case wire words, capital badges, sentences, raw header values (UXF-011) |
| 13 | A wrong cue recovered in one action | not observed | no wrong-cue path in the scenarios |
| 14 | Unmistakable end state | not observed | a live monitor has no end state; idle reads "Idle." and "No sessions yet." (contact-1366x768-light.png) |
| 15 | Readable in every theme; key data 14 px or more | fail | report.md key font 10.4 px (UXF-004); contrast is fine in both themes (contact-*-dark.png, contact-*-light.png) |
| 16 | Focus follows the work | pass | review-errors-attempts-light.png: the expander keeps the row in place and shows the attempts under it |

## Findings filed

| id | title | severity | class | sessions | cost |
|---|---|---|---|---|---|
| UXF-007 | Routes table cut off at the right edge with no cue | S2 | affordance | idle, busy, token-flow, errors | M |
| UXF-008 | Ambient flow layer leaves stray fragments in the gutters | S3 | noise | busy, token-flow, errors | S |
| UXF-009 | Stale watchdog heartbeat said only in red | S3 | cue | busy, errors | S |
| UXF-010 | Opaque labels: limit-block hash, "▸ 2" button | S3 | copy | busy, token-flow, errors | S |
| UXF-011 | Three vocabularies for state words | S3 | copy | busy, token-flow, errors | S |

Already recorded and confirmed by sight, not refiled: UXF-002, 003, 004, 005, 006.
