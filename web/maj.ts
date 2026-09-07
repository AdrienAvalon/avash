// Mise à jour automatique et version affichée.

import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";
import { askConfirm } from "./dialogues";
import { notify, notifyErreur } from "./notifications";
import { t } from "./i18n";

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
      ver.textContent = prev;
      notify(t("maj-archive-portable"), "info");
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
    await update.downloadAndInstall();
    if (await askConfirm(t("maj-installee-redemarrer"), { danger: false, ok: t("maj-redemarrer") })) await relaunch();
  } catch (e) {
    // Endpoint injoignable / pas encore configuré / hors ligne : on le dit
    // sans dramatiser.
    ver.textContent = prev;
    notifyErreur(t("maj-verification-impossible", { e: String(e) }));
  } finally {
    updateBusy = false;
  }
}
$("app-version").addEventListener("click", checkForUpdates);
