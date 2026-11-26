// The fonts every replay renders with. The page asks for `system-ui` and a monospace stack, which
// each host resolves to the fonts it has installed, with other widths: a table that fits on one
// host scrolls sideways on another. The harness therefore installs committed font files in every
// replay, so every host measures the same widths. The shipped page is not changed.
//
// The files are the Latin-1 subsets of IBM Plex Sans and IBM Plex Mono 1.1.0 (SIL Open Font
// License 1.1, `fonts/OFL.txt`), taken unchanged from `fonts/split/woff2/` of the release assets
// `ibm-plex-sans.zip` and `ibm-plex-mono.zip` of github.com/IBM/plex, tags `@ibm/plex-sans@1.1.0`
// and `@ibm/plex-mono@1.1.0`. A character outside the subset (an arrow, a triangle) still comes
// from a host font.
//
// How they are installed, before any script of the page runs:
// - the sans faces under their own family name, which a constructed stylesheet sets on the root and
//   the form controls: `system-ui`, the first family the page asks for, is a generic family that
//   an @font-face cannot take over;
// - the mono faces under `SFMono-Regular`, the first family of the page's monospace stack that
//   Chromium on Linux looks up by name (`ui-monospace` matches nothing there), so the canvas, which
//   names the same stack, draws with them too.
//
// UX_FONTS=host skips them and measures with the host's fonts; UX_FONTS=host:<family> also sets the
// installed family <family> in place of `system-ui` (`host:DejaVu Sans` gives the widths of an
// Ubuntu runner with Playwright's dependencies on a host that has DejaVu Sans). Neither is the
// gate: the ratchet holds only what the pinned fonts measure.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { Page } from '@playwright/test';

export const FONTS_DIR = fileURLToPath(new URL('./fonts/', import.meta.url));
export const SANS_FAMILY = 'IBM Plex Sans';
export const MONO_FAMILY = 'SFMono-Regular';
export const PINNED_LABEL = 'pinned: IBM Plex Sans and IBM Plex Mono 1.1.0, Latin-1';
export const MAX_FONT_BYTES = 200 * 1024;

export interface PinnedFont {
  file: string;
  family: string;
  weight: number;
  sha256: string;
}

export const PINNED_FONTS: PinnedFont[] = [
  { file: 'IBMPlexSans-Regular-Latin1.woff2', family: SANS_FAMILY, weight: 400, sha256: 'b5ad7bd39f996144915f0ad9849a90183b27d8c28ad97ed98af5b1bebc51f6b1' },
  { file: 'IBMPlexSans-SemiBold-Latin1.woff2', family: SANS_FAMILY, weight: 600, sha256: 'fff0ab3a88b0b4aa0b693e4f0201359a15183b08e3fa5696d1918d8f0ade8ad5' },
  { file: 'IBMPlexMono-Regular-Latin1.woff2', family: MONO_FAMILY, weight: 400, sha256: '10d3c7fa7eaf48e78db24f317b64f008a75e00f63a68bb3c2afc6ef51e58674f' },
  { file: 'IBMPlexMono-SemiBold-Latin1.woff2', family: MONO_FAMILY, weight: 600, sha256: '1ce95cff1c5056cb0fed049c2912823293b158b816e193a6f937f2d92b1e0f39' },
];

export type FontMode = { kind: 'pinned' } | { kind: 'host'; sans: string | null };

export function fontMode(value = process.env.UX_FONTS ?? ''): FontMode {
  if (value === '' || value === 'pinned') return { kind: 'pinned' };
  if (value === 'host') return { kind: 'host', sans: null };
  if (value.startsWith('host:') && value.length > 'host:'.length) return { kind: 'host', sans: value.slice('host:'.length) };
  throw new Error(`UX_FONTS=${value}: expected pinned, host or host:<installed family>`);
}

export function fontsLabel(mode: FontMode = fontMode()): string {
  if (mode.kind === 'pinned') return PINNED_LABEL;
  return mode.sans ? `host, sans set to ${mode.sans}` : 'host';
}

const sha256 = (b: Buffer) => createHash('sha256').update(b).digest('hex');

// What is wrong with the committed font files: a file whose digest is not the declared one, more
// bytes than MAX_FONT_BYTES, or no licence beside them.
export function fontFileProblems(fonts: PinnedFont[] = PINNED_FONTS, dir: string = FONTS_DIR): string[] {
  const problems: string[] = [];
  let total = 0;
  for (const f of fonts) {
    let bytes: Buffer;
    try {
      bytes = readFileSync(`${dir}${f.file}`);
    } catch {
      problems.push(`${f.file}: missing`);
      continue;
    }
    total += bytes.length;
    const digest = sha256(bytes);
    if (digest !== f.sha256) problems.push(`${f.file}: sha256 ${digest}, declared ${f.sha256}`);
  }
  if (total > MAX_FONT_BYTES) problems.push(`the font files hold ${total} bytes, more than ${MAX_FONT_BYTES}`);
  let licence = '';
  try {
    licence = readFileSync(`${dir}OFL.txt`, 'utf8');
  } catch {
    // reported below
  }
  if (!licence.includes('SIL Open Font License, Version 1.1')) problems.push('OFL.txt: no SIL Open Font License 1.1 beside the fonts');
  return problems;
}

// The init script that installs the fonts of `mode`, or null for the host's own fonts.
function initScript(mode: FontMode): string | null {
  const rule = (family: string) => `:root, button, input, select, textarea { font-family: ${JSON.stringify(family)} !important; }`;
  if (mode.kind === 'host') {
    if (!mode.sans) return null;
    return `(() => {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(${JSON.stringify(rule(mode.sans))});
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    })();`;
  }
  const problems = fontFileProblems();
  if (problems.length > 0) throw new Error(`pinned fonts: ${problems.join('; ')}`);
  const faces = PINNED_FONTS.map((f) => [f.family, f.weight, readFileSync(`${FONTS_DIR}${f.file}`).toString('base64')]);
  // A FontFace made from bytes is parsed when it is made: it is loaded before the first layout.
  return `(() => {
    for (const [family, weight, data] of ${JSON.stringify(faces)}) {
      const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
      const face = new FontFace(family, bytes, { weight: String(weight), style: 'normal' });
      if (face.status !== 'loaded') throw new Error('pinned font ' + family + ' ' + weight + ': ' + face.status);
      document.fonts.add(face);
    }
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(${JSON.stringify(rule(SANS_FAMILY))});
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  })();`;
}

let script: { mode: string; content: string | null } | null = null;

// Installs the fonts of the current mode in every document the page loads from now on.
export async function pinFonts(page: Page): Promise<void> {
  const mode = fontMode();
  const key = JSON.stringify(mode);
  if (script?.mode !== key) script = { mode: key, content: initScript(mode) };
  if (script.content) await page.addInitScript({ content: script.content });
}
