# Cask Homebrew pour macOS (Mac à puce Apple). À soumettre au dépôt
# Homebrew/homebrew-cask ; l'empreinte et la version se relèvent depuis le
# SHA256SUMS de la release GitHub. `livecheck` suit les releases pour
# `brew bump-cask-pr`.
cask "avash" do
  version "0.7.2"
  sha256 "87587b47cda36cb87cece573889a98c6fac77b67a085ab424401ff7978574ec4"

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

  zap trash: [
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
