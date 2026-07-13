// ─── NOTE on html`` usage in this file ───
// Every component below uses `html` only, no `createElement()` anymore.

// ─── THEME CONTEXT ───
const ThemeCtx = createContext({ name: 'dark', label: 'Dark' });
const themes = [
  { name: 'dark',  label: '🌑 Dark'  },
  { name: 'light', label: '☀️ Light' },
  { name: 'neon',  label: '🟩 Neon'  },
];

// ─── NAV ───
function Nav() {
  const { path } = useLocation();
  return html`
    <nav>
      <div class="nav-brand"><span style="font-size:1.2rem">⚛</span>micro<span>-react</span></div>
      <${Link} to="/" class="nav-link${path === '/' ? ' active' : ''}">🏠 Home</${Link}>
      <${Link} to="/about" class="nav-link${path === '/about' ? ' active' : ''}">📖 About</${Link}>
      <${Link} to="/hello" class="nav-link${path === '/hello' ? ' active' : ''}">👋 Hello</${Link}>
    </nav>
  `;
}

// ─── SHELL ───
function Shell({ themeIdx, setThemeIdx, Page }) {
  
  return html`
    <${Fragment}>
      <${Nav} />
      <${Page} ThemeCtx=${ThemeCtx}/>
      <div style="position:fixed;bottom:1.25rem;right:1.25rem;display:flex;gap:.4rem;z-index:999;background:var(--surface);border:1px solid var(--border);padding:.4rem;border-radius:10px">
        ${themes.map((t, i) => html`
          <button key="${t.name}" class="btn-ghost btn-sm${themeIdx === i ? ' btn-primary' : ''}" onclick="${() => setThemeIdx(i)}" title="${t.label}">${t.label.split(' ')[0]}</button>
        `)}
      </div>
    </${Fragment}>
  `;
}

// ─── APP ROOT  (provides theme context + router) ───
function App() {
  const [themeIdx, setThemeIdx] = useState(0);
  const theme = themes[themeIdx];

  const routes = {
    '/':      () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${HomePage} />`,
    '/about': () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${AboutPage} />`,
    '/hello': () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${HelloPage} />`,
    '*': () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${NotFound} />`,
  };

  return html`
    <${ThemeCtx.Provider} value="${theme}">
      <${Router} routes="${routes}" />
    </${ThemeCtx.Provider}>
  `;
}

// ─── JSX LOADER DEMO  (fetch -> transpileJsx -> render) ───
const { default: HelloPage } = await window.loadJsxModule('./hello.jsx');
const { default: NotFound } = await window.loadJsxModule('./404.jsx');
const { default: HomePage } = await window.loadJsxModule('./home.jsx');
const { default: AboutPage } = await window.loadJsxModule('./about.jsx');

const __root = render(html`<${App} />`, document.getElementById('root'));
window.__root = __root;
