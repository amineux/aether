(() => {
  const nav = document.querySelector('.nav');
  const toggle = document.querySelector('.nav-toggle');
  const links = [...document.querySelectorAll('.nav-links a')];
  function closeMenu() { nav.classList.remove('open'); toggle.setAttribute('aria-expanded', 'false'); }
  toggle.addEventListener('click', () => {
    toggle.setAttribute('aria-expanded', String(nav.classList.toggle('open')));
  });
  links.forEach(link => link.addEventListener('click', closeMenu));
  document.addEventListener('keydown', event => { if (event.key === 'Escape') closeMenu(); });
  const sections = links.map(link => document.querySelector(link.getAttribute('href')));
  function updateActive() {
    let active = -1;
    sections.forEach((section, index) => { if (section.getBoundingClientRect().top <= 150) active = index; });
    links.forEach((link, index) => {
      link.classList.toggle('is-active', index === active);
      if (index === active) link.setAttribute('aria-current', 'location');
      else link.removeAttribute('aria-current');
    });
  }
  window.addEventListener('scroll', updateActive, { passive: true });
  updateActive();
  const legacy = { sell: 'business', partners: 'contact', how: 'evidence', kv: 'evidence', pitch: 'investors' };
  function resolveLegacy() {
    const target = legacy[location.hash.slice(1)];
    if (target) document.getElementById(target).scrollIntoView();
  }
  window.addEventListener('hashchange', resolveLegacy);
  resolveLegacy();
})();
