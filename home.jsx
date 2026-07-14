// ─── COUNTER CARD (useState + useReducer) ───
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

  return (
    <div class="card">
      <span class="hook-pill">useReducer</span>
      <h3>Counter</h3>
      <div class={`counter-display ${cls}`}>{count}</div>
      <div class="btn-row" style="margin:.75rem 0">
        <button class="btn-ghost" onclick={() => dispatch({ type: 'DEC' })}>−</button>
        <button class="btn-primary btn-sm" onclick={() => dispatch({ type: 'RESET' })}>Reset</button>
        <button class="btn-ghost" onclick={() => dispatch({ type: 'INC' })}>+</button>
      </div>
      <label style="font-size:.8rem;color:var(--muted)">
        Step:
        <input 
          type="number" 
          value={step} 
          style="width:64px;margin-left:.4rem"
          onchange={(e) => dispatch({ type: 'STEP', val: Number(e.target.value) || 1 })} 
        />
      </label>
    </div>
  );
}

// ─── TODO CARD (useState + useCallback + useMemo) ───
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

  return (
    <div class="card">
      <span class="hook-pill">useState · useCallback · useMemo · useRef</span>
      <h3>Todo List</h3>
      <div class="todo-input-row">
        <input 
          type="text" 
          placeholder="Add item…" 
          value={draft} 
          ref={inputRef}
          oninput={(e) => setDraft(e.target.value)}
          onkeydown={(e) => e.key === 'Enter' && add()} 
        />
        <button class="btn-primary" onclick={add}>Add</button>
      </div>
      <div class="progress-track">
        <div class="progress-fill" style={`width:${stats.pct}%`}></div>
      </div>
      <ul class="todo-list">
        {items.map(it => (
          <li key={it.id} class={`todo-item${it.done ? ' done' : ''}`}>
            <input type="checkbox" checked={it.done} onchange={() => toggle(it.id)} />
            {it.text}
            <button class="del" onclick={() => remove(it.id)}>✕</button>
          </li>
        ))}
      </ul>
      <p class="todo-stats">{stats.done} / {stats.total} done ({stats.pct}%)</p>
    </div>
  );
}

// ─── TIMER CARD (useEffect + useRef + useState) ───
function TimerCard() {
  const [ms, setMs] = useState(0);
  const [running, setRun] = useState(false);
  const startRef = useRef(null);
  const rafRef = useRef(null);

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
    const m = Math.floor(t / 60000);
    const s = Math.floor((t % 60000) / 1000);
    const cs = Math.floor((t % 1000) / 10);
    return `${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}.${String(cs).padStart(2,'0')}`;
  };

  return (
    <div class="card">
      <span class="hook-pill">useEffect · useRef</span>
      <h3>Stopwatch</h3>
      <div class="timer-display">{fmt(ms)}</div>
      <div class="btn-row" style="margin-top:.75rem">
        <button 
          class={running ? 'btn-danger' : 'btn-success'} 
          onclick={() => setRun(r => !r)}
        >
          {running ? '⏸ Pause' : '▶ Start'}
        </button>
        <button class="btn-ghost" disabled={running} onclick={() => setMs(0)}>↺ Reset</button>
      </div>
    </div>
  );
}

// ─── THEME DEMO (useContext) ───
function ThemeCard({ThemeCtx}) {
  const theme = useContext(ThemeCtx);

  return (
    <div class="card">
      <span class="hook-pill">createContext · useContext</span>
      <h3>Theme Context</h3>
      <p style="font-size:.875rem;color:var(--muted);margin-bottom:.75rem">
        Theme is set from App root. Consumer reads it anywhere in the tree.
      </p>
      <div class={`theme-box theme-${theme.name}`}>
        Current theme: "{theme.label}" — rendered from context 🎨
      </div>
    </div>
  );
}

