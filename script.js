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
  return h('nav', null,
    h('div', { className: 'nav-brand' },
      h('span', { style: { fontSize: '1.2rem' } }, '⚛'),
      'micro', h('span', null, '-react'),
    ),
    h(Link, { to: '/',     className: `nav-link${path === '/'     ? ' active' : ''}` }, '🏠 Home'),
    h(Link, { to: '/about',className: `nav-link${path === '/about'? ' active' : ''}` }, '📖 About'),
  );
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

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'useReducer'),
    h('h3', null, 'Counter'),
    h('div', { className: `counter-display ${cls}` }, count),
    h('div', { className: 'btn-row', style: { margin: '.75rem 0' } },
      h('button', { className: 'btn-ghost', onClick: () => dispatch({ type: 'DEC' }) }, '−'),
      h('button', { className: 'btn-primary btn-sm', onClick: () => dispatch({ type: 'RESET' }) }, 'Reset'),
      h('button', { className: 'btn-ghost', onClick: () => dispatch({ type: 'INC' }) }, '+'),
    ),
    h('label', { style: { fontSize: '.8rem', color: 'var(--muted)' } },
      'Step: ',
      h('input', {
        type: 'number', value: step, style: { width: '64px', marginLeft: '.4rem' },
        onChange: e => dispatch({ type: 'STEP', val: Number(e.target.value) || 1 }),
      }),
    ),
  );
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

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'useState · useCallback · useMemo · useRef'),
    h('h3', null, 'Todo List'),
    h('div', { className: 'todo-input-row' },
      h('input', {
        type: 'text', placeholder: 'Add item…', value: draft, ref: inputRef,
        onInput: e => setDraft(e.target.value),
        onKeydown: e => e.key === 'Enter' && add(),
      }),
      h('button', { className: 'btn-primary', onClick: add }, 'Add'),
    ),
    h('div', { className: 'progress-track' },
      h('div', { className: 'progress-fill', style: { width: `${stats.pct}%` } }),
    ),
    h('ul', { className: 'todo-list' },
      ...items.map(it =>
        h('li', { key: it.id, className: `todo-item${it.done ? ' done' : ''}` },
          h('input', { type: 'checkbox', checked: it.done, onChange: () => toggle(it.id) }),
          it.text,
          h('button', { className: 'del', onClick: () => remove(it.id) }, '✕'),
        )
      ),
    ),
    h('p', { className: 'todo-stats' },
      `${stats.done} / ${stats.total} done (${stats.pct}%)`
    ),
  );
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

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'useEffect · useRef'),
    h('h3', null, 'Stopwatch'),
    h('div', { className: 'timer-display' }, fmt(ms)),
    h('div', { className: 'btn-row', style: { marginTop: '.75rem' } },
      h('button', {
        className: running ? 'btn-danger' : 'btn-success',
        onClick: () => setRun(r => !r),
      }, running ? '⏸ Pause' : '▶ Start'),
      h('button', {
        className: 'btn-ghost', disabled: running,
        onClick: () => setMs(0),
      }, '↺ Reset'),
    ),
  );
}

// ─── THEME DEMO  (useContext) ───
function ThemeCard() {
  const theme  = ThemeCtx.useContext();
  const [, setTheme] = useState(0);

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'createContext · useContext'),
    h('h3', null, 'Theme Context'),
    h('p', { style: { fontSize: '.875rem', color: 'var(--muted)', marginBottom: '.75rem' } },
      'Theme is set from App root. Consumer reads it anywhere in the tree.'
    ),
    h('div', { className: `theme-box theme-${theme.name}` },
      `Current theme: "${theme.label}" — rendered from context 🎨`
    ),
  );
}

