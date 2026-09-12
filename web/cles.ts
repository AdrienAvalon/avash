// Clés SSH : lister, générer, déployer.

import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";
import { t } from "./i18n";

// ---------- Clés SSH : lister, générer, déployer ----------

type KeyEntry = {
  name: string;
  path: string;
  public_line: string | null;
  // null là où le système n'a pas de bits de permission (Windows : ACL).
  mode: string | null;
};

/** Ce que la liste dit des droits d'une clé privée.
 *
 *  Audit du 12 septembre 2026 (couverture.md, section 5, point 2) : décision
 *  extraite pour être testée. OpenSSH refuse une clé privée dont le groupe ou
 *  les autres ont un droit quelconque (`st_mode & 077`, « UNPROTECTED PRIVATE
 *  KEY FILE ») ; « 400 » ou « 700 » lui conviennent. L'ancienne règle (tout ce
 *  qui n'est pas « 600 ») criait à tort sur une clé en lecture seule. `null` :
 *  Windows, droits portés par une ACL (le cœur garde ce `null` exprès). Un mode
 *  illisible avertit plutôt que de se taire. */
export function droitsCle(mode: string | null): "acl" | "correct" | "trop-ouverts" {
  if (mode === null) return "acl";
  if (!/^[0-7]{1,4}$/.test(mode)) return "trop-ouverts";
  return (parseInt(mode, 8) & 0o077) === 0 ? "correct" : "trop-ouverts";
}

const keysModal = () => $("keys-modal");
const keyError = () => $("k-error");
const keyOk = () => $("k-ok");

function keyFeedback(msg: string, kind: "ok" | "error") {
  const el = kind === "ok" ? keyOk() : keyError();
  const other = kind === "ok" ? keyError() : keyOk();
  el.textContent = msg;
  el.hidden = false;
  other.hidden = true;
}

function keyFeedbackClear() {
  keyOk().hidden = true;
  keyError().hidden = true;
}

async function keysRefresh() {
  const list = $("key-list");
  const select = $("d-key") as HTMLSelectElement;
  list.innerHTML = "";
  select.innerHTML = "";
  let keys: KeyEntry[];
  try {
    keys = await invoke<KeyEntry[]>("keys_list");
  } catch (e) {
    keyFeedback(String(e), "error");
    return;
  }
  if (keys.length === 0) {
    const empty = document.createElement("div");
    empty.className = "key-empty";
    empty.textContent = t("cles-aucune");
    list.appendChild(empty);
  }
  for (const k of keys) {
    const row = document.createElement("div");
    row.className = "key-row";

    const name = document.createElement("span");
    name.className = "kname";
    name.textContent = k.name;

    // Des droits trop ouverts font refuser la cle par OpenSSH : on le signale
    // plutot que de laisser l'utilisateur devant un echec incomprehensible.
    const mode = document.createElement("span");
    const droits = droitsCle(k.mode);
    if (droits === "acl") {
      // Windows n'a pas de bits de permission : les droits passent par une ACL
      // (posee par icacls). Afficher « - ⚠ OpenSSH exige 600 » accusait des
      // droits que le systeme n'a pas ; on montre une etiquette neutre, sans
      // avertissement (audit du 7 septembre 2026).
      mode.className = "kmode";
      mode.textContent = t("cles-droits-acl");
      mode.title = t("cles-droits-acl-detail");
    } else {
      const correct = droits === "correct";
      mode.className = "kmode" + (correct ? "" : " warn");
      mode.textContent = correct ? (k.mode ?? "") : `${k.mode} ⚠`;
      mode.title = correct ? t("cles-droits-corrects") : t("cles-droits-600");
    }

    row.append(name, mode);

    if (k.public_line) {
      const copy = document.createElement("button");
      copy.className = "kcopy";
      copy.type = "button";
      copy.textContent = t("cles-copier-publique");
      // Sous WebKitGTK, writeText peut rejeter (permission, contexte non
      // securise) : sans branche d'echec, la promesse partait en rejet non
      // gere, le libelle restait « copier la publique » et l'utilisateur
      // collait l'ancien contenu du presse-papiers dans authorized_keys sans
      // rien voir (audit du 7 septembre 2026). Style promesse plutot qu'async
      // pour tenir les deux branches sans await orphelin.
      copy.addEventListener("click", () => {
        navigator.clipboard.writeText(k.public_line!).then(
          () => {
            copy.textContent = t("cles-copiee");
            setTimeout(() => (copy.textContent = t("cles-copier-publique")), 1500);
          },
          () => keyFeedback(t("cles-copie-impossible"), "error"),
        );
      });
      row.appendChild(copy);

      const opt = document.createElement("option");
      opt.value = k.public_line;
      opt.textContent = k.name;
      select.appendChild(opt);
    }
    list.appendChild(row);
  }
  // Sans clé, le déploiement n'a pas d'objet.
  ($("deploy-block") as HTMLDetailsElement).hidden = keys.length === 0;
}

export async function keysOpen() {
  keyFeedbackClear();
  keysModal().classList.add("open");
  await keysRefresh();
}

function keysClose() {
  keysModal().classList.remove("open");
}

async function keygenSubmit(ev: Event) {
  ev.preventDefault();
  const btn = $("k-gen-submit") as HTMLButtonElement;
  const name = ($("k-name") as HTMLInputElement).value.trim();
  const comment = ($("k-comment") as HTMLInputElement).value.trim();
  btn.disabled = true;
  try {
    const k = await invoke<KeyEntry>("key_generate", { name, comment: comment || null });
    keyFeedback(t("cles-creee", { nom: k.name, chemin: k.path }), "ok");
    ($("keygen-form") as HTMLFormElement).reset();
    await keysRefresh();
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
  }
}

export async function deploySubmit(ev: Event) {
  ev.preventDefault();
  const btn = $("d-submit") as HTMLButtonElement;
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const portRaw = val("d-port");
  btn.disabled = true;
  btn.textContent = t("cles-installation-en-cours");
  try {
    const msg = await invoke<string>("key_deploy", {
      addr: val("d-addr"),
      port: portRaw ? Number(portRaw) : null,
      user: val("d-user"),
      // Le mot de passe est lu brut (pas de trim) : une espace de tête ou de
      // fin appartient au secret et le serveur le refuserait rogné (audit du
      // 7 septembre 2026). Les autres champs restent rognés à dessein.
      password: ($("d-password") as HTMLInputElement).value,
      publicLine: ($("d-key") as HTMLSelectElement).value,
    });
    keyFeedback(msg, "ok");
    // Le mot de passe ne doit pas trainer dans le formulaire une fois servi.
    ($("d-password") as HTMLInputElement).value = "";
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
    btn.textContent = t("installer-la-cle");
  }
}

$("keys-btn").addEventListener("click", keysOpen);
$("k-close").addEventListener("click", keysClose);
$("keygen-form").addEventListener("submit", keygenSubmit);
$("deploy-form").addEventListener("submit", deploySubmit);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && keysModal().classList.contains("open")) keysClose();
});
