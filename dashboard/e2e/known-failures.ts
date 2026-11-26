// Hard-gate assertions the page fails today, each held by an open finding. The fix of a finding
// removes its lines here: the assertion then has to pass.
export type Gate =
  | 'gate1-overflow'
  | 'gate1-truncation'
  | 'gate2-always-visible'
  | 'gate5-axe'
  | 'gate5-keyboard'
  | 'gate5-reduced-motion'
  | 'theme-toggle';

export interface KnownFailure {
  gate: Gate;
  finding: string;
  scenarios?: string[];
}

export const KNOWN_FAILURES: KnownFailure[] = [
  { gate: 'theme-toggle', finding: 'UXF-001' },
  { gate: 'gate5-axe', finding: 'UXF-002' },
  { gate: 'gate1-overflow', finding: 'UXF-003', scenarios: ['busy', 'token-flow', 'errors'] },
];

export function knownFailure(gate: Gate, scenario: string): string | null {
  const hit = KNOWN_FAILURES.find((k) => k.gate === gate && (!k.scenarios || k.scenarios.includes(scenario)));
  return hit ? hit.finding : null;
}
