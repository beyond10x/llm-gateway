// The ess-ui/1 constructs this renderer draws, as the document holds them after loading
// (src/essui/load.ts): short forms expanded, page kinds merged, unknown kinds and keys refused.
// Anything the document may say that is not typed here is refused at build time.

export type Tone = 'neutral' | 'info' | 'success' | 'warning' | 'danger';
export type Lifecycle = 'connecting' | 'loading' | 'live' | 'reconnecting' | 'stale' | 'closed';

export interface Document {
  format: 'ess-ui/1';
  app: string;
  title?: string;
  model: string;
  actor?: string;
  placement_profile: string;
  fixtures?: { dir?: string; scripts?: Record<string, string> };
  tokens?: Tokens;
  themes?: Record<string, Tokens>;
  theme?: { default: string; chosen_by?: string };
  tone_maps?: Record<string, ToneMap>;
  label_maps?: Record<string, Record<string, string>>;
  shells: Record<string, Shell>;
  navigation: { home: string; sections: { name: string; label?: string; pages: string[] }[]; hidden?: string[] };
  channels?: Record<string, Channel>;
  pages: Record<string, Page>;
}

export interface Tokens {
  color?: Record<string, string>;
  space?: Record<string, string>;
  radius?: Record<string, string>;
  type?: Partial<Record<'body' | 'caption' | 'heading' | 'mono', { family?: string; size?: string; weight?: number }>>;
  tone?: Partial<Record<Tone, { text: string; fill: string }>>;
}

export type ToneMap = Record<string, Tone> | { bands: { from: number; tone: Tone }[] };

export interface StateDecl {
  type: unknown;
  class: string;
  store?: string;
  pinned?: boolean;
  default?: unknown;
}

export interface Shell {
  regions: Record<string, { kind: string }>;
  state?: Record<string, StateDecl>;
}

export interface Channel {
  carries: { view?: string; events?: string[] };
  direction: string;
  delivery: string;
  resume: string;
  scope?: string;
  lifecycle?: Lifecycle[];
  reconnect?: { backoff: { from: string; to: string } };
  stale_after?: string;
  transport_today?: string;
}

export interface Page {
  kind: string;
  title?: string;
  host?: string;
  reason?: string;
  stands_in_for?: string;
  header?: Header;
  layout?: Layout;
  sections: Section[];
}

export interface Header {
  title?: string;
  live?: string[];
  actions?: Action[];
  metrics?: Node[];
}

export interface Action {
  name?: string;
  label?: string;
  as?: 'button' | 'link' | 'choice' | 'toggle';
  navigate?: { to: string; params?: Record<string, string> };
  sets?: Record<string, string>;
  choice?: ChoiceNode;
}

export type Layout = 'stack' | { areas: { grid: string[][]; place: Record<string, string[] | Frame> } };

export interface Frame {
  sections: string[];
  frame?: 'each' | 'shared';
  title?: string;
}

export interface Reads {
  view?: string;
  placeholder?: string;
  fixture?: string;
  key?: string;
  params?: Record<string, string>;
  paging?: 'client' | 'none';
  pager?: { sizes?: number[] };
}

export interface Live {
  channel: string;
  effect: 'replace';
}

export interface SectionStates {
  empty?: { message: string };
}

interface SectionBase {
  name: string;
  title?: string;
  visible?: string;
  live?: Live;
  states?: SectionStates;
  children?: Node[];
}

export type Section = (SectionBase & CompositeNode) | (SectionBase & HostSlot);

export interface HostSlot {
  name?: string;
  host: string;
  reason: string;
  stands_in_for: string;
  reads?: Reads;
}

export interface Field {
  field: string;
  name: string;
  label?: string;
  as?: FieldAs;
  tone?: Tone;
  tone_by?: string;
  labels?: string | Record<string, string>;
  visible?: string;
  help?: { text?: string };
}

export type FieldAs = 'text' | 'number' | 'duration' | 'percent' | 'badge' | 'tags' | 'map' | 'json';

export interface CollectionNode {
  component: 'collection';
  name?: string;
  reads?: Reads;
  live?: Live;
  visible?: string;
  style?: 'table' | 'cards' | 'list';
  columns: Field[];
  expand?: Node;
  // Nodes drawn in each row of a list, after its columns, reading the row as row.
  item?: Node[];
}

export interface RecordNode {
  component: 'record';
  name?: string;
  reads?: Reads;
  live?: Live;
  visible?: string;
  fields?: Field[];
  item?: Node[];
}

export interface MetricNode {
  component: 'metric';
  name?: string;
  reads?: Reads;
  live?: Live;
  visible?: string;
  from?: string;
  aggregate?: 'count' | 'sum' | 'min' | 'max' | 'avg';
  field?: string;
  format?: 'number' | 'duration' | 'percent';
  unit?: 'ms' | 's' | 'm' | 'h';
  window?: string;
  label?: string;
}

export interface ChoiceNode {
  component: 'choice';
  name?: string;
  binds: string;
  style?: 'segmented' | 'radio';
  options: { value: string; label: string }[];
  visible?: string;
}

export type CompositeNode = CollectionNode | RecordNode | MetricNode | ChoiceNode;

export interface TextNode {
  primitive: 'text';
  name: string;
  text: string;
  style?: 'body' | 'caption' | 'heading' | 'mono';
  visible?: string;
}

export interface BadgeNode {
  primitive: 'badge';
  name: string;
  text: string;
  tone?: Tone;
  tone_by?: { value: string; map: ToneMap };
  visible?: string;
}

export interface LinkNode {
  primitive: 'link';
  name: string;
  text: string;
  to?: { to: string };
  href?: string;
  visible?: string;
}

export interface ButtonNode {
  primitive: 'button';
  name: string;
  label: string;
  action: Action;
  tone?: 'primary' | 'secondary' | 'danger' | 'ghost';
  visible?: string;
}

export interface ToggleNode {
  primitive: 'toggle';
  name: string;
  label: string;
  binds: string;
  visible?: string;
}

export type PrimitiveNode = TextNode | BadgeNode | LinkNode | ButtonNode | ToggleNode;

export type Node = CompositeNode | PrimitiveNode | HostSlot;

// What a read answers: rows of named values. A value is a plain value the renderer formats by the
// field's `as` or the metric's `format`, a list (`as: tags`), a map (`as: map`), or a Rich value:
// text the binding has already worded, with what ess-ui/1 has no construct for (a second line, a
// monospaced id, the full value on hover, the key a tone map reads when the text is not it).
export interface Part {
  text: string;
  mono?: boolean;
  title?: string;
}

export interface Rich {
  text: string;
  // The full or raw value, shown on hover.
  title?: string;
  // The value the field's tone map reads and its label map words, when it is not the text.
  key?: string;
  mono?: boolean;
  // May wrap between words, where a cell otherwise keeps to one line.
  wrap?: boolean;
  muted?: boolean;
  // Muted words on the same line after the text.
  note?: Part;
  // A small label after the text.
  tag?: Part;
  // Lines under the text.
  sub?: Part[];
  // What the label means, shown on hover over it (a metric's label has no help of its own).
  about?: string;
  // A small table under the value, and the words it shows when it has no row.
  table?: Rich[][];
  empty?: string;
}

export type Value = string | number | boolean | null | Rich | string[] | Record<string, string>;
export type Row = Record<string, Value>;

export function isRich(v: unknown): v is Rich {
  return typeof v === 'object' && v !== null && !Array.isArray(v) && typeof (v as Rich).text === 'string';
}
