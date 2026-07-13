// ─── ABOUT PAGE ───
export default function AboutPage() {
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

  return (
    <main class="page-enter">
      <h1>About micro-react</h1>
      <h2>A 1-file React-compatible runtime for the browser</h2>

      <div class="card">
        <p style="line-height:1.7;color:var(--muted);font-size:.9rem">
          micro-react absorbs the most impactful internals from Preact's source while staying under one self-contained file with zero build requirements. Drop it next to an HTML page, import it as an ES module, and you have a full component model with hooks, context, portals, lazy loading, error boundaries, and a built-in SPA router.
        </p>
      </div>

      <div class="code-block">
<span class="cm">// no build step needed{'\n'}</span>
<span class="kw">import</span> <span class="fn">initWasm</span>, * <span class="kw">as</span> MicroReact <span class="kw">from</span> <span class="str">'./micro-react.js'</span>;{'\n'}
<span class="kw">const</span> {'{'} html, useState, render {'}'} = MicroReact;
<span class="kw">await</span> <span class="fn">initWasm</span>();{'\n'}
<span class="kw">function</span> <span class="fn">App</span>() {'{'}
{'  '}<span class="kw">const</span> [n, setN] = useState(<span class="num">0</span>);
{'  '}<span class="kw">return</span> html<span class="str">`&lt;button onclick="$&#123;() =&gt; setN(n+<span class="num">1</span>)&#125;"&gt;$&#123;n&#125;&lt;/button&gt;`</span>;
{'}'}{'\n'}
const __root = render(html<span class="str">`&lt;$&#123;<span class="fn">App</span>&#125; /&gt;`</span>, document.getElementById(<span class="str">'root'</span>));
<span class="fn">window</span>.__root = __root;
      </div>

      <div class="feature-grid">
        {features.map(f => (
          <div key={f.title} class="feature-item">
            <div class="icon">{f.icon}</div>
            <h4>{f.title}</h4>
            <p>{f.desc}</p>
          </div>
        ))}
      </div>

      <div style="margin-top:2rem">
        <button class="btn-primary" onclick={() => navigate('/')}>← Back to Demo</button>
      </div>
    </main>
  );
}