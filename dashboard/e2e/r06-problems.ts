// What the alert strip of change set R-06 (UXF-019) must name, worked out from a scenario's snapshot
// alone and not from the page's binding, and the one problem no scenario holds: a connection that is
// not live, made by failing the replay's event stream.
import type { Page } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import { settle } from './harness.ts';

export type Kind = 'connection' | 'route' | 'watchdog' | 'limit' | 'pod';

// How a person recognises each line, and the section it opens (none for the connection, which has
// no section: the header says its state).
export const KINDS: Record<Kind, { words: RegExp; href: string | null }> = {
  connection: { words: /^Connection: /, href: null },
  route: { words: /^Failed calls in the last \d+ min: \S/, href: '#h-routes' },
  watchdog: { words: /^Watchdog heartbeat (stale|missing)/, href: '#h-provider' },
  limit: { words: /^Subscription limits · \S/, href: '#h-kpi' },
  pod: { words: /^Pod \S+ shuts down /, href: '#h-provider' },
};

const WINDOW_MS = 5 * 60_000;
const UNIFIED = 'anthropic-ratelimit-unified-';

// The kinds of problem a snapshot holds: a call on a route that failed in the last 5 minutes, a
// watchdog heartbeat older than 5 minutes or missing, a unified limit rejected or at a warning, a
// pod this gateway stops within 5 minutes.
export function kindsOf(s: Snapshot): Kind[] {
  const now = s.now_ms;
  const out: Kind[] = [];
  if (s.calls.some((c) => c.route !== null && (c.outcome !== 'ok' || c.status >= 400) && now - c.started_ms <= WINDOW_MS)) out.push('route');
  const age = s.provider?.watchdog_heartbeat_age_ms;
  if (s.provider && (age === null || age === undefined || age > WINDOW_MS)) out.push('watchdog');
  const statuses = (h: Record<string, string>) => ['status', '5h-status', '7d-status'].map((k) => h[UNIFIED + k]);
  if (s.limits.some((l) => statuses(l.headers).some((v) => v === 'rejected' || v === 'allowed_warning'))) out.push('limit');
  if (s.provider?.pods.some((p) => !p.external && p.shutdown && p.shutdown.at_ms - now < WINDOW_MS)) out.push('pod');
  return out;
}

// The event stream fails: the page polls, and its connection is no longer live.
export async function dropConnection(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as { __uxStream: { fail(): void } }).__uxStream.fail());
  await settle(page);
}

export interface Line {
  text: string;
  level: string;
  href: string | null;
  target: boolean;
  box: { top: number; bottom: number; left: number; right: number };
  covered: number;
}

// Every line of the strip at the top of the page: its words without the severity, the severity, its
// link and whether the link's target exists, its box in the viewport, and how many of five points
// inside it (the corners, 2 px in, and the centre) show something other than the line.
export async function stripLines(page: Page): Promise<Line[]> {
  await page.evaluate(() => window.scrollTo(0, 0));
  await settle(page);
  return page.getByRole('region', { name: 'Problems', exact: true }).locator('li').evaluateAll((lis) =>
    lis.map((li) => {
      const r = li.getBoundingClientRect();
      const points = [
        [r.left + 2, r.top + 2],
        [r.right - 2, r.top + 2],
        [r.left + 2, r.bottom - 2],
        [r.right - 2, r.bottom - 2],
        [(r.left + r.right) / 2, (r.top + r.bottom) / 2],
      ];
      const covered = points.filter(([x, y]) => {
        const hit = document.elementFromPoint(x!, y!);
        return !hit || !li.contains(hit);
      }).length;
      const text = (li as HTMLElement).innerText.replace(/\s+/g, ' ').trim();
      const level = /^(error|warning)\b/.exec(text)?.[1] ?? '';
      const href = li.querySelector('a')?.getAttribute('href') ?? null;
      return {
        text: text.replace(/^(error|warning)\s+/, ''),
        level,
        href,
        target: href !== null && href.startsWith('#') && document.getElementById(href.slice(1)) !== null,
        box: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
        covered,
      };
    }),
  );
}
