(() => {
  const copy = document.getElementById('copy-link');
  const status = document.getElementById('copy-status');
  copy.addEventListener('click', async () => {
    const url = document.querySelector('link[rel="canonical"]').href;
    try {
      await navigator.clipboard.writeText(url);
      status.textContent = 'Brief link copied.';
    } catch {
      status.textContent = `Copy this link: ${url}`;
    }
  });
  document.getElementById('print-brief').addEventListener('click', () => window.print());
})();
