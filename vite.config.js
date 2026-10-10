import { defineConfig } from 'vite';

export default defineConfig(({ command, mode }) => ({
  define: { __TAURI_PACKAGED__: JSON.stringify(mode === 'tauri') },
  server: command === 'serve'
    ? {
        port: 5173,
        strictPort: true
      }
    : undefined
}));
