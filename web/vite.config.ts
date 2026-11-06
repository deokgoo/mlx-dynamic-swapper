import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  base: '/dashboard/',
  server: {
    port: 3000,
    proxy: {
      '/v1': 'http://127.0.0.1:1234',
      '/status': 'http://127.0.0.1:1234',
      '/health': 'http://127.0.0.1:1234',
      '/props': 'http://127.0.0.1:1234',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
