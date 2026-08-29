//! Avash — moteur SSH v0.3 : sessions exécution + PTY interactif complet.
//! v0.1 : connect/auth/exec. v0.2 : `request_pty`.
//! v0.3 : write stdin réel, `window_change` (resize), `known_hosts` strict.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Marqueur place en tete du message quand seule l'absence de mot de passe
/// explique l'echec. L'interface le reconnait pour proposer une saisie.
/// Nom de l'utilisateur courant, avec repli.
///
/// `whoami::username()` est faillible depuis la version 2 (compte systeme
/// illisible, environnement minimal). Un client SSH a toujours besoin d'un
/// nom : on retombe sur $USER, puis sur "user", plutot que d'echouer.
#[must_use]
pub fn current_username() -> String {
    whoami::username()
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "user".to_string())
}

pub const PASSWORD_REQUIRED: &str = "[AVASH_PASSWORD_REQUIRED]";

pub struct ClientAuth {
    pub user: String,
    /// Chemin de la clé privée (OpenSSH). Support agent à venir.
    pub key_path: Option<PathBuf>,
    pub password: Option<String>,
}

/// Raison d'un refus de cle d'hote, partagee entre le handler et l'appelant.
///
/// `check_server_key` ne peut renvoyer qu'un `russh::Error` sans message : sans
/// ce canal, l'utilisateur ne verrait qu'un "Unknown key" opaque alors que
/// c'est l'avertissement le plus important de l'application.
type HostKeyVerdict = Arc<std::sync::Mutex<Option<String>>>;

/// Compteurs d'un tunnel, partages entre le relais et l'interface.
///
/// Atomiques : mis a jour depuis des taches concurrentes, lus a tout moment
/// par un instantane sans verrou.
#[derive(Debug, Default)]
pub struct ForwardCounters {
    /// Connexions en cours.
    pub active: AtomicU64,
    /// Connexions relayees depuis l'ouverture.
    pub total: AtomicU64,
    /// Octets client -> destination.
    pub bytes_up: AtomicU64,
    /// Octets destination -> client.
    pub bytes_down: AtomicU64,
}

