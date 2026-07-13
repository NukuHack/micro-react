// ─── NOTE on html`` usage in this file ───
// Every component below uses `html` only, no `h()`.
//  don't self-close a non-void element or component tag (`<${Comp} />`);
//  write the explicit closing tag: `<${Comp}></${Comp}>`.

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

// ─── COUNTER CARD  (useState + useReducer) ───
function counterReducer(state, action) {
  switch (action.type) {
    case 'INC':   return { ...state, count: state.count + state.step };
    case 'DEC':   return { ...state, count: state.count - state.step };
    case 'RESET': return { count: 0, step: state.step };
    case 'STEP':  return { ...state, step: action.val };
    default:      return state;
  }
}

function CounterCard() {
  const [{ count, step }, dispatch] = useReducer(counterReducer, { count: 0, step: 1 });
  const cls = count > 0 ? 'positive' : count < 0 ? 'negative' : 'zero';

  return html`
    <div class="card">
      <span class="hook-pill">useReducer</span>
      <h3>Counter</h3>
      <div class="counter-display ${cls}">${count}</div>
      <div class="btn-row" style="margin:.75rem 0">
        <button class="btn-ghost" onclick="${() => dispatch({ type: 'DEC' })}">−</button>
        <button class="btn-primary btn-sm" onclick="${() => dispatch({ type: 'RESET' })}">Reset</button>
        <button class="btn-ghost" onclick="${() => dispatch({ type: 'INC' })}">+</button>
      </div>
      <label style="font-size:.8rem;color:var(--muted)">
        Step:
        <input type="number" value="${step}" style="width:64px;margin-left:.4rem"
          onchange="${e => dispatch({ type: 'STEP', val: Number(e.target.value) || 1 })}" />
      </label>
    </div>
  `;
}

// ─── TODO CARD  (useState + useCallback + useMemo) ───
let _todoId = 0;
function TodoCard() {
  const [items, setItems] = useState([
    { id: ++_todoId, text: 'Try micro-react', done: true },
    { id: ++_todoId, text: 'Build something cool', done: false },
  ]);
  const [draft, setDraft] = useState('');
  const inputRef = useRef(null);

  const add = useCallback(() => {
    const t = draft.trim();
    if (!t) return;
    setItems(prev => [...prev, { id: ++_todoId, text: t, done: false }]);
    setDraft('');
    inputRef.current?.focus();
  }, [draft]);

  const toggle = useCallback(id =>
    setItems(prev => prev.map(it => it.id === id ? { ...it, done: !it.done } : it)),
  []);

  const remove = useCallback(id =>
    setItems(prev => prev.filter(it => it.id !== id)),
  []);

  const stats = useMemo(() => ({
    total: items.length,
    done:  items.filter(i => i.done).length,
    pct:   items.length ? Math.round(items.filter(i => i.done).length / items.length * 100) : 0,
  }), [items]);

  return html`
    <div class="card">
      <span class="hook-pill">useState · useCallback · useMemo · useRef</span>
      <h3>Todo List</h3>
      <div class="todo-input-row">
        <input type="text" placeholder="Add item…" value="${draft}" ref="${inputRef}"
          oninput="${e => setDraft(e.target.value)}"
          onkeydown="${e => e.key === 'Enter' && add()}" />
        <button class="btn-primary" onclick="${add}">Add</button>
      </div>
      <div class="progress-track">
        <div class="progress-fill" style="width:${stats.pct}%"></div>
      </div>
      <ul class="todo-list">
        ${items.map(it => html`
          <li key="${it.id}" class="todo-item${it.done ? ' done' : ''}">
            <input type="checkbox" checked="${it.done}" onchange="${() => toggle(it.id)}" />
            ${it.text}
            <button class="del" onclick="${() => remove(it.id)}">✕</button>
          </li>
        `)}
      </ul>
      <p class="todo-stats">${stats.done} / ${stats.total} done (${stats.pct}%)</p>
    </div>
  `;
}

