// Entry point of the landing page.
import { initDemo } from './demo.js';
import { initReveal } from './reveal.js';
import { initCopy, initInstall, initProviders } from './extras.js';

const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const data = JSON.parse(document.getElementById('landing-data').textContent);

initDemo(data, reducedMotion);
initProviders(reducedMotion);
initInstall();
initCopy();
initReveal();
