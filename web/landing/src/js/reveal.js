// Scroll reveal: adds .in to each .reveal / .reveal-grp element the first time it enters the viewport, then stops watching it.
// If anything goes wrong the .reveal-on class is removed so every element is shown.
export function initReveal() {
  const root = document.documentElement;
  const show = () => root.classList.remove('reveal-on');
  try {
    const targets = [...document.querySelectorAll('.reveal, .reveal-grp')];
    if (!root.classList.contains('reveal-on')) return;
    if (!('IntersectionObserver' in window)) return show();
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          e.target.classList.add('in');
          io.unobserve(e.target);
        }
      },
      { rootMargin: '0px 0px -8% 0px', threshold: 0.12 },
    );
    targets.forEach((t) => io.observe(t));
    root.classList.add('reveal-ready');
  } catch {
    show();
  }
}
