import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const outDir = process.env.VITE_OUT_DIR ?? '../static/dashboard'

export default defineConfig({
  base: './',
  plugins: [vue()],
  build: {
    emptyOutDir: true,
    outDir,
    assetsDir: 'generated/assets',
    sourcemap: false,
  },
})
