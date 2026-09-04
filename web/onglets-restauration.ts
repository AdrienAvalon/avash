// Mémoire des onglets : la liste suit chaque ouverture et fermeture
// (`onglets_memoriser`, dans le répertoire de configuration), et l'écran
// d'accueil propose de rouvrir ce qui l'était la dernière fois. Proposé,
// jamais imposé : un onglet SSH rouvert peut demander un mot de passe.

import { invoke } from "@tauri-apps/api/core";
import { $, state } from "./etat";
import { t } from "./i18n";
import { rdpSessions } from "./rdp";
import { orderedTabs } from "./raccourcis";
import { listeAMemoriser, nombreARestaurer, type OngletMemorise, type OngletOuvert } from "./onglets-memoire";

/** Écrit la mémoire ; une erreur ici n'a pas à gêner l'utilisateur. */
async function memoriser(liste: OngletMemorise[]): Promise<void> {
  await invoke("onglets_memoriser", { onglets: liste }).catch(() => {});
}

async function memorises(): Promise<OngletMemorise[]> {
  return invoke<OngletMemorise[]>("onglets_memorises").catch(() => []);
}

/** À chaque ouverture ou fermeture d'onglet : la mémoire suit l'état réel. */
export function majMemoireOnglets(): void {
  const ouverts: OngletOuvert[] = orderedTabs().map((o) =>
    o.kind === "ssh"
      ? { kind: "ssh", alias: state.sessions.get(o.id)?.alias ?? "" }
      : { kind: "rdp", hostId: rdpSessions.get(o.id)?.hostId },
  );
  void memoriser(listeAMemoriser(
    ouverts,
    new Set(state.hosts.map((h) => h.alias)),
    new Set(state.rdpHosts.map((h) => h.id)),
  ));
}

/**
 * Au lancement, une fois les hôtes chargés : s'il y a quelque chose à rouvrir,
 * l'accueil le propose. `rouvrir` ouvre un onglet ; les entrées dont l'hôte a
 * disparu depuis sont passées. « Ignorer » efface la mémoire.
 */
export async function proposerRestauration(rouvrir: (o: OngletMemorise) => Promise<void>): Promise<void> {
  const bandeau = $("restaurer");
  const liste = await memorises();
  const aliasConnus = new Set(state.hosts.map((h) => h.alias));
  const bureauxConnus = new Set(state.rdpHosts.map((h) => h.id));
  const n = nombreARestaurer(liste, aliasConnus, bureauxConnus);
  // Un onglet déjà ouvert entre-temps (double-clic pendant le chargement) :
  // la proposition n'a plus de sens, la mémoire suit déjà ce qui est ouvert.
  if (n === 0 || orderedTabs().length > 0) {
    bandeau.hidden = true;
    return;
  }
  $("restaurer-texte").textContent = t("restaurer-texte", { n });
  bandeau.hidden = false;
  const fermer = () => { bandeau.hidden = true; };
  $("restaurer-ok").onclick = () => {
    fermer();
    void (async () => {
      for (const o of liste) {
        const existe = o.kind === "ssh" ? aliasConnus.has(o.alias) : bureauxConnus.has(o.host_id);
        if (existe) await rouvrir(o);
      }
    })();
  };
  $("restaurer-non").onclick = () => {
    fermer();
    void memoriser([]);
  };
}