impl ForwardCounters {
    /// Relaie `a` <-> `b` jusqu'a la fin de la connexion, en comptant.
    ///
    /// Ecrit a la main plutot que via `copy_bidirectional` : celui-ci ne rend
    /// les volumes qu'a la fin, alors qu'un tunnel porte souvent des
    /// connexions longues (base de donnees, VNC) que l'interface doit voir
    /// progresser.
    pub async fn relay<A, B>(&self, a: &mut A, b: &mut B)
    where
        A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized,
        B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        self.active.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        let mut buf_a = vec![0u8; 32 * 1024];
        let mut buf_b = vec![0u8; 32 * 1024];
        let (mut a_done, mut b_done) = (false, false);
        while !(a_done && b_done) {
            tokio::select! {
                r = a.read(&mut buf_a), if !a_done => match r {
                    Ok(0) | Err(_) => {
                        a_done = true;
                        // Fin de lecture d'un cote : on signale la fin
                        // d'ecriture a l'autre, qui repondra par son EOF.
                        let _ = b.shutdown().await;
                    }
                    Ok(n) => {
                        if b.write_all(&buf_a[..n]).await.is_err() { break; }
                        self.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
                    }
                },
                r = b.read(&mut buf_b), if !b_done => match r {
                    Ok(0) | Err(_) => {
                        b_done = true;
                        let _ = a.shutdown().await;
                    }
                    Ok(n) => {
                        if a.write_all(&buf_b[..n]).await.is_err() { break; }
                        self.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
                    }
                },
            }
        }
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Destination locale d'une redirection distante (`ssh -R`).
struct RemoteTarget {
    host: String,
    port: u16,
    counters: Arc<ForwardCounters>,
}

/// Redirections distantes actives sur une session : port ecoute par le
/// serveur -> destination locale a joindre pour chaque connexion.
///
/// Le serveur ouvre lui-meme un canal `forwarded-tcpip` a chaque client qui
/// frappe a ce port ; le handler consulte cette table pour savoir vers quelle
/// adresse locale relayer.
type RemoteForwards = Arc<std::sync::Mutex<HashMap<u32, Arc<RemoteTarget>>>>;

/// Handler d'auth + vérification `known_hosts`.
struct AvashAuth {
    host: String,
    port: u16,
    verdict: HostKeyVerdict,
    forwards: RemoteForwards,
}

impl russh::client::Handler for AvashAuth {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Un certificat se valide contre une autorite, pas contre known_hosts.
        // On n'en gere pas encore : refuser vaut mieux qu'accepter a l'aveugle.
        let server_public_key = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key,
            russh::keys::PublicKeyOrCertificate::Certificate(_) => {
                *self.verdict.lock().unwrap() = Some(
                    "Ce serveur présente un certificat SSH. Avash ne sait pas \
                     encore les valider et refuse la connexion."
                        .into(),
                );
                return Err(russh::Error::UnknownKey);
            }
        };
        // TOFU (Trust On First Use), avec la distinction que fait OpenSSH :
        // hôte inconnu  -> on apprend la clé (premier contact) ;
        // clé CHANGÉE   -> on refuse, sans jamais réapprendre en silence.
        match russh::keys::check_known_hosts(&self.host, self.port, server_public_key) {
            // Hôte connu, clé identique.
            Ok(true) => Ok(true),

            // Hôte inconnu : premier contact, on mémorise.
            Ok(false) => {
                russh::keys::known_hosts::learn_known_hosts(
                    &self.host,
                    self.port,
                    server_public_key,
                )
                .map_err(|_| russh::Error::UnknownKey)?;
                Ok(true)
            }

            // La clé d'hôte a changé : réinstallation du serveur, ou interception.
            // Dans le doute on refuse — c'est à l'utilisateur de trancher.
            Err(russh::keys::Error::KeyChanged { line }) => {
                *self.verdict.lock().unwrap() = Some(format!(
                    "LA CLÉ D'HÔTE A CHANGÉ pour {}:{}.\n\n\
                     Soit le serveur a été réinstallé, soit quelqu'un intercepte \
                     la connexion. Avash refuse de se connecter.\n\n\
                     Si tu es certain que le serveur a changé légitimement, \
                     supprime la ligne {} de ~/.ssh/known_hosts.",
                    self.host, self.port, line
                ));
                Err(russh::Error::UnknownKey)
            }

            // known_hosts illisible ou autre erreur : on refuse aussi.
            Err(e) => {
                *self.verdict.lock().unwrap() =
                    Some(format!("Vérification de la clé d'hôte impossible : {e}"));
                Err(russh::Error::UnknownKey)
            }
        }
    }

    /// Le serveur relaie une connexion recue sur un port redirige (`-R`).
    ///
    /// On ne relaie que vers une destination enregistree par `remote_forward` :
    /// un serveur malveillant ne peut pas nous faire ouvrir une connexion
    /// locale arbitraire.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let dest = self.forwards.lock().unwrap().get(&connected_port).cloned();
        let Some(target) = dest else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        // On accepte AVANT de joindre la destination : le serveur attend une
        // reponse rapide, et un echec local ferme simplement le canal.
        reply.accept().await;
        tokio::spawn(async move {
            let Ok(mut local) =
                tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await
            else {
                let _ = channel.close().await;
                return;
            };
            let mut remote = channel.into_stream();
            target.counters.relay(&mut remote, &mut local).await;
        });
        Ok(())
    }
}

pub struct AvashSession {
    session: russh::client::Handle<AvashAuth>,
    forwards: RemoteForwards,
}

