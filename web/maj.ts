// Mise à jour automatique et version affichée.

import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { $ } from "./etat";
import { askConfirm } from "./dialogues";
import { notifyErreur } from "./notifications";

// ---------- Mise à jour ----------

let updateBusy = false;
async function checkForUpdates() {
  if (updateBusy) return;
  updateBusy = true;
  const ver = $("app-version");
  const prev = ver.textContent;
  ver.textContent = "…";
  try {
    const update = await checkUpdate();
    if (!update) {
      ver.textContent = "à jour";
      setTimeout(() => (ver.textContent = prev), 1800);
      return;
    }
    ver.textContent = prev;
    const ok = await askConfirm(
      `Version ${update.version} disponible (actuelle : ${update.currentVersion}).\n\n` +
        `${update.body ?? ""}\n\nTélécharger et installer maintenant ?`,
      { danger: false, ok: "Installer" },
    );
    if (!ok) return;
    await update.downloadAndInstall();
    if (await askConfirm("Mise à jour installée. Redémarrer Avash maintenant ?", { danger: false, ok: "Redémarrer" })) await relaunch();
  } catch (e) {
    // Endpoint injoignable / pas encore configuré / hors ligne : on le dit
    // sans dramatiser.
    ver.textContent = prev;
    notifyErreur(`Vérification des mises à jour impossible : ${e}`);
  } finally {
    updateBusy = false;
  }
}
$("app-version").addEventListener("click", checkForUpdates);
