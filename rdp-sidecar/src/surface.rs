//! Les surfaces du canal graphique, et ce qu'on y fait.
//!
//! Le pipeline graphique ne dessine pas dans l'image affichée : il travaille sur
//! des surfaces qu'il crée, remplit, recopie et met en cache, puis qu'il
//! rattache à une sortie. Ce module tient ces surfaces et les opérations qui les
//! modifient ; le décodage des images vit à côté (`progressif`, codec planaire).
//!
//! Toutes les coordonnées viennent du serveur. Elles sont donc traitées comme
//! des entrées : un rectangle qui déborde est rogné, jamais cru sur parole.

/// Une surface : une image RGBA que le serveur remplit puis affiche.
#[derive(Debug)]
pub struct Surface {
    pub largeur: u16,
    pub hauteur: u16,
    pub pixels: Vec<u8>,
}

/// Rectangle en pixels, bornes hautes exclues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub x: u16,
    pub y: u16,
    pub largeur: u16,
    pub hauteur: u16,
}

impl Zone {
    /// Rogne la zone sur une surface. `None` si rien n'en reste.
    #[must_use]
    pub fn rognee(self, largeur: u16, hauteur: u16) -> Option<Self> {
        if self.x >= largeur || self.y >= hauteur || self.largeur == 0 || self.hauteur == 0 {
            return None;
        }
        Some(Self {
            x: self.x,
            y: self.y,
            largeur: self.largeur.min(largeur - self.x),
            hauteur: self.hauteur.min(hauteur - self.y),
        })
    }

    /// Lit un rectangle « gauche, haut, droite, bas » tel que le porte le
    /// protocole, bornes hautes exclues.
    #[must_use]
    pub fn depuis_bords(o: &[u8]) -> Option<Self> {
        let [l, h, d, b] = [
            u16::from_le_bytes([*o.first()?, *o.get(1)?]),
            u16::from_le_bytes([*o.get(2)?, *o.get(3)?]),
            u16::from_le_bytes([*o.get(4)?, *o.get(5)?]),
            u16::from_le_bytes([*o.get(6)?, *o.get(7)?]),
        ];
        // Un rectangle aux bords inversés n'est pas une zone vide : c'est une
        // zone dont la largeur, calculée par soustraction, déborderait.
        //
        // `then` et non `then_some` : ce dernier construit la valeur AVANT
        // d'examiner la condition, si bien que la soustraction avait lieu de
        // toute façon — panique en débogage, débordement silencieux en release.
        // La garde ne gardait rien.
        (d > l && b > h).then(|| Self {
            x: l,
            y: h,
            largeur: d - l,
            hauteur: b - h,
        })
    }
}

impl Surface {
    #[must_use]
    pub fn nouvelle(largeur: u16, hauteur: u16) -> Self {
        Self {
            largeur,
            hauteur,
            pixels: vec![0; usize::from(largeur) * usize::from(hauteur) * 4],
        }
    }

    fn decalage(&self, x: u16, y: u16) -> usize {
        (usize::from(y) * usize::from(self.largeur) + usize::from(x)) * 4
    }

    /// Recopie des pixels RGBA dans la surface, en rognant sur ses bords.
    pub fn ecrire(&mut self, zone: Zone, pixels: &[u8], pas_source: usize) -> Option<Zone> {
        let z = zone.rognee(self.largeur, self.hauteur)?;
        let largeur = usize::from(z.largeur) * 4;
        for ligne in 0..usize::from(z.hauteur) {
            let src = ligne * pas_source;
            let dst = self.decalage(z.x, z.y) + ligne * usize::from(self.largeur) * 4;
            if src + largeur > pixels.len() || dst + largeur > self.pixels.len() {
                break;
            }
            self.pixels[dst..dst + largeur].copy_from_slice(&pixels[src..src + largeur]);
        }
        Some(z)
    }

    /// Extrait un rectangle en RGBA compact, ligne par ligne.
    #[must_use]
    pub fn extraire(&self, zone: Zone) -> Option<(Zone, Vec<u8>)> {
        let z = zone.rognee(self.largeur, self.hauteur)?;
        let largeur = usize::from(z.largeur) * 4;
        let mut v = Vec::with_capacity(largeur * usize::from(z.hauteur));
        for ligne in 0..usize::from(z.hauteur) {
            let d = self.decalage(z.x, z.y) + ligne * usize::from(self.largeur) * 4;
            v.extend_from_slice(self.pixels.get(d..d + largeur)?);
        }
        Some((z, v))
    }

    /// Peint un rectangle d'une seule couleur (RGBA).
    pub fn remplir(&mut self, zone: Zone, couleur: [u8; 4]) -> Option<Zone> {
        let z = zone.rognee(self.largeur, self.hauteur)?;
        for ligne in 0..usize::from(z.hauteur) {
            let d = self.decalage(z.x, z.y) + ligne * usize::from(self.largeur) * 4;
            for p in self.pixels[d..d + usize::from(z.largeur) * 4]
                .as_chunks_mut::<4>()
                .0
            {
                p.copy_from_slice(&couleur);
            }
        }
        Some(z)
    }
}

