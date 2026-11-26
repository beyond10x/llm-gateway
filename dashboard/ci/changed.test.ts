// Both sides of the path list that decides whether the Gate workflow's dashboard job runs its
// checks on a pull request.
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SCRIPT = fileURLToPath(new URL('./changed.sh', import.meta.url));

function byArgs(...paths: string[]): string {
  return execFileSync('sh', [SCRIPT, ...paths], { encoding: 'utf8' }).trim();
}

function byStdin(paths: string[]): string {
  return execFileSync('sh', [SCRIPT], { encoding: 'utf8', input: paths.join('\n') }).trim();
}

describe('dashboard/ci/changed.sh', () => {
  it('says yes for a dashboard source, the built page or the workflow', () => {
    expect(byArgs('dashboard/src/App.vue')).toBe('yes');
    expect(byArgs('crates/llm-gateway/src/dashboard.html')).toBe('yes');
    expect(byArgs('.github/workflows/gate.yml')).toBe('yes');
    expect(byStdin(['README.md', 'dashboard/package.json', 'docs/gateway.md'])).toBe('yes');
  });

  it('says no for a planning-only or prose-only change', () => {
    expect(byArgs('.engineering/planning/story/x.md', 'CHANGELOG.md')).toBe('no');
    expect(byStdin(['.engineering/planning/story/x.md', 'README.md', 'AGENTS.md'])).toBe('no');
  });

  it('says no for a change elsewhere in the gateway crate', () => {
    expect(byArgs('crates/llm-gateway/src/dashboard.rs', 'crates/llm-gateway/src/dashboard.html.bak')).toBe('no');
    expect(byArgs('website/docs/dashboard/index.md', '.github/workflows/pages.yml')).toBe('no');
  });

  it('says no for no changed path at all', () => {
    expect(byStdin([])).toBe('no');
  });
});