impl AvashSession {
    pub async fn connect(host: &str, port: u16, auth: &ClientAuth) -> Result<Self> {
        let config = Arc::new(russh::client::Config {
            // Un tunnel ou un terminal inactif derriere un NAT se fait couper
            // en silence apres quelques minutes. Le keepalive detecte la
            // coupure (3 pings sans reponse) au lieu de laisser une session
            // zombie que l'utilisateur croit vivante.
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let verdict: HostKeyVerdict = Arc::new(std::sync::Mutex::new(None));
        let forwards: RemoteForwards = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let handler = AvashAuth {
            host: host.to_string(),
            port,
            verdict: verdict.clone(),
            forwards: forwards.clone(),
        };
        let mut session = match russh::client::connect(config, (host, port), handler).await {
            Ok(s) => s,
            Err(e) => {
                // Un refus de cle d'hote porte un message explicite : on le
                // remonte tel quel plutot que le "Unknown key" de russh.
                if let Some(reason) = verdict.lock().unwrap().take() {
                    return Err(anyhow!(reason));
                }
                return Err(e).with_context(|| format!("Connexion SSH à {host}:{port}"));
            }
        };
        Self::authenticate(&mut session, auth).await?;
        Ok(Self { session, forwards })
    }

    async fn authenticate(
        session: &mut russh::client::Handle<AvashAuth>,
        auth: &ClientAuth,
    ) -> Result<()> {
        if let Some(key_path) = &auth.key_path {
            let key = russh::keys::load_secret_key(key_path, None)
                .with_context(|| format!("Chargement clé {}", key_path.display()))?;
            // `None` en hash : ignore hors RSA, et pour RSA russh retombe sur
            // l'algorithme historique. Nos cles generees sont ed25519.
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
            if session
                .authenticate_publickey(&auth.user, key)
                .await?
                .success()
            {
                return Ok(());
            }
        }
        if let Some(password) = &auth.password {
            if session
                .authenticate_password(&auth.user, password)
                .await?
                .success()
            {
                return Ok(());
            }
        }
        // Marqueur reconnu par l'interface : elle demande alors le mot de
        // passe et retente, plutot que d'afficher un echec sans recours.
        if auth.password.is_none() {
            return Err(anyhow!(
                "{PASSWORD_REQUIRED} Aucune méthode d'authentification n'a abouti pour « {} ». \
                 Un mot de passe est nécessaire.",
                auth.user
            ));
        }
        Err(anyhow!("Authentification échouée pour {}", auth.user))
    }

    /// Exécution one-shot : stdout + exit code.
    pub async fn run(&mut self, command: &str) -> Result<(String, u32)> {
        let mut channel = self.session.channel_open_session().await?;
        channel.exec(false, command).await?;
        let mut stdout = String::new();
        let mut exit_code = 0u32;

        // ⚠️ Ne PAS casser sur Eof : dans le protocole SSH, `exit-status`
        // arrive APRES l'EOF. Casser sur Eof renverrait donc toujours 0, quel
        // que soit le vrai code de sortie — verifie contre un vrai serveur.
        // On laisse la boucle courir jusqu'a la fermeture du canal (wait()
        // rend None), ou jusqu'a Close.
        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { ref data } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExtendedData { ref data, .. } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status,
                russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok((stdout, exit_code))
    }

    /// Ouvre un canal PTY interactif.
    /// Le front écrit dans `in_tx` (touches clavier), lit `out_rx` (flux terminal),
    /// et appelle `resize_tx` pour `window_change`.
    pub async fn open_pty(&mut self, cols: u32, rows: u32, term: &str) -> Result<PtyChannel> {
        let channel = self.session.channel_open_session().await?;
        channel
            .request_pty(false, term, cols, rows, 0, 0, &[])
            .await
            .context("Demande PTY refusée")?;
        channel.request_shell(false).await?;

        // Le canal est partagé entre le pump de sortie et le writer d'entrée :
        // russh::Channel est clonable via son sender interne ? Non — mais on
        // dédouble : le stream into_stream() possède le canal. On garde donc
        // une approche à deux moitiés : wait() pour la sortie, data() pour l'entrée
        // n'est pas possible sur le même objet possédé. Solution russh idiomatique :
        // cloner le canal (russh 0.45 : Channel implémente Clone ? Non).
        // → On utilise une seule tâche qui possède le canal et traite via select.
        let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(256);
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(256);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(16);

        let mut pump_channel = channel;
        let pump = tokio::spawn(async move {
            // Le resize est optionnel : sa fermeture ne doit pas tuer la session,
            // mais son bras select! doit etre desactive (voir plus bas).
            let mut resize_closed = false;
            loop {
                tokio::select! {
                    // Sortie du serveur → front
                    msg = pump_channel.wait() => {
                        match msg {
                            Some(russh::ChannelMsg::Data { ref data }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => break,
                            Some(_) => {}
                        }
                    }
                    // Clavier du front → stdin serveur
                    maybe = in_rx.recv() => {
                        match maybe {
                            Some(bytes) => {
                                let mut cursor = std::io::Cursor::new(bytes);
                                if pump_channel.data(&mut cursor).await.is_err() { break; }
                            }
                            None => break,
                        }
                    }
                    // Resize du front → window_change.
                    // La garde `if !resize_closed` est indispensable : un canal
                    // ferme rend Ready(None) immediatement et sans fin, et ce
                    // bras ferait tourner la boucle a vide a 100 % de CPU.
                    // On desactive donc le bras plutot que d'ignorer le None.
                    maybe = resize_rx.recv(), if !resize_closed => {
                        match maybe {
                            Some((c, r)) => {
                                if pump_channel.window_change(c, r, 0, 0).await.is_err() { break; }
                            }
                            None => resize_closed = true,
                        }
                    }
                }
            }
            let _ = pump_channel.close().await;
        });

        Ok(PtyChannel {
            out_rx,
            in_tx,
            resize_tx,
            _pump: pump,
        })
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.session
            .disconnect(russh::Disconnect::ByApplication, "au revoir", "")
            .await?;
        Ok(())
    }

    // ---------- Redirections de port ----------

    /// Ouvre un canal `direct-tcpip` : le serveur joint `host:port` pour nous
    /// (`ssh -L` et `-D`). Le canal se manipule comme un flux TCP.
    pub async fn open_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        originator: std::net::SocketAddr,
    ) -> Result<russh::Channel<russh::client::Msg>> {
        self.session
            .channel_open_direct_tcpip(
                host,
                u32::from(port),
                originator.ip().to_string(),
                u32::from(originator.port()),
            )
            .await
            .with_context(|| format!("Le serveur n'a pas pu joindre {host}:{port}"))
    }

    /// Demande au serveur d'ecouter sur `bind_addr:port` (`ssh -R`) et de
    /// relayer chaque connexion vers `local_host:local_port` chez nous.
    ///
    /// Rend le port effectivement ecoute (le serveur en choisit un si `port`
    /// vaut 0).
    pub async fn remote_forward(
        &self,
        bind_addr: &str,
        port: u16,
        local_host: &str,
        local_port: u16,
        counters: Arc<ForwardCounters>,
    ) -> Result<u16> {
        // Enregistre avant la demande : une connexion peut arriver des que le
        // serveur ecoute, avant meme que sa reponse nous parvienne.
        let dest = Arc::new(RemoteTarget {
            host: local_host.to_string(),
            port: local_port,
            counters,
        });
        self.forwards
            .lock()
            .unwrap()
            .insert(u32::from(port), dest.clone());
        let bound = self
            .session
            .tcpip_forward(bind_addr, u32::from(port))
            .await
            .with_context(|| format!("Le serveur refuse d'écouter sur {bind_addr}:{port}"))?;
        // Port 0 : le serveur a choisi, on retient le vrai numero.
        let bound = if port == 0 && bound != 0 {
            let mut f = self.forwards.lock().unwrap();
            f.remove(&0);
            f.insert(bound, dest);
            u16::try_from(bound).unwrap_or(0)
        } else {
            port
        };
        Ok(bound)
    }

    /// La connexion au serveur est-elle tombee (reseau, keepalive, kill) ?
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    /// Annule une redirection distante.
    pub async fn cancel_remote_forward(&self, bind_addr: &str, port: u16) -> Result<()> {
        self.forwards.lock().unwrap().remove(&u32::from(port));
        self.session
            .cancel_tcpip_forward(bind_addr, u32::from(port))
            .await?;
        Ok(())
    }

    /// Ouvre un canal dédié au sous-système SFTP (canal indépendant, session intacte).
    pub async fn open_sftp_channel(&mut self) -> Result<russh::Channel<russh::client::Msg>> {
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(channel)
    }
}

/// Canal PTY exposé au front : sortie terminal + entrée clavier + resize.
pub struct PtyChannel {
    pub out_rx: mpsc::Receiver<Vec<u8>>,
    pub in_tx: mpsc::Sender<Vec<u8>>,
    pub resize_tx: mpsc::Sender<(u32, u32)>,
    _pump: tokio::task::JoinHandle<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_username_ne_rend_jamais_vide() {
        // Un client SSH a toujours besoin d'un nom : le repli garantit une
        // valeur non vide meme sans compte systeme lisible.
        assert!(!current_username().is_empty());
    }
}