// ─── MEMO DEMO ───
const RenderLog = memo(function RenderLog({ logs }) {
  return h('div', { className: 'render-log' },
    ...logs.map((l, i) => h('p', { key: i }, l)),
  );
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

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'memo · useMemo'),
    h('h3', null, 'memo() Demo'),
    h('p', { style: { fontSize: '.875rem', color: 'var(--muted)', marginBottom: '.75rem' } },
      'The log only updates when "count" changes, not when "unrelated" does.'
    ),
    h('div', { className: 'btn-row' },
      h('button', { className: 'btn-primary btn-sm', onClick: () => setCount(c => c + 1) },
        `Count: ${count}`,
      ),
      h('button', { className: 'btn-ghost btn-sm', onClick: () => setUnrelated(u => u + 1) },
        `Unrelated: ${unrelated}`,
      ),
    ),
    h(RenderLog, { logs: stableLogs }),
  );
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

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'useRef · useLayoutEffect'),
    h('h3', null, 'DOM Refs'),
    h('div', {
      ref: boxRef,
      className: `card highlight-box${lit ? ' lit' : ''}`,
      style: { marginTop: '.5rem', padding: '1rem', cursor: 'pointer' },
      onClick: () => setLit(l => !l),
    },
      h('p', null, `Click to ${lit ? 'un-highlight' : 'highlight'} me`),
      h('p', { style: { fontSize: '.8rem', color: 'var(--muted)', marginTop: '.4rem' } },
        `Measured via ref: ${size.w} × ${size.h}px`
      ),
    ),
  );
}

// ─── ERROR BOUNDARY CARD ───
function BombComponent({ shouldExplode }) {
  if (shouldExplode) throw new Error('💥 Intentional render error!');
  return h('div', { style: { color: 'var(--green)', fontSize: '.9rem' } },
    '✅ Component is rendering fine.'
  );
}

function ErrorBoundaryCard() {
  const [explode, setExplode] = useState(false);
  const [key, setKey]         = useState(0);

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'ErrorBoundary'),
    h('h3', null, 'Error Boundary'),
    h(ErrorBoundary, {
      key,
      fallback: err => h('div', { className: 'error-box' },
        h('strong', null, 'Caught error: '), err.message,
        h('br'),
        h('button', {
          className: 'btn-ghost btn-sm', style: { marginTop: '.6rem' },
          onClick: () => { setExplode(false); setKey(k => k + 1); },
        }, '↺ Reset'),
      ),
    },
      h(BombComponent, { shouldExplode: explode }),
    ),
    h('div', { className: 'btn-row', style: { marginTop: '.75rem' } },
      h('button', {
        className: 'btn-danger btn-sm',
        onClick: () => setExplode(true),
        disabled: explode,
      }, '💣 Throw error'),
    ),
  );
}

// ─── TABS DEMO  (useState + dynamic children) ───
const tabContent = {
  Events:  () => h('p', null, '🧲 Event delegation via logical-clock proxy. Prevents "click mounts node that immediately receives same click" race (ported from Preact).'),
  Hooks:   () => h('p', null, '🪝 Full hook surface: useState, useReducer, useEffect, useLayoutEffect, useMemo, useCallback, useRef, useId, useTransition, useDeferredValue, useSyncExternalStore, useImperativeHandle.'),
  Router:  () => h('p', null, '🛣️ Built-in SPA router with pattern matching, Link component, and hooks: useLocation, useNavigate, useParams, useSearchParams. URL and DOM stay in sync.'),
  Context: () => h('p', null, '🧩 createContext with Provider / Consumer / ctx.useContext(). Subscribers re-render on value changes without prop-drilling.'),
};

function TabsCard() {
  const [tab, setTab] = useState('Events');
  const Content = tabContent[tab];

  return h('div', { className: 'card' },
    h('span', { className: 'hook-pill' }, 'useState'),
    h('h3', null, 'Feature Tabs'),
    h('div', { className: 'tabs' },
      ...Object.keys(tabContent).map(t =>
        h('button', { key: t, className: `tab${tab === t ? ' active' : ''}`, onClick: () => setTab(t) }, t)
      ),
    ),
    h('div', { style: { fontSize: '.875rem', lineHeight: '1.6', color: 'var(--muted)' } },
      h(Content),
    ),
  );
}

