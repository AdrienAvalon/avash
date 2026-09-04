// Connexion directe (sans `~/.ssh/config`) : formulaire SSH ou RDP.

import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";
import { loadHosts, openManualSession } from "./main";
import { openRdp } from "./rdp";
import { t } from "./i18n";

// ---------- Connexion directe (sans ~/.ssh/config) ----------

export type ManualTarget = {
  addr: string;
  port: number | null;
  user: string;
  password: string | null;
  key_path: string | null;
};

const manualModal = () => $("manual-modal");
const manualError = () => $("m-error");

export function manualOpen() {
  manualError().hidden = true;
  manualSyncProto();
  manualModal().classList.add("open");
  ($("m-addr") as HTMLInputElement).focus();
}

/** Le protocole choisi dans le formulaire. */
function protocoleChoisi(): "ssh" | "rdp" | "vnc" {
  const v = (document.querySelector('input[name="proto"]:checked') as HTMLInputElement | null)?.value;
  return v === "rdp" || v === "vnc" ? v : "ssh";
}

/** Adapte le formulaire au protocole : un bureau (RDP ou VNC) n'a ni clé, ni
 *  sauvegarde config ; en VNC l'utilisateur est facultatif. */
function manualSyncProto() {
  const proto = protocoleChoisi();
  const bureau = proto !== "ssh";
  $("m-auth-switch").hidden = bureau;
  $("m-key-row").hidden = bureau || (document.querySelector('input[name="auth"]:checked') as HTMLInputElement | null)?.value !== "key";
  $("m-save-row").hidden = bureau;
  $("m-alias-row").hidden = bureau || !($("m-save") as HTMLInputElement).checked;
  // Mot de passe toujours visible pour un bureau (seule auth), sinon selon le mode.
  if (bureau) $("m-password-row").hidden = false;
  else manualSyncAuthRows();
  $("m-rdp-remember-row").hidden = !bureau;
  $("m-rdp-save-row").hidden = !bureau;
  $("m-rdp-name-row").hidden = !bureau || !($("m-rdp-save") as HTMLInputElement).checked;
  $("m-vnc-hint").hidden = proto !== "vnc";
  ($("m-user") as HTMLInputElement).required = proto !== "vnc";
  ($("m-port") as HTMLInputElement).placeholder = proto === "rdp" ? "3389" : proto === "vnc" ? "5900" : "22";
  ($("m-password") as HTMLInputElement).placeholder = "";
}

function manualClose() {
  manualModal().classList.remove("open");
  ($("manual-form") as HTMLFormElement).reset();
  manualSyncAuthRows();
  manualSyncSaveRow();
  manualSyncProto();
}

/** N'affiche que le champ correspondant au mode d'authentification choisi. */
export function manualSyncSaveRow() {
  $("m-alias-row").hidden = !($("m-save") as HTMLInputElement).checked;
}

function manualSyncAuthRows() {
  const mode = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement | null)?.value;
  $("m-password-row").hidden = mode !== "password";
  $("m-key-row").hidden = mode !== "key";
}

function manualReadForm(): ManualTarget {
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const mode = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement).value;
  const portRaw = val("m-port");
  return {
    addr: val("m-addr"),
    port: portRaw ? Number(portRaw) : null,
    user: val("m-user"),
    password: mode === "password" ? val("m-password") || null : null,
    key_path: mode === "key" ? val("m-key") || null : null,
  };
}

