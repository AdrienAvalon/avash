// Mise à jour automatique et version affichée.

import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { $ } from "./etat";
import { askConfirm } from "./dialogues";
import { notifyErreur } from "./notifications";
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
