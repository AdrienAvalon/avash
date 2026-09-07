// Imports CSS en side-effect (xterm) : déclarés pour le typage strict de TS 7.
// Vite les gère au bundling ; côté types il suffit de les déclarer.
declare module "*.css";
// Import « brut » d'un fichier en chaîne (Vite `?raw`) : utilisé par les tests
// qui montent le vrai index.html dans jsdom.
declare module "*?raw" {
  const contenu: string;
  export default contenu;
}

// `import.meta.glob(..., { eager, ?raw })` : Vite remplace l'appel par les
// fichiers correspondants chargés en chaîne. `tsconfig` fixe `types: []`, donc
// les types de `vite/client` ne sont pas chargés ; on déclare la seule forme
// qu'on utilise (le test i18n scanne tout le code source d'un coup).
interface ImportMeta {
  glob(
    motif: string,
    options: { query: "?raw"; import: "default"; eager: true },
  ): Record<string, string>;
}
