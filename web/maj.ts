// Mise à jour automatique et version affichée.

import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";
import { askConfirm } from "./dialogues";
import { notify, notifyErreur } from "./notifications";
import { t } from "./i18n";
import { estWindows } from "./vnc-clavier";

/** La mise à jour rendue par `check`, nommée ici plutôt qu'importée : la
 *  garde des permissions (scripts/tests/permissions-utilisees-par-le-front.sh)
 *  lit les imports du greffon comme autant de permissions demandées. Ses
 *  `download` et `install` relèvent d'`updater:default`, déjà accordé. */
type Update = NonNullable<Awaited<ReturnType<typeof checkUpdate>>>;

// ---------- Sessions ouvertes au moment d'installer ----------

/** Ce que la mise à jour doit savoir des onglets, branché par `raccourcis.ts`
 *  (ce module ne dépend pas de la page entière, ses tests non plus). */
export type SessionsOuvertes = {
  /** Sessions SSH, consoles série et bureaux distants non fermés. */
  nombre(): number;
  /** Ferme tous les onglets, comme Ctrl+W sans confirmation. */
  fermerTout(): Promise<void>;
};
let sessions: SessionsOuvertes = { nombre: () => 0, fermerTout: async () => {} };
export function brancherSessionsPourMaj(s: SessionsOuvertes): void {
  sessions = s;
}

/** La mise à jour reportée au prochain lancement : sa version. */
const CLE_AU_LANCEMENT = "avash.maj.au-prochain-lancement";
function lireReport(): string | null {
  try {
    return localStorage.getItem(CLE_AU_LANCEMENT);
  } catch {
    return null;
  }
}
function ecrireReport(version: string | null): void {
  try {
    if (version === null) localStorage.removeItem(CLE_AU_LANCEMENT);
    else localStorage.setItem(CLE_AU_LANCEMENT, version);
  } catch {
    /* stockage refusé : la mise à jour se reproposera au prochain clic */
  }
}

/** Sous Windows, l'installeur ferme Avash : le greffon lance le setup NSIS
 *  puis termine le processus (`std::process::exit`), sans retour à la page.
 *
 *  Audit du 27 novembre 2026 (WI-3), choix du mainteneur : un clic sur
 *  « Installer » coupait sur-le-champ toutes les sessions, transferts SFTP
 *  compris, sans le dire. Si des sessions sont ouvertes, on demande : installer
 *  maintenant (les onglets sont fermés proprement avant, ce qui arrête aussi
 *  les processus de bureau avant que l'installeur ne remplace leur exécutable),
 *  ou au prochain lancement. Rend vrai pour installer tout de suite. */
async function accordAvantFermeture(version: string): Promise<boolean> {
  const n = sessions.nombre();
  if (n === 0) return true;
  const maintenant = await askConfirm(
    t(n > 1 ? "maj-sessions-fermees-pluriel" : "maj-sessions-fermees", { n }) + "\n\n" + t("maj-sessions-question"),
    { danger: true, ok: t("maj-installer-maintenant"), annuler: t("maj-au-prochain-lancement") },
  );
  if (!maintenant) {
    ecrireReport(version);
    notify(t("maj-reportee", { version }), "info");
    return false;
  }
  await sessions.fermerTout();
  return true;
}

/** Télécharge puis installe sous Windows, après accord si des sessions sont
 *  ouvertes. Le téléchargement passe d'abord : un réseau coupé ne ferme rien. */
async function installerSousWindows(update: Update): Promise<void> {
  try {
    await update.download();
  } catch (e) {
    notifyErreur(t("maj-installation-impossible", { e: String(e) }));
    return;
  }
  if (!(await accordAvantFermeture(update.version))) return;
  ecrireReport(null);
  try {
    await update.install(); // ne revient pas : l'installeur prend la main
  } catch (e) {
    notifyErreur(t("maj-installation-impossible", { e: String(e) }));
  }
}

/** Au lancement, la mise à jour reportée s'installe, avant que l'utilisateur
 *  n'ait rouvert ses sessions. Si une session s'est ouverte pendant le
 *  téléchargement, la même question revient. */
export async function installerLaMajReportee(): Promise<void> {
  if (!lireReport()) return;
  const canal = await invoke<string>("canal_de_mise_a_jour").catch(() => "greffon");
  if (canal !== "greffon" || !estWindows()) {
    ecrireReport(null);
    return;
  }
  let update: Update | null;
  try {
    update = await checkUpdate();
  } catch {
    return; // hors ligne : on réessaiera au prochain lancement
  }
  if (!update) {
    ecrireReport(null); // déjà installée, ou retirée
    return;
  }
  notify(t("maj-installation-reportee", { version: update.version }), "info");
  await installerSousWindows(update);
}

