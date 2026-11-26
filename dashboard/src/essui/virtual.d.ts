// The modules the Vite plugin in vite.config.ts builds from ui/llm-gateway.ui.yaml.
declare module 'virtual:ess-ui/document' {
  const doc: import('./types.ts').Document;
  export default doc;
}

declare module 'virtual:ess-ui/tokens.css' {}
