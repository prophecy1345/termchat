// "copy" buttons next to install commands.
for (const button of document.querySelectorAll<HTMLButtonElement>("[data-copy]")) {
  const label = button.textContent;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(button.dataset.copy ?? "");
      button.textContent = button.dataset.done ?? label;
      setTimeout(() => (button.textContent = label), 1500);
    } catch {
      // No clipboard (insecure context): the command is on screen to select.
    }
  });
}