// ─── TIMER CARD  (useEffect + useRef + useState) ───
function TimerCard() {
  const [ms, setMs]       = useState(0);
  const [running, setRun] = useState(false);
  const startRef = useRef(null);
  const rafRef   = useRef(null);

  useEffect(() => {
    if (running) {
      startRef.current = performance.now() - ms;
      const tick = () => {
        setMs(Math.floor(performance.now() - startRef.current));
        rafRef.current = requestAnimationFrame(tick);
      };
      rafRef.current = requestAnimationFrame(tick);
      return () => cancelAnimationFrame(rafRef.current);
    }
  }, [running]);

  const fmt = t => {
    const m  = Math.floor(t / 60000);
    const s  = Math.floor((t % 60000) / 1000);
    const cs = Math.floor((t % 1000) / 10);
    return `${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}.${String(cs).padStart(2,'0')}`;
  };

  return html`
    <div class="card">
      <span class="hook-pill">useEffect · useRef</span>
      <h3>Stopwatch</h3>
      <div class="timer-display">${fmt(ms)}</div>
      <div class="btn-row" style="margin-top:.75rem">
        <button class="${running ? 'btn-danger' : 'btn-success'}" onclick="${() => setRun(r => !r)}">${running ? '⏸ Pause' : '▶ Start'}</button>
        <button class="btn-ghost" disabled="${running}" onclick="${() => setMs(0)}">↺ Reset</button>
      </div>
    </div>
  `;
}

// ─── THEME DEMO  (useContext) ───
function ThemeCard() {
  const theme  = ThemeCtx.useContext();
  const [, setTheme] = useState(0);

  return html`
    <div class="card">
      <span class="hook-pill">createContext · useContext</span>
      <h3>Theme Context</h3>
      <p style="font-size:.875rem;color:var(--muted);margin-bottom:.75rem">Theme is set from App root. Consumer reads it anywhere in the tree.</p>
      <div class="theme-box theme-${theme.name}">Current theme: "${theme.label}" — rendered from context 🎨</div>
    </div>
  `;
}

// ─── MEMO DEMO ───
const RenderLog = memo(function RenderLog({ logs }) {
  return html`
    <div class="render-log">
      ${logs.map((l, i) => html`<p key="${i}">${l}</p>`)}
    </div>
  `;
});

function MemoCard() {
  const [count, setCount] = useState(0);
  const [unrelated, setUnrelated] = useState(0);
  const logsRef = useRef([]);

  const stableLogs = useMemo(() => {
    logsRef.current = [...logsRef.current, `[${new Date().toLocaleTimeString()}] MemoChild rendered (count=${count})`];
    if (logsRef.current.length > 6) logsRef.current.shift();
    return logsRef.current;
  }, [count]);

  return html`
    <div class="card">
      <span class="hook-pill">memo · useMemo</span>
      <h3>memo() Demo</h3>
      <p style="font-size:.875rem;color:var(--muted);margin-bottom:.75rem">The log only updates when "count" changes, not when "unrelated" does.</p>
      <div class="btn-row">
        <button class="btn-primary btn-sm" onclick="${() => setCount(c => c + 1)}">Count: ${count}</button>
        <button class="btn-ghost btn-sm" onclick="${() => setUnrelated(u => u + 1)}">Unrelated: ${unrelated}</button>
      </div>
      <${RenderLog} logs="${stableLogs}"></${RenderLog}>
    </div>
  `;
}

// ─── REF CARD  (useRef + useLayoutEffect) ───
function RefCard() {
  const boxRef = useRef(null);
  const [lit, setLit] = useState(false);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useLayoutEffect(() => {
    if (boxRef.current && boxRef.current instanceof Element) {
      const r = boxRef.current.getBoundingClientRect();
      setSize({ w: Math.round(r.width), h: Math.round(r.height) });
    }
  }, [lit]);

  return html`
    <div class="card">
      <span class="hook-pill">useRef · useLayoutEffect</span>
      <h3>DOM Refs</h3>
      <div ref="${boxRef}" class="card highlight-box${lit ? ' lit' : ''}"
        style="margin-top:.5rem;padding:1rem;cursor:pointer" onclick="${() => setLit(l => !l)}">
        <p>Click to ${lit ? 'un-highlight' : 'highlight'} me</p>
        <p style="font-size:.8rem;color:var(--muted);margin-top:.4rem">Measured via ref: ${size.w} × ${size.h}px</p>
      </div>
    </div>
  `;
}