// ---------- Mise à jour ----------

let updateBusy = false;
async function checkForUpdates() {
  if (updateBusy) return;
  updateBusy = true;
  const ver = $("app-version");
  const prev = ver.textContent;
  ver.textContent = "…";
  try {
    // Trouvé par l'audit du 7 septembre 2026 (et complété par la relecture) : le
    // greffon updater ne peut pas installer sur plusieurs emballages, car
    // latest.json ne sert que l'AppImage, le setup NSIS et l'app macOS. Sur
    // Flatpak (/app en lecture seule), AUR, Flathub, .deb/.rpm, il retombe sur
    // l'AppImage et échoue après ~100 Mo : c'est le gestionnaire de paquets qui
    // met à jour. Sur l'archive portable Windows (avash-ui.exe brut, non
    // estampillé), il installerait le setup NSIS ailleurs en laissant le dossier
    // portable en arrière : on renvoie télécharger la nouvelle archive. La
    // commande dit qui installe ; « greffon » (défaut) garde la mise à jour
    // intégrée, les autres canaux la court-circuitent avec le bon message.
    const canal = await invoke<string>("canal_de_mise_a_jour").catch(() => "greffon");
    if (canal === "gestionnaire") {
      ver.textContent = prev;
      notify(t("maj-gere-par-paquet"), "info");
      return;
    }
    if (canal === "archive") {
      // Audit du 27 novembre 2026 (WI-2) : ce canal rendait la main avant
      // toute vérification et affirmait toujours « Une nouvelle version est
      // disponible », archive à jour comprise, sans dire laquelle. Le
      // manifeste signé se lit comme ailleurs (le greffon en vérifie la
      // signature) ; rien n'est téléchargé ni installé : l'URL qu'il donne
      // est celle du setup NSIS, qui s'installerait hors du dossier portable.
      const update = await checkUpdate();
      if (!update) {
        ver.textContent = t("maj-a-jour");
        setTimeout(() => (ver.textContent = prev), 1800);
        return;
      }
      ver.textContent = prev;
      notify(t("maj-archive-portable", { version: update.version }), "info");
      return;
    }
    const update = await checkUpdate();
    if (!update) {
      ver.textContent = t("maj-a-jour");
      setTimeout(() => (ver.textContent = prev), 1800);
      return;
    }
    ver.textContent = prev;
    const ok = await askConfirm(
      t("maj-disponible", { version: update.version, actuelle: update.currentVersion }) +
        `\n\n${update.body ?? ""}\n\n` + t("maj-installer-question"),
      { danger: false, ok: t("maj-installer") },
    );
    if (!ok) return;
    if (estWindows()) {
      await installerSousWindows(update);
      return;
    }
    // Trouvé par l'audit du 7 septembre 2026 : un seul catch englobait
    // checkUpdate(), downloadAndInstall() et relaunch(), si bien qu'une signature
    // invalide, un réseau coupé pendant le téléchargement ou un AppImage non
    // inscriptible étaient annoncés comme « Vérification impossible » alors que la
    // vérification avait réussi. On distingue donc chaque étape par un catch
    // propre. `ver` affiche déjà `prev` (posé plus haut), rien à restaurer ici.
    try {
      await update.downloadAndInstall();
    } catch (e) {
      notifyErreur(t("maj-installation-impossible", { e: String(e) }));
      return;
    }
    // La mise à jour est désormais sur le disque : si le redémarrage échoue, le
    // dire explicitement (« prendra effet au prochain lancement »), sinon
    // l'utilisateur relance un cycle de vérification pour une mise à jour déjà
    // appliquée.
    if (await askConfirm(t("maj-installee-redemarrer"), { danger: false, ok: t("maj-redemarrer") })) {
      try {
        await relaunch();
      } catch (e) {
        notifyErreur(t("maj-redemarrage-impossible", { e: String(e) }));
      }
    }
  } catch (e) {
    // Endpoint injoignable / pas encore configuré / hors ligne : on le dit
    // sans dramatiser. Ne reste ici que l'échec de vérification (checkUpdate).
    ver.textContent = prev;
    notifyErreur(t("maj-verification-impossible", { e: String(e) }));
  } finally {
    updateBusy = false;
  }
}
$("app-version").addEventListener("click", checkForUpdates);
void installerLaMajReportee();
