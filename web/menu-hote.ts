// Menu contextuel d'un hôte et modale de modification.

import { invoke } from "@tauri-apps/api/core";
import { type Host } from "./filters";
import { $, state } from "./etat";
import { MODALES_AU_DESSUS, askConfirm } from "./dialogues";
import { closeEditRdp } from "./rdp";
import { closeMoveModal, openMoveModal } from "./dossiers";
import { loadHosts, openSession } from "./main";
import { notify, notifyErreur } from "./notifications";
import { snippetsClose, snippetsModal } from "./snippets";
import { tunnelsClose, tunnelsOpen } from "./tunnels";
import { t } from "./i18n";

// ---------- Menu contextuel d'un hôte ----------

export const MENUS_CONTEXTUELS = ["host-context", "rdp-context", "folder-context", "sftp-context"];

export function closeAllContextMenus() {
  for (const id of MENUS_CONTEXTUELS) $(id).classList.remove("open");
}

/** Rend un menu contextuel utilisable au clavier, après Maj+F10.
 *
 *  Le menu s'ouvrait mais le focus restait sur la ligne : les flèches
 *  continuaient de déplacer la sélection *derrière* le menu resté ouvert,
 *  Entrée relançait la connexion par-dessus, et Échap ne fermait rien — seul un
 *  clic de souris en sortait, ce qui annulait la raison d'être du raccourci.
 *
 *  Le focus revient à l'élément d'où l'on vient, comme pour les modales. */
export function ouvrirMenuAuClavier(menu: HTMLElement, origine: HTMLElement): void {
  const items = [...menu.querySelectorAll<HTMLElement>("[data-act]")].filter((i) => !i.hidden);
  if (items.length === 0) return;
  for (const i of items) i.tabIndex = -1;
  items[0].focus();
  const surTouche = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      fermer();
      return;
    }
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const pas = e.key === "ArrowDown" ? 1 : -1;
      items[(i + pas + items.length) % items.length].focus();
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const choisi = items[i] ?? items[0];
      fermer();
      choisi.click();
    } else if (e.key === "Tab") {
      // Une tabulation sort du menu : on le referme plutôt que de laisser un
      // menu ouvert sans focus dedans.
      fermer();
    }
  };
  function fermer() {
    menu.removeEventListener("keydown", surTouche, true);
    menu.classList.remove("open");
    if (origine.isConnected) origine.focus();
  }
  menu.addEventListener("keydown", surTouche, true);
}
/**
 * Positionne un menu contextuel en le gardant dans la fenêtre.
 *
 * Le menu des hôtes SSH — le plus haut des cinq, sept entrées — posait
 * brutalement les coordonnées du clic : un clic droit sur le dernier hôte d'une
 * liste descendant jusqu'en bas rendait « Supprimer l'hôte » inatteignable.
 * On mesure la taille réelle plutôt que de la supposer.
 */
