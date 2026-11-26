// Prints the ten slowest tests and the run's wall time after every run.
import type { FullResult, Reporter, TestCase, TestResult } from '@playwright/test/reporter';

export default class SlowestReporter implements Reporter {
  private readonly times: { title: string; ms: number }[] = [];
  private started = Date.now();

  onBegin(): void {
    this.started = Date.now();
  }

  onTestEnd(test: TestCase, result: TestResult): void {
    this.times.push({ title: `[${test.parent.project()?.name ?? ''}] ${test.title}`, ms: result.duration });
  }

  onEnd(result: FullResult): void {
    const slowest = [...this.times].sort((a, b) => b.ms - a.ms).slice(0, 10);
    const lines = slowest.map((t) => `  ${(t.ms / 1000).toFixed(2).padStart(6)} s  ${t.title}`);
    console.log(`\nSlowest tests:\n${lines.join('\n')}`);
    console.log(`Wall time: ${((Date.now() - this.started) / 1000).toFixed(1)} s (${result.status})`);
  }

  printsToStdio(): boolean {
    return false;
  }
}
