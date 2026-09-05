// Premier module évalué du paquet : pose un repère de temps avant que
// xterm.js et le reste ne s'exécutent. Importé en tête de main.ts, il est
// placé par le bundler avant tout autre module ; la différence avec le repère
// posé après les imports de main.ts dit ce que coûte l'évaluation des modules
// (xterm.js pour l'essentiel), celle avec domInteractive ce que coûtent la
// lecture et la compilation du paquet. Lu par e2e/mesures/latence.spec.js.
performance.mark("avash:modules-debut");
