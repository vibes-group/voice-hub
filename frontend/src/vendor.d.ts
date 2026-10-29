// Side-effect CSS imports (resolved by Vite at build time).
declare module '*.css';

// Vite build-time env (only the flag we use).
interface ImportMeta {
  readonly env: { readonly PROD: boolean };
}

// Vite asset URL imports (hashed, under /assets/).
declare module '*?url' {
  const url: string;
  export default url;
}
declare module '*?worker&url' {
  const url: string;
  export default url;
}
