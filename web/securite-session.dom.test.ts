// @vitest-environment jsdom
// Barre d'état de sécurité d'une session (refonte de novembre 2026). Elle ne
// doit rien affirmer que le cœur n'ait établi, distinguer le premier contact
// d'un hôte connu, et ne jamais interpréter comme HTML ce qui vient de
// l'échange avec le serveur.
import { describe, it, expect, vi, beforeAll } from "vitest";

vi.mock("./main", () => ({ renderHosts: vi.fn() }));

import { rendreBarreSecurite, type SecuriteSession } from "./securite-session";
import type { Session } from "./etat";
import { setLangue } from "./i18n";

// La langue par défaut suit celle du navigateur simulé : la fixer rend les
// libellés attendus indépendants du poste.
beforeAll(() => setLangue("fr"));

const etabli: SecuriteSession = {
  cle_hote: "connue",
  algorithme_cle_hote: "ssh-ed25519",
  empreinte_cle_hote: "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG",
  echange_de_cles: "curve25519-sha256",
  chiffrement: "chacha20-poly1305@openssh.com",
  integrite: null,
  authentification: "cle",
};

function session(partiel: Partial<Session>): Session {
  return { id: 1, alias: "web-1", etat: "live", closed: false, ...partiel } as Session;
}

function barre(): HTMLElement {
  const el = document.createElement("div");
  el.hidden = true;
  return el;
}

describe("barre d'état de sécurité", () => {
  it("reste_masquee_sans_session_ssh", () => {
    const b = barre();
    rendreBarreSecurite(b, null);
    expect(b.hidden).toBe(true);
    rendreBarreSecurite(b, session({ serie: true, securite: etabli }));
    expect(b.hidden).toBe(true);
    expect(b.childElementCount).toBe(0);
  });

  it("montre_ce_que_la_poignee_de_main_a_etabli", () => {
    const b = barre();
    rendreBarreSecurite(b, session({ securite: etabli }));
    expect(b.hidden).toBe(false);
    expect(b.classList.contains("premier-contact")).toBe(false);
    expect(b.querySelector(".bs-cle")!.textContent).toBe("Clé d'hôte connue");
    const texte = b.textContent!;
    expect(texte).toContain("ssh-ed25519 SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG");
    expect(texte).toContain("curve25519-sha256");
    expect(texte).toContain("chacha20-poly1305@openssh.com");
    expect(b.querySelector(".bs-auth")!.textContent).toBe("Authentifié par clé");
  });

  it("signale_le_premier_contact_au_lieu_de_le_confondre_avec_un_hote_connu", () => {
    const b = barre();
    rendreBarreSecurite(b, session({ securite: { ...etabli, cle_hote: "premier-contact" } }));
    expect(b.classList.contains("premier-contact")).toBe(true);
    expect(b.querySelector(".bs-cle")!.textContent).toContain("Premier contact");
    // Revenir sur un hôte connu doit lever l'alerte : la classe ne colle pas.
    rendreBarreSecurite(b, session({ securite: etabli }));
    expect(b.classList.contains("premier-contact")).toBe(false);
  });

  it("n_affiche_un_code_d_integrite_que_s_il_est_employe", () => {
    const b = barre();
    rendreBarreSecurite(b, session({ securite: etabli }));
    expect(b.textContent).not.toContain("hmac");
    rendreBarreSecurite(
      b,
      session({ securite: { ...etabli, chiffrement: "aes256-ctr", integrite: "hmac-sha2-256-etm@openssh.com" } }),
    );
    expect(b.textContent).toContain("hmac-sha2-256-etm@openssh.com");
  });

  it("n_affirme_rien_avant_que_la_session_soit_etablie", () => {
    const b = barre();
    rendreBarreSecurite(b, session({ etat: "connecting", securite: etabli }));
    expect(b.querySelector(".bs-cle")).toBeNull();
    expect(b.textContent).toBe("Connexion en cours…");
    rendreBarreSecurite(b, session({ etat: "closed", closed: true, securite: etabli }));
    expect(b.querySelector(".bs-cle")).toBeNull();
    // Vivante mais sans état relevé (le cœur n'a pas répondu) : pas d'invention.
    rendreBarreSecurite(b, session({ securite: null }));
    expect(b.querySelector(".bs-cle")).toBeNull();
  });

  it("n_interprete_pas_en_html_ce_qui_vient_du_serveur", () => {
    const b = barre();
    rendreBarreSecurite(b, session({ securite: { ...etabli, chiffrement: '<img src=x onerror="document.title=1">' } }));
    expect(b.querySelector("img")).toBeNull();
    expect(b.textContent).toContain("<img src=x");
  });
});
