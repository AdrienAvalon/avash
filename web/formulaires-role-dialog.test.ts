// Trouvé par l'audit du 7 septembre 2026 : le correctif de #manual-modal
// (commentaire de #manual-form, index.html) avait déjà établi que role="dialog"
// n'est pas admis sur un <form> (rôle implicite « form », règle axe-core
// aria-allowed-role). Sept autres modales portaient pourtant encore
// <form class="modal" … role="dialog"> : sftp-copier, pass, send, ask, edit,
// rdp-edit, move. L'audit axe E2E n'ouvrant que la vue principale et
// #manual-modal, ces formulaires masqués passaient sous le radar et un lecteur
// d'écran recevait un rôle contradictoire (form annoncé comme dialog).
// Ce test lit le balisage brut de index.html et verrouille l'invariant : aucun
// <form> ne porte role="dialog" ; chaque modale-formulaire est enveloppée d'un
// <div class="modal" role="dialog">, et la règle display:contents couvre les
// huit formulaires enveloppés.
//
// On analyse le texte brut de index.html (comme menus-contextuels-roles-aria et
// verifier-maj-atteignable-clavier) : on gèle le balisage source, pas un DOM
// reconstruit par jsdom.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";

// Les sept formulaires qui portaient <form … role="dialog"> plus #manual-form,
// déjà corrigé : tous doivent vivre sous un <div class="modal" role="dialog">
// avec un <form> nu (pas de rôle) mis à display:contents.
const FORMS_MODALES = [
  "manual-form",
  "sftp-copier-form",
  "pass-form",
  "send-form",
  "ask-form",
  "edit-form",
  "rdp-edit-form",
  "move-form",
];

describe("Aucun <form> ne porte role=\"dialog\"", () => {
  it("aucune balise <form> ne déclare role=\"dialog\"", () => {
    // axe-core (aria-allowed-role) : role="dialog" interdit sur un <form>.
    const fautifs = indexHtml.match(/<form\b[^>]*\brole="dialog"[^>]*>/g) ?? [];
    expect(fautifs, fautifs.join("\n")).toEqual([]);
  });

  it("chaque formulaire de modale a un id nu (sans role ni class=modal)", () => {
    for (const id of FORMS_MODALES) {
      const re = new RegExp(`<form\\b[^>]*\\bid="${id}"[^>]*>`);
      const balise = indexHtml.match(re)?.[0];
      expect(balise, `formulaire ${id} introuvable`).toBeTruthy();
      expect(balise, balise).not.toMatch(/\brole=/);
      expect(balise, balise).not.toMatch(/\bclass="modal/);
    }
  });

  it("la règle display:contents couvre les huit formulaires enveloppés", () => {
    // display:contents efface la boîte du <form> pour que .modal garde sa mise
    // en page en flex ; sans elle, un formulaire enveloppé casserait la modale.
    // On retire d'abord les commentaires CSS : celui de #manual-form contient
    // des « / * … * / » sans accolade que le sélecteur avalerait sinon.
    const sansCommentaires = indexHtml.replace(/\/\*[\s\S]*?\*\//g, "");
    const re = /([^{}]*)\{\s*display:\s*contents;\s*\}/g;
    const couverts = new Set<string>();
    for (const [, selecteurs] of sansCommentaires.matchAll(re)) {
      for (const s of selecteurs.split(",")) {
        const m = s.trim().match(/^#([\w-]+)$/);
        if (m) couverts.add(m[1]);
      }
    }
    for (const id of FORMS_MODALES) {
      expect(couverts.has(id), `${id} absent de la règle display:contents`).toBe(true);
    }
  });
});
