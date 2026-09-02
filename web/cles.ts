// Clés SSH : lister, générer, déployer.

import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";

// ---------- Clés SSH : lister, générer, déployer ----------

type KeyEntry = {
  name: string;
  path: string;
  public_line: string | null;
  mode: string;
};

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
    empty.textContent = "Aucune clé pour l'instant — crée-en une ci-dessous.";
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
    mode.className = "kmode" + (k.mode === "600" ? "" : " warn");
    mode.textContent = k.mode === "600" ? "600" : `${k.mode} ⚠`;
    mode.title = k.mode === "600" ? "Droits corrects" : "OpenSSH exige 600 sur une clé privée";

    row.append(name, mode);

    if (k.public_line) {
      const copy = document.createElement("button");
      copy.className = "kcopy";
      copy.type = "button";
      copy.textContent = "copier la publique";
      copy.addEventListener("click", async () => {
        await navigator.clipboard.writeText(k.public_line!);
        copy.textContent = "copiée ✓";
        setTimeout(() => (copy.textContent = "copier la publique"), 1500);
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
    keyFeedback(`Clé « ${k.name} » créée dans ${k.path}`, "ok");
    ($("keygen-form") as HTMLFormElement).reset();
    await keysRefresh();
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
  }
}

async function deploySubmit(ev: Event) {
  ev.preventDefault();
  const btn = $("d-submit") as HTMLButtonElement;
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const portRaw = val("d-port");
  btn.disabled = true;
  btn.textContent = "Installation…";
  try {
    const msg = await invoke<string>("key_deploy", {
      addr: val("d-addr"),
      port: portRaw ? Number(portRaw) : null,
      user: val("d-user"),
      password: val("d-password"),
      publicLine: ($("d-key") as HTMLSelectElement).value,
    });
    keyFeedback(msg, "ok");
    // Le mot de passe ne doit pas trainer dans le formulaire une fois servi.
    ($("d-password") as HTMLInputElement).value = "";
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
    btn.textContent = "Installer la clé";
  }
}

$("keys-btn").addEventListener("click", keysOpen);
$("k-close").addEventListener("click", keysClose);
$("keygen-form").addEventListener("submit", keygenSubmit);
$("deploy-form").addEventListener("submit", deploySubmit);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && keysModal().classList.contains("open")) keysClose();
});
