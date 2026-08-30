import { defineConfig } from "vite";

export default defineConfig({
  root: ".",
  build: {
    outDir: "dist",
    // WebKitGTK 2.52 (Tauri) gère l'ESNext : pas de transpilation legacy
    // inutile → bundle plus petit et JS plus rapide à parser/exécuter.
    target: "esnext",
    // Le gain de taille au build ne vaut pas le coût de recalcul gzip à chaque
    // compilation (app locale, pas servie sur réseau).
    reportCompressedSize: false,
  },
  server: { port: 5173, strictPort: true },
});
