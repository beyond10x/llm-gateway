// Hard-gate failures the page has today, each held by an open finding. An entry names the exact
// items it holds: an axe rule id and node target (`<rule> <target>`), an element's selector, or an
// always-visible element's name. Any item it does not name fails its gate, and an item it names
// that no replay of its scenarios shows any more fails too, so the fix of a finding removes its
// lines here. `viewports` limits an entry to those viewports.
// An entry without items holds a whole single-valued check (the token-flow canvas): its test is
// marked `test.fail` and fails once the check passes.
// A copy item is `<kind> <place>` (e2e/copy.ts); a key-data item is `<class> <px>px <element>`.
export type Check = 'overflow' | 'truncation' | 'header' | 'axe' | 'keyboard' | 'reduced-motion' | 'key-data' | 'copy' | 'flow';

export interface KnownFailure {
  finding: string;
  check: Check;
  scenarios?: string[];
  viewports?: string[];
  items?: string[];
}

const ALL = ['idle', 'busy', 'token-flow', 'errors', 'offline'];

export const KNOWN_FAILURES: KnownFailure[] = [
  // Words run together where the renderer drops the space before a second line or between tags.
  { finding: 'UXF-016', check: 'copy', scenarios: ALL, items: ['run-together #h-kpi div.tile div.sub'] },
  { finding: 'UXF-016', check: 'copy', scenarios: ['busy', 'errors', 'token-flow', 'offline'], items: ['run-together #h-sessions th.col-session'] },
  { finding: 'UXF-016', check: 'copy', scenarios: ['busy', 'errors', 'token-flow'], items: ['run-together #h-kpi th', 'run-together #h-routes td.col-cache'] },
  { finding: 'UXF-016', check: 'copy', scenarios: ['busy', 'errors'], items: ['run-together #h-inflight td.col-state span.pill'] },
  { finding: 'UXF-016', check: 'copy', scenarios: ['busy'], items: ['run-together #h-calls td.col-flags span.flag'] },
  // A limit card named by a hash, raw header names as labels, the attempts button named "▸ 2".
  { finding: 'UXF-010', check: 'copy', scenarios: ['busy', 'errors'], items: ['hex-title #h-kpi h3 span.mono', 'glyph-control #h-calls button.expand'] },
  { finding: 'UXF-010', check: 'copy', scenarios: ['busy'], items: ['header-name #h-kpi dt.mono'] },
  // Raw header values shown as words ("five_hour", "out_of_credits"): a second vocabulary.
  { finding: 'UXF-011', check: 'copy', scenarios: ['busy', 'errors'], items: ['snake-value #h-kpi div.tile div.value', 'snake-value #h-kpi div.tile div.sub'] },
  // Section headings at 13.3 px and limit card titles at 12.6 px, under the 14 px heading floor.
  { finding: 'UXF-021', check: 'key-data', scenarios: ALL, items: ['heading 13.3px h2'] },
  { finding: 'UXF-021', check: 'key-data', scenarios: ['busy', 'errors'], items: ['heading 12.6px h3', 'heading 12.6px span.mono'] },
];

function applies(k: KnownFailure, check: Check, scenario: string, viewport?: string): boolean {
  return k.check === check && (!k.scenarios || k.scenarios.includes(scenario)) && (!k.viewports || viewport === undefined || k.viewports.includes(viewport));
}

// The items of one replay that no entry holds.
export function unexpected(check: Check, scenario: string, items: string[], list: KnownFailure[] = KNOWN_FAILURES, viewport?: string): string[] {
  const held = new Set(list.filter((k) => applies(k, check, scenario, viewport)).flatMap((k) => k.items ?? []));
  return items.filter((i) => !held.has(i));
}

// The items an entry names for the scenario that none of its replays showed.
export function stale(check: Check, scenario: string, seen: Set<string>, list: KnownFailure[] = KNOWN_FAILURES): string[] {
  return list
    .filter((k) => applies(k, check, scenario))
    .flatMap((k) => (k.items ?? []).filter((i) => !seen.has(i)).map((i) => `${k.finding}: ${i}`));
}

// The finding that holds a whole check for the scenario, or null.
export function wholeCheck(check: Check, scenario: string, list: KnownFailure[] = KNOWN_FAILURES): string | null {
  return list.find((k) => applies(k, check, scenario) && !k.items)?.finding ?? null;
}