export function placerMenu(menu: HTMLElement, e: MouseEvent): void {
  menu.style.visibility = "hidden";
  menu.classList.add("open");
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(e.clientX, window.innerWidth - width - 8))}px`;
  menu.style.top = `${Math.max(4, Math.min(e.clientY, window.innerHeight - height - 8))}px`;
  menu.style.visibility = "";
}

export function openHostMenu(h: Host, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("host-context");
  m.dataset.alias = h.alias;
  placerMenu(m, e);
  m.classList.add("open");
}
function hideHostMenu() { $("host-context").classList.remove("open"); }
window.addEventListener("click", hideHostMenu);
window.addEventListener("blur", hideHostMenu);

$("host-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const alias = $("host-context").dataset.alias;
  hideHostMenu();
  if (!alias) return;
  const h = state.hosts.find((x) => x.alias === alias);
  if (!h) return;
  if (act === "connect") {
    void openSession(h);
  } else if (act === "edit") {
    await openEditHost(alias);
  } else if (act === "move") {
    openMoveModal("ssh", alias);
  } else if (act === "tunnels") {
    await tunnelsOpen(alias);
  } else if (act === "delete") {
    const ok = await askConfirm(
      t("hote-supprimer-question", { alias }),
    );
    if (!ok) return;
    try {
      await invoke("host_delete", { alias });
      await loadHosts();
    } catch (err) {
      notifyErreur(t("suppression-impossible", { e: String(err) }));
    }
  } else if (act === "forget") {
    // L'action ne disait rien du tout : ni succès, ni échec. On ne pouvait pas
    // savoir si le trousseau avait été purgé ou si le clic avait manqué sa
    // cible — et un échec réel (trousseau verrouillé, D-Bus absent) laissait
    // croire le secret effacé alors qu'il était toujours là.
    try {
      await invoke("password_forget", {
        addr: h.hostname ?? h.alias,
        port: h.port,
        user: h.user ?? null,
      });
      notify(t("hote-mdp-oublie", { alias: h.alias }), "succes");
    } catch (err) {
      notifyErreur(t("hote-mdp-non-oublie", { e: String(err) }));
    }
  }
});


// ---------- Modifier un hôte ----------

async function openEditHost(alias: string) {
  const err = $("e-error");
  err.hidden = true;
  try {
    const h = await invoke<Host>("host_get", { alias });
    ($("e-old") as HTMLInputElement).value = h.alias;
    ($("e-alias") as HTMLInputElement).value = h.alias;
    ($("e-addr") as HTMLInputElement).value = h.hostname ?? "";
    ($("e-port") as HTMLInputElement).value = h.port ? String(h.port) : "";
    ($("e-user") as HTMLInputElement).value = h.user ?? "";
    ($("e-key") as HTMLInputElement).value = h.identity_file ?? "";
    ($("e-jump") as HTMLInputElement).value = h.proxy_jump ?? "";
    ($("e-tags") as HTMLInputElement).value = h.tags.join(", ");
    ($("edit-form") as HTMLFormElement).dataset.folder = h.folder ?? "";
    $("edit-modal").classList.add("open");
    setTimeout(() => ($("e-alias") as HTMLInputElement).focus(), 30);
  } catch (e) {
    notifyErreur(t("hote-chargement-impossible", { e: String(e) }));
  }
}

function closeEditHost() { $("edit-modal").classList.remove("open"); }

$("e-cancel").addEventListener("click", closeEditHost);
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // Une boîte ouverte PAR-DESSUS (confirmation, saisie, mot de passe) a déjà
  // traité la touche : sans cette garde, renoncer à une suppression fermait
  // aussi la fenêtre Tunnels ou Snippets d'où l'on venait — l'utilisateur qui
  // annule était puni deux fois.
  if (MODALES_AU_DESSUS.some((id) => $(id).classList.contains("open"))) return;
  // Un seul `return` par branche : elles s'enchaînaient toutes.
  if ($("edit-modal").classList.contains("open")) { closeEditHost(); return; }
  if ($("rdp-edit-modal").classList.contains("open")) { closeEditRdp(); return; }
  if ($("move-modal").classList.contains("open")) { closeMoveModal(); return; }
  if ($("tunnels-modal").classList.contains("open")) { tunnelsClose(); return; }
  if (snippetsModal().classList.contains("open")) { snippetsClose(); return; }
});

$("edit-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const portRaw = val("e-port");
  const err = $("e-error");
  const submit = $("e-submit") as HTMLButtonElement;
  submit.disabled = true;
  try {
    await invoke("host_update", {
      oldAlias: val("e-old"),
      alias: val("e-alias"),
      addr: val("e-addr"),
      port: portRaw ? Number(portRaw) : null,
      user: val("e-user") || null,
      keyPath: val("e-key") || null,
      proxyJump: val("e-jump") || null,
      tags: val("e-tags") || null,
      folder: ($("edit-form") as HTMLFormElement).dataset.folder ?? null,
    });
    closeEditHost();
    await loadHosts();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});
