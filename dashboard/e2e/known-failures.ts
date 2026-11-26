// Hard-gate failures the page has today, each held by an open finding. An entry names the exact
// items it holds: an axe rule id and node target (`<rule> <target>`), or an element's selector.
// Any item it does not name fails its gate, and an item it names that no gating replay of its
// scenarios shows any more fails too, so the fix of a finding removes its lines here.
// An entry without items holds a whole single-valued check (the token-flow canvas): its test is
// marked `test.fail` and fails once the check passes.
export type Check = 'overflow' | 'truncation' | 'header' | 'axe' | 'keyboard' | 'reduced-motion' | 'flow';

export interface KnownFailure {
  finding: string;
  check: Check;
  scenarios?: string[];
  items?: string[];
}

export const KNOWN_FAILURES: KnownFailure[] = [
  { finding: 'UXF-002', check: 'axe', items: ['scrollable-region-focusable section[aria-labelledby="h-routes"] > .scroll'] },
  { finding: 'UXF-002', check: 'axe', scenarios: ['busy', 'token-flow', 'errors'], items: ['scrollable-region-focusable section[aria-labelledby="h-inflight"] > .scroll'] },
  { finding: 'UXF-003', check: 'overflow', scenarios: ['busy', 'errors'], items: ['section[aria-labelledby="h-kpi"] > div.tiles:nth-of-type(1) > div.tile:nth-of-type(3) > table.mini'] },
  { finding: 'UXF-003', check: 'overflow', scenarios: ['token-flow'], items: ['section[aria-labelledby="h-kpi"] > div.tiles > div.tile:nth-of-type(3) > table.mini'] },
  { finding: 'UXF-008', check: 'flow', scenarios: ['token-flow'] },
];

function applies(k: KnownFailure, check: Check, scenario: string): boolean {
  return k.check === check && (!k.scenarios || k.scenarios.includes(scenario));
}

// The items of one replay that no entry holds.
export function unexpected(check: Check, scenario: string, items: string[], list: KnownFailure[] = KNOWN_FAILURES): string[] {
  const held = new Set(list.filter((k) => applies(k, check, scenario)).flatMap((k) => k.items ?? []));
  return items.filter((i) => !held.has(i));
}

// The items an entry names for the scenario that none of its gating replays showed.
export function stale(check: Check, scenario: string, seen: Set<string>, list: KnownFailure[] = KNOWN_FAILURES): string[] {
  return list
    .filter((k) => applies(k, check, scenario))
    .flatMap((k) => (k.items ?? []).filter((i) => !seen.has(i)).map((i) => `${k.finding}: ${i}`));
}

// The finding that holds a whole check for the scenario, or null.
export function wholeCheck(check: Check, scenario: string, list: KnownFailure[] = KNOWN_FAILURES): string | null {
  return list.find((k) => applies(k, check, scenario) && !k.items)?.finding ?? null;
}
