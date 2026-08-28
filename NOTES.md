# Purr — notes de dev

## v0.1 backend (28/08 00:45)
- cargo init fait, dépendances posées (tokio, russh 0.45, russh-sftp, serde_yaml)
- Parser `~/.ssh/config` écrit + testé (multi-alias déplié, wildcards exclus) — tests verts ✅
- Attention : grep des sources à éviter sur les chemins ~/.ssh (policy) — passer par le parser Rust uniquement
- Prochaines étapes backend : connexion russh réelle, list dir SFTP, puis brancher Tauri (webkit2gtk manquant — sudo requis)

## Build
- `source ~/.cargo/env && cargo test` dans /home/avalon/dev/purr
