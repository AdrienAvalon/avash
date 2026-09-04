// Mesures de performance du front, prises sur la vraie application par le
// même harnais que la suite bout en bout (bac à sable, sshd local).
// `scripts/mesures-front.sh` lance ce fichier sous Xvfb ; les chiffres vont
// dans docs/feuille-de-route.md (axe 3), jamais dans une assertion : une
// mesure sous charge ne doit pas rougir la chaîne.
//
// Ce n'est pas un test : ce fichier n'est pas dans la liste des
// spécifications de la suite.
import { config as base } from "./wdio.conf.js";

export const config = {
  ...base,
  specs: ["./mesures/latence.spec.js"],
};