// ─── ERROR BOUNDARY CARD ───
function BombComponent({ shouldExplode }) {
  if (shouldExplode) throw new Error('💥 Intentional render error!');
  return html`<div style="color:var(--green);font-size:.9rem">✅ Component is rendering fine.</div>`;
}

function ErrorBoundaryCard() {
  const [explode, setExplode] = useState(false);
  const [key, setKey]         = useState(0);

  const fallback = err => html`
    <div class="error-box">
      <strong>Caught error: </strong>${err.message}<br />
      <button class="btn-ghost btn-sm" style="margin-top:.6rem" onclick="${() => { setExplode(false); setKey(k => k + 1); }}">↺ Reset</button>
    </div>
  `;

  return html`
    <div class="card">
      <span class="hook-pill">ErrorBoundary</span>
      <h3>Error Boundary</h3>
      <${ErrorBoundary} key="${key}" fallback="${fallback}">
        <${BombComponent} shouldExplode="${explode}"></${BombComponent}>
      </${ErrorBoundary}>
      <div class="btn-row" style="margin-top:.75rem">
        <button class="btn-danger btn-sm" onclick="${() => setExplode(true)}" disabled="${explode}">💣 Throw error</button>
      </div>
    </div>
  `;
}

// ─── TABS DEMO  (useState + dynamic children) ───
const tabContent = {
  Events:  () => html`<p>🧲 Event delegation via logical-clock proxy. Prevents "click mounts node that immediately receives same click" race (ported from Preact).</p>`,
  Hooks:   () => html`<p>🪝 Full hook surface: useState, useReducer, useEffect, useLayoutEffect, useMemo, useCallback, useRef, useId, useTransition, useDeferredValue, useSyncExternalStore, useImperativeHandle.</p>`,
  Router:  () => html`<p>🛣️ Built-in SPA router with pattern matching, Link component, and hooks: useLocation, useNavigate, useParams, useSearchParams. URL and DOM stay in sync.</p>`,
  Context: () => html`<p>🧩 createContext with Provider / Consumer / ctx.useContext(). Subscribers re-render on value changes without prop-drilling.</p>`,
};

function TabsCard() {
  const [tab, setTab] = useState('Events');
  const Content = tabContent[tab];

  return html`
    <div class="card">
      <span class="hook-pill">useState</span>
      <h3>Feature Tabs</h3>
      <div class="tabs">
        ${Object.keys(tabContent).map(t =>
          html`<button key="${t}" class="tab${tab === t ? ' active' : ''}" onclick="${() => setTab(t)}">${t}</button>`
        )}
      </div>
      <div style="font-size:.875rem;line-height:1.6;color:var(--muted)">
        <${Content}></${Content}>
      </div>
    </div>
  `;
}

// ─── HOME PAGE ───
function HomePage() {
  return html`
    <main class="page-enter">
      <div class="hero">
        <div>
          <div class="hero-pill" style="margin-bottom:.75rem"><span class="dot"></span>live demo</div>
          <h1>micro-react</h1>
          <h2>Pure JS · no build step · no dependencies</h2>
        </div>
      </div>

      <div class="card-grid">
        <${CounterCard}></${CounterCard}>
        <${TimerCard}></${TimerCard}>
      </div>

      <${TodoCard}></${TodoCard}>

      <div class="card-grid">
        <${ThemeCard}></${ThemeCard}>
        <${MemoCard}></${MemoCard}>
      </div>

      <div class="card-grid">
        <${RefCard}></${RefCard}>
        <${ErrorBoundaryCard}></${ErrorBoundaryCard}>
      </div>

      <${TabsCard}></${TabsCard}>
    </main>
  `;
}

