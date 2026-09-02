// Verrous clavier (Num, Maj, Défilement) : lecture des diodes côté Rust et des événements.

import { invoke } from "@tauri-apps/api/core";
import { choisirVerrous } from "./filters";

// ---------- Verrous clavier (Num / Maj / Défilement) ----------
// Un bureau RDP démarre avec ses propres verrous : si le pavé numérique est
// allumé sur le poste mais éteint dans la session distante, l'utilisateur doit
// appuyer sur Verr.Num pour les réaligner. On suit donc l'état local en
// permanence, pour pouvoir l'imposer au distant dès la connexion.
//
// Le navigateur ne révèle cet état que sur un événement clavier : on l'écoute
// dans toute l'application (en capture), pas seulement sur le canvas RDP — ainsi
// l'état est le plus souvent déjà connu au moment où une session s'ouvre.
let verrousDesEvenements: number | null = null;

/** Bits attendus par le message [10] : 1 = numérique, 2 = majuscules, 4 = défilement. */
function readLocks(e: KeyboardEvent): number {
  return (
    (e.getModifierState("NumLock") ? 1 : 0) |
    (e.getModifierState("CapsLock") ? 2 : 0) |
    (e.getModifierState("ScrollLock") ? 4 : 0)
  );
}
for (const type of ["keydown", "keyup"] as const) {
  window.addEventListener(type, (e) => { verrousDesEvenements = readLocks(e); }, true);
}

/**
 * État des verrous à transmettre au bureau distant, ou `null` si inconnu.
 *
 * Le système est interrogé en premier : une session s'ouvre le plus souvent à
 * la souris, sans qu'aucune touche n'ait été frappée. Les événements clavier ne
 * sont qu'un secours, jamais prioritaires — voir `choisirVerrous`.
 */
export async function currentLocks(): Promise<number | null> {
  const duSysteme = await invoke<number | null>("keyboard_locks").catch(() => null);
  return choisirVerrous(duSysteme ?? null, verrousDesEvenements);
}
