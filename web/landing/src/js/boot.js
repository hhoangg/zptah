// Runs before first paint (a blocking script in <head>; a file of its own because the CSP has no inline scripts).
// 1. Marks the page as scripted so the CSS can lay out the demo for it.
// 2. An installed app that still starts at `/` (installed before the manifest start_url became `/app`; iOS never refreshes
//    it) is the app, not the marketing page: send it to /app, which restores the last place or goes to /login.
document.documentElement.classList.add('js');
if ((matchMedia('(display-mode: standalone)').matches || navigator.standalone === true) && /^\/(home\/?)?$/.test(location.pathname)) {
  location.replace('/app');
}
// 3. Scroll reveal: hide the reveal targets only when it can run (IntersectionObserver, no reduced motion), and give up
//    after 4s if reveal.js has not reported ready, so content can never stay hidden.
if ('IntersectionObserver' in window && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
  document.documentElement.classList.add('reveal-on');
  setTimeout(() => {
    if (!document.documentElement.classList.contains('reveal-ready')) document.documentElement.classList.remove('reveal-on');
  }, 4000);
}