// ─── ABOUT PAGE ───
function AboutPage() {
  const navigate = useNavigate();

  const features = [
    { icon: '⚡', title: 'Skew-based diffing',   desc: 'Preact-style O(n) keyed reconciler with insertion-skew tracking.' },
    { icon: '🎯', title: 'Event delegation',      desc: 'Single proxy per event name + logical-clock guard against mount-race bugs.' },
    { icon: '🔒', title: 'Security built-in',     desc: 'URL sanitisation, CSS injection guard, prototype-pollution protection.' },
    { icon: '🪝', title: 'Full hook surface',     desc: '15 hooks including useTransition, useDeferredValue, useSyncExternalStore.' },
    { icon: '🧩', title: 'Context API',           desc: 'createContext with Provider, Consumer and useContext subscriber pattern.' },
    { icon: '🛣️', title: 'SPA Router',            desc: 'Pattern-matched router with Link, useNavigate, useParams, useSearchParams.' },
    { icon: '🏎️', title: 'Depth-sorted queue',    desc: 'Parents always re-render before children; redundant child renders skipped.' },
    { icon: '🏷️', title: 'html`` template tag',  desc: 'Tagged template literal alternative to createElement for quick authoring.' },
  ];

  return html`
    <main class="page-enter">
      <h1>About micro-react</h1>
      <h2>A 1-file React-compatible runtime for the browser</h2>

      <div class="card">
        <p style="line-height:1.7;color:var(--muted);font-size:.9rem">micro-react absorbs the most impactful internals from Preact's source while staying under one self-contained file with zero build requirements. Drop it next to an HTML page, import it as an ES module, and you have a full component model with hooks, context, portals, lazy loading, error boundaries, and a built-in SPA router.</p>
      </div>

<div class="code-block">
<span class="cm">// no build step needed\n</span>
<span class="kw">import</span> <span class="fn">initWasm</span>, * <span class="kw">as</span> MicroReact <span class="kw">from</span> <span class="str">'./micro-react.js'</span>;

<span class="kw">const</span> { html, useState, render } = MicroReact;
<span class="kw">await</span> <span class="fn">initWasm</span>();

<span class="kw">function</span> <span class="fn">App</span>() {
  <span class="kw">const</span> [n, setN] = useState(<span class="num">0</span>);
  <span class="kw">return</span> html<span class="str">\`&lt;button onclick="\${() =&gt; setN(n+<span class="num">1</span>)}"&gt;\${n}&lt;/button&gt;\`</span>;
}

const __root = render(html<span class="str">\`&lt;\${<span class="fn">App</span>}&gt;&lt;/\${<span class="fn">App</span>}&gt;\`</span>, document.getElementById(<span class="str">'root'</span>));
<span class="fn">window</span>.__root = __root;</div>

      <div class="feature-grid">
        ${features.map(f => html`
          <div key="${f.title}" class="feature-item">
            <div class="icon">${f.icon}</div>
            <h4>${f.title}</h4>
            <p>${f.desc}</p>
          </div>
        `)}
      </div>

      <div style="margin-top:2rem">
        <button class="btn-primary" onclick="${() => navigate('/')}">← Back to Demo</button>
      </div>
    </main>
  `;
}

// ─── SHELL ───
function Shell({ themeIdx, setThemeIdx, Page }) {
  
  return html`
    <${Fragment}>
      <${Nav}></${Nav}>
      <${Page}></${Page}>
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
    '/':      () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${HomePage} ></${Shell}>`,
    '/about': () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${AboutPage} ></${Shell}>`,
    '/hello': () => html`<${Shell} themeIdx="${themeIdx}" setThemeIdx="${setThemeIdx}" Page=${HelloPage} ></${Shell}>`,
  };

  return html`
    <${ThemeCtx.Provider} value="${theme}">
      <${Router} routes="${routes}"></${Router}>
    </${ThemeCtx.Provider}>
  `;
}

// ─── JSX LOADER DEMO  (fetch -> transpileJsx -> render) ───
const { default: Hello,  lol } = await window.loadJsxModule('./hello.jsx');
function HelloPage() {
  return html`<div style='position:relative;top:1rem;left:1rem;z-index:999'>
    <${Hello} name="micro-react"/>
    <${lol} name="Looool"/>
  </div>`;
}

const __root = render(html`<${App}></${App}>`, document.getElementById('root'));
window.__root = __root;
