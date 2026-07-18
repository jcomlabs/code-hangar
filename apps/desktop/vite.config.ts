import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            {
              name: "vendor",
              test: /node_modules[\\/]/,
              priority: 10
            }
          ]
        }
      }
    }
  },
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: false
  },
  test: {
    environment: "node",
    globals: true
  }
});
