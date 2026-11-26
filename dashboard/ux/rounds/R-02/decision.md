# R-03 decision: does the token flow get its own panel at the top of the page?

Finding: UXF-008. Target metric: flow.visible_ratio, now 0.005 to 0.09 (91 to 99.5 % of the canvas
behind opaque panels at 1366x768), expected ≥ 0.95.
Yes: a full-width "Token flow" panel above Overview, about 180 px high, in which the canvas draws
the session strings, labels and particles; the background layer goes.
No: the canvas stays fixed behind the page and shows in the gaps between panels.
Other options put: translucent panels over the background canvas (costs text contrast), or no change.
Default if unanswered by the next round: yes, because only the panel keeps text contrast and shows
the session labels.
Answer: yes, 2026-11-26 (taken on default after 15 minutes, then confirmed by the owner)