async function manualSubmit(ev: Event) {
  ev.preventDefault();
  const submit = $("m-submit") as HTMLButtonElement;
  const proto = protocoleChoisi();
  if (proto !== "ssh") {
    // Bureau distant : on passe par le sidecar, pas de sauvegarde ~/.ssh.
    const vnc = proto === "vnc";
    const addr = ($("m-addr") as HTMLInputElement).value.trim();
    const user = ($("m-user") as HTMLInputElement).value.trim();
    const password = ($("m-password") as HTMLInputElement).value;
    // L'authentification VNC classique n'a qu'un mot de passe.
    if (!addr || (!user && !vnc)) {
      manualError().textContent = t(vnc ? "cd-adresse-requise" : "cd-adresse-utilisateur-requis");
      manualError().hidden = false;
      return;
    }
    const portRaw = ($("m-port") as HTMLInputElement).value.trim();
    const rport = portRaw ? Number(portRaw) : (vnc ? 5900 : 3389);
    const enregistrer = ($("m-rdp-save") as HTMLInputElement).checked;
    const memoriser = ($("m-rdp-remember") as HTMLInputElement).checked;
    const nomRdp = ($("m-rdp-name") as HTMLInputElement).value.trim();
    // Le volet SSH refuse un alias vide avant d'enregistrer ; le volet RDP
    // acceptait n'importe quoi et posait dans la barre latérale une ligne sans
    // libellé, que même sa suppression ne savait plus nommer.
    if (enregistrer && !nomRdp) {
      manualError().textContent = t("cd-nom-bureau");
      manualError().hidden = false;
      return;
    }
    submit.disabled = true;
    const libelleRdp = submit.textContent;
    submit.textContent = "Connexion…";
    try {
      if (enregistrer) {
        await invoke("rdp_host_save", {
          id: null, name: nomRdp,
          host: addr, port: rport, user, width: 0, height: 0, protocole: proto,
        });
      }
      // « Mémoriser le mot de passe » était imbriqué dans « Enregistrer la
      // connexion » : cochée seule, la case ne faisait rien et le mot de passe
      // était redemandé à la connexion suivante, sans le moindre message.
      if (memoriser && password) {
        await invoke("rdp_password_save", { host: addr, port: rport, user, password, protocole: proto });
      }
      if (enregistrer || memoriser) await loadHosts();
    } catch (e) {
      manualError().textContent = e instanceof Error ? e.message : String(e);
      manualError().hidden = false;
      return;
    } finally {
      submit.disabled = false;
      submit.textContent = libelleRdp;
    }
    manualClose();
    await openRdp({ host: addr, port: rport, user, password, vnc });
    return;
  }
  const target = manualReadForm();
  manualError().hidden = true;
  submit.disabled = true;
  submit.textContent = "Connexion…";
  // Retenu hors du bloc : l'onglet doit porter ce nom-là, pas
  // « utilisateur@adresse ».
  let alias: string | undefined;
  try {
    if (($("m-save") as HTMLInputElement).checked) {
      // Enregistrer AVANT de connecter : si l'ecriture echoue (alias deja
      // pris, fichier illisible), l'utilisateur le voit dans le formulaire
      // plutot que de decouvrir plus tard que rien n'a ete sauve.
      alias = ($("m-alias") as HTMLInputElement).value.trim();
      if (!alias) throw new Error(t("cd-nom-hote"));
      await invoke("host_save", {
        alias,
        addr: target.addr,
        port: target.port,
        user: target.user,
        keyPath: target.key_path,
        proxyJump: null,
        tags: null,
      });
      await loadHosts();
    }
    await openManualSession(target, alias);
    manualClose();
  } catch (e) {
    // Le backend renvoie un message deja redige pour l'utilisateur
    // (cle introuvable, cle d'hote modifiee, identifiants manquants).
    manualError().textContent = e instanceof Error ? e.message : String(e);
    manualError().hidden = false;
  } finally {
    submit.disabled = false;
    submit.textContent = t("se-connecter");
  }
}

// Cablage du formulaire de connexion directe.
$("manual-btn").addEventListener("click", manualOpen);
$("m-cancel").addEventListener("click", manualClose);
$("manual-form").addEventListener("submit", manualSubmit);
$("m-save").addEventListener("change", manualSyncSaveRow);
// Pre-remplir le nom avec l'adresse : c'est presque toujours ce qu'on veut.
$("m-addr").addEventListener("blur", () => {
  const alias = $("m-alias") as HTMLInputElement;
  if (!alias.value.trim()) alias.value = ($("m-addr") as HTMLInputElement).value.trim();
});
document
  .querySelectorAll('input[name="auth"]')
  .forEach((r) => r.addEventListener("change", manualSyncAuthRows));
document.querySelectorAll('input[name="proto"]').forEach((r) => r.addEventListener("change", manualSyncProto));
$("m-rdp-save").addEventListener("change", manualSyncProto);
// Fermeture à Échap seulement (pas au clic dehors : évite de perdre la saisie).
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && manualModal().classList.contains("open")) manualClose();
});
manualSyncAuthRows();
