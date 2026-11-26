// The viewports and themes of the layout contract.
export interface Viewport {
  name: string;
  width: number;
  height: number;
  gating: boolean;
}

export const VIEWPORTS: Viewport[] = [
  { name: '1366x768', width: 1366, height: 768, gating: true },
  { name: '1280x720', width: 1280, height: 720, gating: true },
  { name: '1536x754', width: 1536, height: 754, gating: false },
  { name: '960x1080', width: 960, height: 1080, gating: false },
  { name: '390x844', width: 390, height: 844, gating: false },
];

export const GATING = VIEWPORTS.filter((v) => v.gating);

export type Theme = 'light' | 'dark';
export const THEMES: Theme[] = ['light', 'dark'];

// Key data below this size fails heuristic 15.
export const KEY_FONT_PX = 14;

export const AXE_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];
