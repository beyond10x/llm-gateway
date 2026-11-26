// Runs once per Playwright run, before any test: drops old measurement runs from ux/.metrics/.
import { pruneRuns, runId } from './metrics-dir.ts';

export default function globalSetup(): void {
  pruneRuns(8, runId());
}
