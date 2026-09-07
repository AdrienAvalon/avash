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
    // Trouvé par l'audit du 7 septembre 2026 : sur Flatpak (/app en lecture
    // seule) et sur un binaire Linux non estampillé par le bundler (AUR,
    // Flathub), le greffon updater retombe sur l'AppImage et échoue à
    // l'installation après ~100 Mo. Là, c'est le gestionnaire de paquets qui
    // met à jour : on le dit au lieu d'appeler checkUpdate().
    if (await invoke<boolean>("emballage_gere_ses_mises_a_jour").catch(() => false)) {
      ver.textContent = prev;
      notify(t("maj-gere-par-paquet"), "info");
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
