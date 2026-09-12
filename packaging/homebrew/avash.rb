# Cask Homebrew pour macOS (Mac à puce Apple). À soumettre au dépôt
# Homebrew/homebrew-cask ; l'empreinte et la version se relèvent depuis le
# SHA256SUMS de la release GitHub. `livecheck` suit les releases pour
# `brew bump-cask-pr`.
cask "avash" do
  version "0.12.2"
  sha256 "c079da672d784e04d234e3103adf767d4b2ce2edf4845a927cb2d1b14c90fea1"

  url "https://github.com/AdrienAvalon/avash/releases/download/v#{version}/Avash_#{version}_aarch64.dmg",
      verified: "github.com/AdrienAvalon/avash/"
  name "Avash"
  desc "Native SSH and RDP connection manager"
  homepage "https://github.com/AdrienAvalon/avash"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on arch: :arm64
  depends_on macos: ">= :ventura"

  app "Avash.app"

  # Trouvé par l'audit du 7 septembre 2026 : les trois répertoires
  # `dev.avash.app` ne couvrent que l'état de la webview Tauri. Le cœur range
  # le sien (bureaux RDP, tunnels, snippets, empreintes TOFU, enregistrements)
  # sous `config_dir()/avash`, soit `~/Library/Application Support/avash` sur
  # macOS ; sans cette ligne, `brew uninstall --zap` le laissait sur le disque.
  zap trash: [
    "~/Library/Application Support/avash",
    "~/Library/Application Support/dev.avash.app",
    "~/Library/Caches/dev.avash.app",
    "~/Library/WebKit/dev.avash.app",
  ]

  caveats <<~EOS
    Avash n'est pas notarisé par Apple : au premier lancement, faites un clic
    droit sur l'application puis « Ouvrir », une fois. / Avash is not notarised:
    on first launch, right-click the app and choose "Open", once.
  EOS
end