// ─── MEMO DEMO ───
const RenderLog = memo(function RenderLog({ logs }) {
  return (
    <div class="render-log">
      {logs.map((l, i) => <p key={i}>{l}</p>)}
    </div>
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

  return (
    <div class="card">
      <span class="hook-pill">memo · useMemo</span>
      <h3>memo() Demo</h3>
      <p style="font-size:.875rem;color:var(--muted);margin-bottom:.75rem">
        The log only updates when "count" changes, not when "unrelated" does.
      </p>
      <div class="btn-row">
        <button class="btn-primary btn-sm" onclick={() => setCount(c => c + 1)}>
          Count: {count}
        </button>
        <button class="btn-ghost btn-sm" onclick={() => setUnrelated(u => u + 1)}>
          Unrelated: {unrelated}
        </button>
      </div>
      <RenderLog logs={stableLogs} />
    </div>
  );
}

// ─── REF CARD (useRef + useLayoutEffect) ───
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

  return (
    <div class="card">
      <span class="hook-pill">useRef · useLayoutEffect</span>
      <h3>DOM Refs</h3>
      <div 
        ref={boxRef} 
        class={`card highlight-box${lit ? ' lit' : ''}`}
        style="margin-top:.5rem;padding:1rem;cursor:pointer" 
        onclick={() => setLit(l => !l)}
      >
        <p>Click to {lit ? 'un-highlight' : 'highlight'} me</p>
        <p style="font-size:.8rem;color:var(--muted);margin-top:.4rem">
          Measured via ref: {size.w} × {size.h}px
        </p>
      </div>
    </div>
  );
}

// ─── ERROR BOUNDARY CARD ───
function BombComponent({ shouldExplode }) {
  if (shouldExplode) throw new Error('💥 Intentional render error!');
  return <div style="color:var(--green);font-size:.9rem">✅ Component is rendering fine.</div>;
}

function ErrorBoundaryCard() {
  const [explode, setExplode] = useState(false);
  const [key, setKey] = useState(0);

  const fallback = err => (
    <div class="error-box">
      <strong>Caught error: </strong>{err.message}<br />
      <button 
        class="btn-ghost btn-sm" 
        style="margin-top:.6rem" 
        onclick={() => { setExplode(false); setKey(k => k + 1); }}
      >
        ↺ Reset
      </button>
    </div>
  );

  return (
    <div class="card">
      <span class="hook-pill">ErrorBoundary</span>
      <h3>Error Boundary</h3>
      <ErrorBoundary key={key} fallback={fallback}>
        <BombComponent shouldExplode={explode} />
      </ErrorBoundary>
      <div class="btn-row" style="margin-top:.75rem">
        <button 
          class="btn-danger btn-sm" 
          onclick={() => setExplode(true)} 
          disabled={explode}
        >
          💣 Throw error
        </button>
      </div>
    </div>
  );
}

// ─── TABS DEMO (useState + dynamic children) ───
const tabContent = {
  Events:  () => <p>🧲 Event delegation via logical-clock proxy. Prevents "click mounts node that immediately receives same click" race (ported from Preact).</p>,
  Hooks:   () => <p>🪝 Full hook surface: useState, useReducer, useEffect, useLayoutEffect, useMemo, useCallback, useRef, useId, useTransition, useDeferredValue, useSyncExternalStore, useImperativeHandle.</p>,
  Router:  () => <p>🛣️ Built-in SPA router with pattern matching, Link component, and hooks: useLocation, useNavigate, useParams, useSearchParams. URL and DOM stay in sync.</p>,
  Context: () => <p>🧩 createContext with Provider / Consumer / useContext(ctx). Subscribers re-render on value changes without prop-drilling.</p>,
};

function TabsCard() {
  const [tab, setTab] = useState('Events');
  const Content = tabContent[tab];

  return (
    <div class="card">
      <span class="hook-pill">useState</span>
      <h3>Feature Tabs</h3>
      <div class="tabs">
        {Object.keys(tabContent).map(t =>
          <button key={t} class={`tab${tab === t ? ' active' : ''}`} onclick={() => setTab(t)}>
            {t}
          </button>
        )}
      </div>
      <div style="font-size:.875rem;line-height:1.6;color:var(--muted)">
        <Content />
      </div>
    </div>
  );
}

// ─── HOME PAGE ───
export default function HomePage({ThemeCtx}) {
  return (
    <main class="page-enter">
      <div class="hero">
        <div>
          <div class="hero-pill" style="margin-bottom:.75rem">
            <span class="dot"></span>live demo
          </div>
          <h1>micro-react</h1>
          <h2>Pure JS · no build step · no dependencies</h2>
        </div>
      </div>

      <div class="card-grid">
        <CounterCard />
        <TimerCard />
      </div>

      <TodoCard />

      <div class="card-grid">
        <ThemeCard ThemeCtx={ThemeCtx} />
        <MemoCard />
      </div>

      <div class="card-grid">
        <RefCard />
        <ErrorBoundaryCard />
      </div>

      <TabsCard />
    </main>
  );
}