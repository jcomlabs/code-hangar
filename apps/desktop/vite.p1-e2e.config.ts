import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  build: {
    outDir: "dist-p1-e2e",
    emptyOutDir: true,
    modulePreload: { polyfill: false },
    rolldownOptions: {
      input: "p1-e2e.html"
    }
  }
});
