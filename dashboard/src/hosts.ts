// The host components the page registers, by the `host` name of their slot in
// ui/llm-gateway.ui.yaml: the token-flow canvas, which draws from the whole snapshot (the one row
// of `dashboard.TokenFlow`), and the setup guide at /, which renders the llms.txt the gateway puts
// inside the page.
import { defineComponent, h, markRaw, type Component, type PropType } from 'vue';
import FlowCanvas from './FlowCanvas.vue';
import Index from './Index.vue';
import type { Snapshot } from './extra.ts';
import type { Row } from './essui/types.ts';

const TokenFlow = defineComponent({
  name: 'TokenFlow',
  props: { rows: { type: Array as PropType<Row[]>, default: () => [] } },
  setup(props) {
    return () => h(FlowCanvas, { snap: (props.rows[0] as unknown as Snapshot | undefined) ?? null });
  },
});

export const HOSTS: Record<string, Component> = { TokenFlow: markRaw(TokenFlow), SetupGuide: markRaw(Index) };
