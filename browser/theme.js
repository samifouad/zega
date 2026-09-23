export const palettes = {
  light: { ground: '#F3EFE3', panel: '#FBF8EF', water: '#BFD3D6', rule: '#D8CDBA', strongRule: '#B9A88C', ink: '#30251B', soft: '#7E6C59', accent: '#AC542E', park: '#E0E3CC' },
  dark: { ground: '#1E1610', panel: '#281E16', water: '#23383C', rule: '#423326', strongRule: '#6D5540', ink: '#F3EFE3', soft: '#BDAA90', accent: '#E19B68', park: '#293023' },
};
export function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  const colors = palettes[theme];
  for (const [key, value] of Object.entries(colors)) document.documentElement.style.setProperty(`--${key}`, value);
  for (const [key, target] of Object.entries({ bg: 'ground', border: 'rule', dim: 'soft' })) document.documentElement.style.setProperty(`--${key}`, colors[target]);
}
