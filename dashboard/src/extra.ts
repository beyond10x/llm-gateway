// The page's view of the snapshot: the generated types plus the fields a newer gateway sends.
// Every addition is optional, so the page renders against an older snapshot too, and the
// intersections stay valid once generated/snapshot.ts carries the same fields.
import type * as G from './generated/snapshot.ts';

export type { AttemptRecord, SessionRow } from './generated/snapshot.ts';

export interface RouteRate {
  route: string;
  input_tps: number | null;
  output_tps: number | null;
}

export interface Kpi {
  input_tokens: number;
  output_tokens: number;
  input_rate: number | null;
  output_rate: number | null;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  uncached_input_tokens: number;
  cache_hit_rate: number | null;
  route_rates: RouteRate[];
}

export interface LimitRow {
  credential: string;
  seen_ms: number;
  headers: Record<string, string>;
}

export interface PodShutdown {
  at_ms: number;
  limit: 'idle' | 'max-runtime' | 'balance';
}

export type CallRecord = G.CallRecord & {
  input_tps?: number | null;
  output_tps?: number | null;
  usage_note?: string | null;
};

export type InFlight = G.InFlight;

export type RouteRow = G.RouteRow & {
  input_tps?: number | null;
  output_tps?: number | null;
  uncached_input_tokens?: number;
};

export type PodRow = G.PodRow & { shutdown?: PodShutdown | null };

export type ProviderPanel = Omit<G.ProviderPanel, 'pods'> & { pods: PodRow[] };

export type Snapshot = Omit<G.Snapshot, 'calls' | 'routes' | 'provider' | 'kpi' | 'limits'> & {
  calls: CallRecord[];
  routes: RouteRow[];
  provider: ProviderPanel | null;
  kpi?: Kpi;
  limits?: LimitRow[];
};
