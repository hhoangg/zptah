// Providers shown in the model menu (name, tile gradient, whether the glyph is monochrome) and shared constants.

export const PROV = [
  { id: 'codex', name: 'Codex', tile: ['#7A8CFF', '#4048FF'], mono: false, model: 'Codex' },
  { id: 'claude', name: 'Claude Code', tile: ['#E3876A', '#D06A49'], mono: false, model: 'Claude Opus 5' },
  { id: 'cursor', name: 'Cursor', tile: ['#343434', '#161616'], mono: true, model: 'Composer 2.5' },
  { id: 'grok', name: 'Grok Build', tile: ['#343434', '#161616'], mono: true, model: 'Grok 4.6' },
  { id: 'opencode', name: 'OpenCode', tile: ['#3B3636', '#1F1C1C'], mono: true, model: 'OpenCode' },
  { id: 'antigravity', name: 'Antigravity', tile: ['#e9e9ee', '#cfcfd8'], mono: false, model: 'Antigravity' },
  { id: 'pi', name: 'Pi', tile: ['#343434', '#161616'], mono: true, model: 'Pi' },
  { id: 'omp', name: 'omp', tile: ['#C45BE0', '#8A5CF2'], mono: false, model: 'omp' },
  { id: 'fx', name: 'fx', tile: ['#343434', '#161616'], mono: true, model: 'fx' },
  { id: 'hermes', name: 'Hermes Agent', tile: ['#3A3A3E', '#1E1E22'], mono: true, model: 'Hermes' },
  { id: 'devin', name: 'Devin', tile: ['#3A3A3E', '#1E1E22'], mono: true, model: 'Devin' },
];
export const P = Object.fromEntries(PROV.map((p) => [p.id, p]));


/** The app icon, served from the landing's own files (the CSP has no remote images). */
export const APP_ICON = '/landing/app-icon.png';
