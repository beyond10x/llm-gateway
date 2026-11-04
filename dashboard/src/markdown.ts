// The subset of Markdown the gateway's /llms.txt is written in: headings, paragraphs, tables
// and fenced code blocks, with `code` and [links](url) inline. The page renders the text the
// server embedded, so it cannot show a command /llms.txt does not hold.

export type Inline = { kind: 'text' | 'code'; text: string } | { kind: 'link'; text: string; href: string };

export type Block =
  | { kind: 'heading'; level: number; text: Inline[] }
  | { kind: 'paragraph'; text: Inline[] }
  | { kind: 'table'; head: Inline[][]; rows: Inline[][][] }
  | { kind: 'code'; lang: string; code: string };

export function inline(source: string): Inline[] {
  const out: Inline[] = [];
  const pattern = /`([^`]+)`|\[([^\]]+)\]\(([^)\s]+)\)|<(https?:\/\/[^>\s]+)>/g;
  let at = 0;
  for (const match of source.matchAll(pattern)) {
    const index = match.index ?? 0;
    if (index > at) out.push({ kind: 'text', text: source.slice(at, index) });
    if (match[1] !== undefined) out.push({ kind: 'code', text: match[1] });
    else if (match[2] !== undefined) out.push({ kind: 'link', text: match[2], href: safe(match[3]) });
    else out.push({ kind: 'link', text: match[4], href: safe(match[4]) });
    at = index + match[0].length;
  }
  if (at < source.length) out.push({ kind: 'text', text: source.slice(at) });
  return out;
}

function safe(href: string): string {
  return /^(https?:\/\/|\/)/.test(href) ? href : '#';
}

function cells(line: string): string[] {
  return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((cell) => cell.trim());
}

export function parse(markdown: string): Block[] {
  const lines = markdown.split('\n');
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith('```')) {
      const lang = line.slice(3).trim();
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].startsWith('```')) body.push(lines[i++]);
      i += 1;
      blocks.push({ kind: 'code', lang, code: body.join('\n') });
      continue;
    }
    const heading = /^(#{1,6}) (.*)$/.exec(line);
    if (heading) {
      blocks.push({ kind: 'heading', level: heading[1].length, text: inline(heading[2]) });
      i += 1;
      continue;
    }
    if (line.startsWith('|') && i + 1 < lines.length && /^\|[\s:|-]+\|$/.test(lines[i + 1].trim())) {
      const head = cells(line).map(inline);
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && lines[i].startsWith('|')) rows.push(cells(lines[i++]).map(inline));
      blocks.push({ kind: 'table', head, rows });
      continue;
    }
    if (line.trim() === '') {
      i += 1;
      continue;
    }
    const paragraph: string[] = [];
    while (i < lines.length && lines[i].trim() !== '' && !/^(```|#|\|)/.test(lines[i])) paragraph.push(lines[i++]);
    blocks.push({ kind: 'paragraph', text: inline(paragraph.join(' ')) });
  }
  return blocks;
}
