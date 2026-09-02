//! Dossiers de rangement de l'arbre unifié SSH + RDP.

/// Liste des dossiers connus (registre ; les dossiers dérivés des hôtes sont
/// ajoutés côté front).
#[tauri::command]
pub fn folders_list() -> Result<Vec<String>, String> {
    avash::folders::list().map_err(|e| format!("{e:#}"))
}

/// Crée un dossier (et ses ancêtres).
#[tauri::command]
pub fn folder_create(path: String) -> Result<Vec<String>, String> {
    avash::folders::create(&path).map_err(|e| format!("{e:#}"))
}

/// Supprime un dossier : ses hôtes (et ceux des sous-dossiers) reviennent à la
/// racine, puis le dossier et ses descendants sont retirés du registre.
#[tauri::command]
pub fn folder_delete(path: String) -> Result<Vec<String>, String> {
    avash::folders::delete_core(
        &avash::ssh_config_path(),
        &avash::rdphost::hosts_path(),
        &avash::folders::folders_path(),
        &path,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Renomme un dossier et remappe ses hôtes (et sous-dossiers).
#[tauri::command]
pub fn folder_rename(from: String, to: String) -> Result<Vec<String>, String> {
    avash::folders::rename_core(
        &avash::ssh_config_path(),
        &avash::rdphost::hosts_path(),
        &avash::folders::folders_path(),
        &from,
        &to,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Range un hôte SSH dans un dossier (déplacement). Le dossier cible est
/// enregistré s'il est nouveau.
#[tauri::command]
pub fn host_set_folder(alias: String, folder: String) -> Result<(), String> {
    let norm = avash::folders::normalize(&folder);
    avash::set_host_folder(alias.trim(), &norm).map_err(|e| format!("{e:#}"))?;
    if !norm.is_empty() {
        avash::folders::create(&norm).map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}

/// État des verrous clavier du poste : bit 1 = numérique, 2 = majuscules,
/// 4 = défilement. `None` quand le système ne sait pas le dire.
///
/// Le navigateur ne révèle ces verrous que sur un événement clavier. Or une
/// session RDP s'ouvre le plus souvent à la souris : sans interrogation du
/// système, le bureau distant démarrerait avec ses propres verrous, et le pavé
/// numérique paraîtrait éteint alors qu'il est allumé côté utilisateur.
#[tauri::command]
#[must_use]
pub fn keyboard_locks() -> Option<u8> {
    lock_bits()
}

#[cfg(target_os = "linux")]
fn lock_bits() -> Option<u8> {
    lock_bits_from_leds(std::path::Path::new("/sys/class/leds"))
}

/// Lit l'état des verrous dans une arborescence de diodes à la façon du noyau.
///
/// Tous les claviers n'exposent pas de diode (claviers virtuels, machines sans
/// témoin lumineux) : sans aucune diode reconnue on rend `None`, pour ne pas
/// imposer un état inventé au bureau distant.
#[cfg(target_os = "linux")]
fn lock_bits_from_leds(racine: &std::path::Path) -> Option<u8> {
    let mut bits = 0u8;
    let mut connu = false;
    for e in std::fs::read_dir(racine).ok()?.flatten() {
        let nom = e.file_name().to_string_lossy().to_lowercase();
        let bit = if nom.ends_with("::numlock") {
            1
        } else if nom.ends_with("::capslock") {
            2
        } else if nom.ends_with("::scrolllock") {
            4
        } else {
            continue;
        };
        if let Ok(v) = std::fs::read_to_string(e.path().join("brightness")) {
            connu = true;
            if v.trim() != "0" {
                bits |= bit;
            }
        }
    }
    connu.then_some(bits)
}

#[cfg(windows)]
fn lock_bits() -> Option<u8> {
    // Bit de poids faible de GetKeyState : état de bascule de la touche.
    // `#[link]` explicite : ne pas dépendre du hasard qu'une autre caisse du
    // graphe lie déjà user32 — sinon la panne serait une erreur d'édition de
    // liens à la release, pas une erreur de compilation.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyState(virtual_key: i32) -> i16;
    }
    const VK_CAPITAL: i32 = 0x14;
    const VK_NUMLOCK: i32 = 0x90;
    const VK_SCROLL: i32 = 0x91;
    let actif = |vk: i32| unsafe { GetKeyState(vk) } & 1 != 0;
    Some(
        u8::from(actif(VK_NUMLOCK))
            | (u8::from(actif(VK_CAPITAL)) << 1)
            | (u8::from(actif(VK_SCROLL)) << 2),
    )
}

#[cfg(not(any(target_os = "linux", windows)))]
fn lock_bits() -> Option<u8> {
    None // macOS : pas d'interface simple, on s'en remet aux événements clavier.
}

#[cfg(all(test, target_os = "linux"))]
mod tests_verrous {
    use super::lock_bits_from_leds;
    use std::path::{Path, PathBuf};

    /// Arborescence de diodes jetable, à la façon de /sys/class/leds.
    /// Le projet n'a pas de dépendance de test pour les répertoires temporaires :
    /// on suit la convention des autres tests et on nettoie à la libération.
    struct Leds(PathBuf);
    impl Leds {
        fn new(entrees: &[(&str, &str)]) -> Self {
            let base = std::env::temp_dir().join(format!(
                "avash-leds-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            for (nom, valeur) in entrees {
                let p = base.join(nom);
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join("brightness"), valeur).unwrap();
            }
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Leds {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn aucune_diode_reconnue_ne_rend_rien() {
        // Mieux vaut ne rien affirmer que d'éteindre le pavé numérique du distant.
        let d = Leds::new(&[("input0::compose", "1")]);
        assert_eq!(lock_bits_from_leds(d.path()), None);
    }

    #[test]
    fn repertoire_absent_ne_rend_rien() {
        assert_eq!(lock_bits_from_leds(Path::new("/n/existe/pas")), None);
    }

    #[test]
    fn diodes_eteintes_donnent_zero() {
        let d = Leds::new(&[("input3::numlock", "0"), ("input3::capslock", "0")]);
        assert_eq!(lock_bits_from_leds(d.path()), Some(0));
    }

    #[test]
    fn chaque_verrou_a_son_bit() {
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::numlock", "1")]).path()),
            Some(1)
        );
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::capslock", "1")]).path()),
            Some(2)
        );
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::scrolllock", "1")]).path()),
            Some(4)
        );
    }

    #[test]
    fn les_verrous_se_combinent_et_plusieurs_claviers_s_agregent() {
        // Deux claviers branchés : l'état allumé de l'un suffit.
        let d = Leds::new(&[
            ("input3::numlock", "0"),
            ("input9::numlock", "1"),
            ("input9::scrolllock", "1"),
        ]);
        assert_eq!(lock_bits_from_leds(d.path()), Some(1 | 4));
    }
}
