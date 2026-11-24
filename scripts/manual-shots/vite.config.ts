import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({ root: ".", plugins: [react()], server: { port: 1430, strictPort: true }, build: { target: "esnext" }, esbuild: { target: "esnext" }, optimizeDeps: { esbuildOptions: { target: "esnext" } } });