/// Le cache de surfaces du serveur : des morceaux d'image qu'il dépose une fois
/// et redemande ensuite par leur emplacement, sans les retransmettre.
///
/// Windows s'en sert massivement — près de six cents messages pour une seule
/// ouverture de session. L'ignorer ne laisse pas un écran incomplet : il laisse
/// un écran où tout ce qui se répète manque.
#[derive(Debug, Default)]
pub struct Cache {
    entrees: std::collections::BTreeMap<u16, (u16, u16, Vec<u8>)>,
}

/// Nombre d'emplacements retenus. La spécification en autorise bien plus, mais
/// un serveur ne doit pas pouvoir faire enfler la mémoire du client à volonté :
/// chaque entrée est une image, et rien ne borne leur taille par ailleurs.
const EMPLACEMENTS_MAX: usize = 4096;

impl Cache {
    pub fn deposer(&mut self, emplacement: u16, largeur: u16, hauteur: u16, pixels: Vec<u8>) {
        if self.entrees.len() >= EMPLACEMENTS_MAX && !self.entrees.contains_key(&emplacement) {
            return;
        }
        self.entrees.insert(emplacement, (largeur, hauteur, pixels));
    }

    #[must_use]
    pub fn lire(&self, emplacement: u16) -> Option<&(u16, u16, Vec<u8>)> {
        self.entrees.get(&emplacement)
    }

    pub fn oublier(&mut self, emplacement: u16) {
        self.entrees.remove(&emplacement);
    }
}

#[cfg(test)]
mod tests {
    use super::{Cache, Surface, Zone, EMPLACEMENTS_MAX};

    #[test]
    fn une_zone_est_rognee_sur_la_surface() {
        let z = Zone {
            x: 90,
            y: 60,
            largeur: 64,
            hauteur: 64,
        };
        assert_eq!(
            z.rognee(100, 70),
            Some(Zone {
                x: 90,
                y: 60,
                largeur: 10,
                hauteur: 10
            })
        );
        assert_eq!(z.rognee(80, 70), None, "hors surface en largeur");
        assert_eq!(z.rognee(100, 50), None, "hors surface en hauteur");
    }

    #[test]
    fn un_rectangle_aux_bords_inverses_est_refuse() {
        // Sans ce contrôle, droite - gauche déborde et produit une largeur
        // énorme : le serveur nous ferait recopier bien au-delà du tampon.
        let inverse = [10, 0, 10, 0, 0, 0, 0, 0];
        assert_eq!(Zone::depuis_bords(&inverse), None);
        let vide = [5, 0, 5, 0, 5, 0, 5, 0];
        assert_eq!(Zone::depuis_bords(&vide), None);
        let bon = [1, 0, 2, 0, 5, 0, 9, 0];
        assert_eq!(
            Zone::depuis_bords(&bon),
            Some(Zone {
                x: 1,
                y: 2,
                largeur: 4,
                hauteur: 7
            })
        );
        assert_eq!(Zone::depuis_bords(&[0, 0]), None, "trop court");
    }

    #[test]
    fn ecrire_puis_extraire_rend_les_memes_pixels() {
        let mut s = Surface::nouvelle(8, 4);
        let zone = Zone {
            x: 2,
            y: 1,
            largeur: 3,
            hauteur: 2,
        };
        let pixels: Vec<u8> = (0..3 * 2 * 4).map(|i| i as u8).collect();
        assert_eq!(s.ecrire(zone, &pixels, 3 * 4), Some(zone));
        let (z, relu) = s.extraire(zone).expect("zone dans la surface");
        assert_eq!(z, zone);
        assert_eq!(relu, pixels);
    }

    #[test]
    fn remplir_ne_deborde_pas_de_la_surface() {
        let mut s = Surface::nouvelle(4, 4);
        let z = s
            .remplir(
                Zone {
                    x: 2,
                    y: 2,
                    largeur: 10,
                    hauteur: 10,
                },
                [1, 2, 3, 4],
            )
            .expect("zone rognée");
        assert_eq!((z.largeur, z.hauteur), (2, 2));
        // Le coin haut-gauche n'a pas été touché.
        assert_eq!(&s.pixels[0..4], &[0, 0, 0, 0]);
        assert_eq!(
            &s.pixels[(2 * 4 + 2) * 4..(2 * 4 + 2) * 4 + 4],
            &[1, 2, 3, 4]
        );
    }

    #[test]
    fn le_cache_est_borne() {
        // Chaque entrée est une image : un serveur ne doit pas pouvoir en
        // déposer indéfiniment.
        let mut c = Cache::default();
        for i in 0..u16::try_from(EMPLACEMENTS_MAX).unwrap() + 100 {
            c.deposer(i, 1, 1, vec![0; 4]);
        }
        assert!(c.lire(0).is_some());
        assert!(
            c.lire(u16::try_from(EMPLACEMENTS_MAX).unwrap() + 50)
                .is_none(),
            "au-delà du plafond, plus rien n'est retenu"
        );
        c.oublier(0);
        assert!(c.lire(0).is_none());
    }
}
