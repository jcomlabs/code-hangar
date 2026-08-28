import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  build: {
    // Tauri's supported WebView2 runtime loads emitted ES modules directly.
    // Vite's legacy modulepreload shim injects a browser `fetch()` helper even
    // though Code Hangar never needs an HTTP fetch path; omit that shim so the
    // final Local and Connector bundles remain free of the primitive.
    modulePreload: { polyfill: false },
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            {
              // App.tsx is intentionally the orchestration shell. Keep the
              // substantial pure transcript parser in its own local ES module
              // so neither edition's entry chunk drifts over the enforced
              // 500 kB budget as product surfaces grow.
              name: "session-transcript",
              test: /[\\/]src[\\/]session-transcript\.ts$/,
              priority: 20
            },
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
