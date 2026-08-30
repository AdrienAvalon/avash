describe("Avash — démarrage", () => {
  it("affiche la barre latérale, le bouton dossier et l'accueil", async () => {
    await expect($(".brand")).toBeExisting();
    await expect($("#new-folder-btn")).toBeExisting();
    await expect($("#manual-btn")).toBeExisting();
    await expect($("#terminal-empty")).toBeDisplayed();
  });
});
