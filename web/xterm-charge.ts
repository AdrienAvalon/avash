// xterm.js et ses extensions ne se chargent qu'au premier terminal.
//
// Ils pèsent la moitié du paquet (331 Ko pour xterm, 113 pour le rendu WebGL,
// 32 pour la recherche, 15 pour la sérialisation), et l'accueil n'en a pas
// besoin : mesuré le 05/09/2026 par e2e/mesures, la lecture et la compilation
// du paquet faisaient 283 ms des 303 ms d'exécution des modules au démarrage,
// l'évaluation 13 ms et l'initialisation d'avash 7 ms. En les sortant du
// paquet principal, le terminal se charge à part, à l'oisiveté juste après
// l'accueil, ou au premier onglet s'il vient avant.
type Xterm = {
  Terminal: typeof import("@xterm/xterm").Terminal;
  FitAddon: typeof import("@xterm/addon-fit").FitAddon;
  WebglAddon: typeof import("@xterm/addon-webgl").WebglAddon;
  SearchAddon: typeof import("@xterm/addon-search").SearchAddon;
  SerializeAddon: typeof import("@xterm/addon-serialize").SerializeAddon;
  WebLinksAddon: typeof import("@xterm/addon-web-links").WebLinksAddon;
};

let promesse: Promise<Xterm> | null = null;

/** Le module xterm.js et ses extensions, chargés une fois pour toutes. */
export function chargerXterm(): Promise<Xterm> {
  promesse ??= Promise.all([
    import("@xterm/xterm"),
    import("@xterm/addon-fit"),
    import("@xterm/addon-webgl"),
    import("@xterm/addon-search"),
    import("@xterm/addon-serialize"),
    import("@xterm/addon-web-links"),
  ]).then(([xterm, fit, webgl, search, serialize, liens]) => ({
    Terminal: xterm.Terminal,
    FitAddon: fit.FitAddon,
    WebglAddon: webgl.WebglAddon,
    SearchAddon: search.SearchAddon,
    SerializeAddon: serialize.SerializeAddon,
    WebLinksAddon: liens.WebLinksAddon,
  }));
  return promesse;
}
