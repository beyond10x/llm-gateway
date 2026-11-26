# R-06 decision: does the page get an alert strip under the header?

Finding: UXF-019. Target metric: screen_only.looks for the tasks "find the failing route" and "read
the watchdog state", now below the fold (glance 3) in busy and errors at 1280x720, expected glance 1.
Yes: a strip under the header, shown only while something is wrong, with one line per current
problem (a route with errors, a stale watchdog heartbeat, a rejected or warning limit, a pod that
shuts down within 5 minutes, the connection not live), each linking to its section; no strip when
nothing is wrong.
No: the sections stay as they are; the reader scrolls to In flight, Routes and Provider.
Default if unanswered by the next round: yes, because the failures are the page's most urgent
content and today none of them is on the first screen.
Answer: pending