// ─── HOME PAGE ───
function HomePage() {
  return h('main', { className: 'page-enter' },
    h('div', { className: 'hero' },
      h('div', null,
        h('div', { className: 'hero-pill', style: { marginBottom: '.75rem' } },
          h('span', { className: 'dot' }), 'live demo',
        ),
        h('h1', null, 'micro-react v3'),
        h('h2', null, 'Pure JS · no build step · no dependencies'),
      ),
    ),

    h('div', { className: 'card-grid' },
      h(CounterCard),
      h(TimerCard),
    ),

    h(TodoCard),

    h('div', { className: 'card-grid' },
      h(ThemeCard),
      h(MemoCard),
    ),

    h('div', { className: 'card-grid' },
      h(RefCard),
      h(ErrorBoundaryCard),
    ),

    h(TabsCard),
  );
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

  return h('main', { className: 'page-enter' },
    h('h1', null, 'About micro-react'),
    h('h2', null, 'A 1-file React-compatible runtime for the browser'),

    h('div', { className: 'card' },
      h('p', { style: { lineHeight: '1.7', color: 'var(--muted)', fontSize: '.9rem' } },
        'micro-react v3 absorbs the most impactful internals from Preact\'s source while staying under one self-contained file with zero build requirements. Drop it next to an HTML page, import it as an ES module, and you have a full component model with hooks, context, portals, lazy loading, error boundaries, and a built-in SPA router.'
      ),
    ),

    html`<div class="code-block"><span class="cm">// no build step needed
</span><span class="kw">import</span> MicroReact <span class="kw">from</span> <span class="str">'./micro-react.js'</span>;

<span class="kw">const</span> { createElement: h, useState, render } = MicroReact;

<span class="kw">function</span> <span class="fn">App</span>() {
  <span class="kw">const</span> [n, setN] = useState(<span class="num">0</span>);
  <span class="kw">return</span> h(<span class="str">'button'</span>, { onClick: () => setN(n+<span class="num">1</span>) }, n);
}

render(h(<span class="fn">App</span>), document.getElementById(<span class="str">'root'</span>));</div>`,

    h('div', { className: 'feature-grid' },
      ...features.map(f =>
        h('div', { key: f.title, className: 'feature-item' },
          h('div', { className: 'icon' }, f.icon),
          h('h4', null, f.title),
          h('p', null, f.desc),
        )
      ),
    ),

    h('div', { style: { marginTop: '2rem' } },
      h('button', { className: 'btn-primary', onClick: () => navigate('/') }, '← Back to Demo'),
    ),
  );
}

// ─── SHELL  (inside Router, so Nav can use useLocation) ───
function Shell({ themeIdx, setThemeIdx }) {
  const { path } = useLocation();

  const Page = path === '/about' ? AboutPage : HomePage;

  return h(Fragment, null,
    h(Nav),
    h(Page),
    // Theme switcher (fixed bottom-right)
    h('div', {
      style: {
        position: 'fixed', bottom: '1.25rem', right: '1.25rem',
        display: 'flex', gap: '.4rem', zIndex: 999,
        background: 'var(--surface)', border: '1px solid var(--border)',
        padding: '.4rem', borderRadius: '10px',
      },
    },
      ...themes.map((t, i) =>
        h('button', {
          key: t.name,
          className: `btn-ghost btn-sm${themeIdx === i ? ' btn-primary' : ''}`,
          onClick: () => setThemeIdx(i),
          title: t.label,
        }, t.label.split(' ')[0])
      ),
    ),
  );
}

// ─── APP ROOT  (provides theme context + router) ───
function App() {
  const [themeIdx, setThemeIdx] = useState(0);
  const theme = themes[themeIdx];

  return h(ThemeCtx.Provider, { value: theme },
    h(Router, {
      routes: {
        '/':      () => h(Shell, { themeIdx, setThemeIdx }),
        '/about': () => h(Shell, { themeIdx, setThemeIdx }),
      },
    }),
  );
}

const __root = render(h(App), document.getElementById('root'));
window.__root = __root;