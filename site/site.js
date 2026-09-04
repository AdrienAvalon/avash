// Vitrine d'avash : onglets d'installation et liste des fichiers de la
// dernière version, lue sur l'API GitHub. Sans réseau ou sans script, la page
// garde ses liens vers la page des versions : rien ici n'est indispensable.
(function () {
  "use strict";

  const DEPOT = "AdrienAvalon/avash";
  const langue = document.documentElement.lang === "en" ? "en" : "fr";
  const T = langue === "en"
    ? { taille: (o) => `${(o / 1048576).toFixed(1)} MB`, version: (v) => `Latest version: ${v}` }
    : { taille: (o) => `${(o / 1048576).toFixed(1).replace(".", ",")} Mo`, version: (v) => `Dernière version : ${v}` };

  // Onglets Linux / Windows / macOS : le premier ouvert est celui du système
  // du visiteur, quand on le devine.
  const onglets = Array.from(document.querySelectorAll(".onglets button"));
  const panneaux = Array.from(document.querySelectorAll(".panneau"));
  function ouvrir(nom) {
    onglets.forEach((b) => b.setAttribute("aria-selected", String(b.dataset.onglet === nom)));
    panneaux.forEach((p) => {
      if (p.dataset.panneau === nom) p.setAttribute("data-actif", "");
      else p.removeAttribute("data-actif");
    });
  }
  onglets.forEach((b) => b.addEventListener("click", () => ouvrir(b.dataset.onglet)));
  const ua = navigator.userAgent;
  ouvrir(/Windows/.test(ua) ? "windows" : /Mac OS X|Macintosh/.test(ua) ? "macos" : "linux");

  // Fichiers de la dernière version, rangés par système d'après leur nom.
  const familles = {
    linux: [/\.AppImage$/, /\.deb$/, /\.rpm$/],
    windows: [/-setup\.exe$/, /windows-x64\.zip$/],
    macos: [/\.dmg$/],
  };
  fetch(`https://api.github.com/repos/${DEPOT}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } })
    .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
    .then((rel) => {
      document.querySelectorAll("[data-version]").forEach((el) => { el.textContent = T.version(rel.tag_name); });
      for (const [nom, motifs] of Object.entries(familles)) {
        const ul = document.querySelector(`.fichiers[data-famille="${nom}"]`);
        if (!ul) continue;
        const items = rel.assets.filter((a) => motifs.some((m) => m.test(a.name)));
        if (items.length === 0) continue;
        ul.textContent = "";
        for (const a of items) {
          const li = document.createElement("li");
          const lien = document.createElement("a");
          lien.href = a.browser_download_url;
          lien.textContent = a.name;
          const taille = document.createElement("span");
          taille.textContent = T.taille(a.size);
          lien.appendChild(taille);
          li.appendChild(lien);
          ul.appendChild(li);
        }
      }
    })
    .catch(() => { /* la page garde ses liens statiques */ });
})();
