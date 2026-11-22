import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Configurazione consigliata da Tauri: porta fissa, nessun clear dello schermo.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: { target: "es2021", sourcemap: false },
});
