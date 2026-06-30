const MicroReact = (() => {
  'use strict';

  // ═══════════════════════════════════════════════════════════════════════════
  // 0.  SYMBOLS / CONSTANTS
  // ═══════════════════════════════════════════════════════════════════════════
  const Fragment        = Symbol('MicroReact.Fragment');
  const FORWARD_REF     = Symbol('MicroReact.forwardRef');
  const MEMO_TYPE       = Symbol('MicroReact.memo');
  const PORTAL_TYPE     = Symbol('MicroReact.portal');
  const LAZY_TYPE       = Symbol('MicroReact.lazy');

  const IS_DEV = typeof process === 'undefined' || (typeof process.env !== 'undefined' && process.env.NODE_ENV !== 'production');
  const MAX_HOOKS           = 512;
  const MAX_RENDER_DEPTH    = 256;
  const MAX_SYNC_RERENDERS  = 25;   // setState-in-render guard (mirrors Preact)

  // Vnode flags (bitmask, Preact-style)
  const FLAG_INSERT   = 1 << 0;  // node needs DOM insertion / move
  const FLAG_MATCHED  = 1 << 1;  // node matched an old child during keyed diff

  const SVG_NS  = 'http://www.w3.org/2000/svg';
  const MATH_NS = 'http://www.w3.org/1998/Math/MathML';
  const HTML_NS = 'http://www.w3.org/1999/xhtml';

  const MATHML_TOKENS = /(mi|mn|mo|ms$|mte|msp)/;

  // Preact event-clock: prevents events from firing on nodes that were mounted
  // during the same bubble. Incremented once per dispatched event.
  let eventClock = 0;
  const EVENT_DISPATCHED = '__mrD' + Math.random().toString(36).slice(2);
  const EVENT_ATTACHED   = '__mrA' + Math.random().toString(36).slice(2);

  // ═══════════════════════════════════════════════════════════════════════════
  // 1.  SECURITY
  // ═══════════════════════════════════════════════════════════════════════════
  const BLOCKED_ATTRS  = new Set(['srcdoc']);
  const URL_ATTRS      = new Set(['href', 'src', 'action', 'formaction', 'poster', 'data', 'cite', 'longdesc']);
  const SAFE_URL_RE    = /^(https?:|mailto:|tel:|#|\/|\.\.?\/)/i;
  const UNSAFE_CSS_RE  = /expression\s*\(|url\s*\(/i;

  function sanitizeUrl(v) {
    if (typeof v !== 'string') return '#';
    const s = v.trim().replace(/[\x00-\x20]*/g, '');
    return SAFE_URL_RE.test(s) ? v.trim() : '#';
  }
  function isSafeKey(k) {
    return k !== '__proto__' && k !== 'constructor' && k !== 'prototype';
  }
  function sanitizeCSSVal(prop, val) {
    if (typeof val !== 'string') return val;
    if (UNSAFE_CSS_RE.test(val)) {
      IS_DEV && console.warn(`[MicroReact] Blocked unsafe CSS value "${prop}":`, val);
      return '';
    }
    return val;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 2.  VNODE FACTORY
  // ═══════════════════════════════════════════════════════════════════════════
  // Global monotonic counter — mirrors Preact's _original identity trick.
  // When a vnode is re-used in the same render we clone it; when the same
  // vnode reference appears again with the SAME _original we can bail out.
  let vnodeId = 0;

  function createVNode(type, props, key, ref) {
    return {
      type,
      props,
      key,
      ref,
      // internal bookkeeping
      _children : null,   // child vnode array (set by reconciler)
      _parent   : null,   // parent vnode
      _dom      : null,   // first real DOM node produced by this vnode
      _component: null,   // backing component instance (function-component wrapper)
      _depth    : 0,
      _index    : -1,
      _flags    : 0,
      _original : ++vnodeId,
    };
  }

  function createElement(type, props, ...args) {
    if (type == null) {
      IS_DEV && console.warn('[MicroReact] createElement: null/undefined type');
      type = 'span';
    }
    const safe = Object.create(null);
    if (props) {
      for (const k in props) {
        if (Object.prototype.hasOwnProperty.call(props, k) && isSafeKey(k)) {
          safe[k] = props[k];
        }
      }
    }
    const children = args.flat(Infinity).filter(c => c != null && c !== false && c !== true);
    if (children.length) safe.children = children.length === 1 ? children[0] : children;

    const key = safe.key != null ? String(safe.key) : null;
    const ref = safe.ref ?? null;
    delete safe.key;
    delete safe.ref;

    return createVNode(type, safe, key, ref);
  }

  function isValidElement(v) {
    return v != null && typeof v === 'object' && '_original' in v;
  }

  function cloneElement(el, extra, ...ch) {
    if (!isValidElement(el)) throw new TypeError('[MicroReact] cloneElement: not an element');
    const merged = { ...el.props, ...extra };
    if (ch.length) merged.children = ch.length === 1 ? ch[0] : ch;
    return createElement(el.type, { ...merged, key: el.key, ref: el.ref });
  }

  function createRef() { return { current: null }; }

  // ═══════════════════════════════════════════════════════════════════════════
  // 3.  html TAGGED TEMPLATE  (unchanged from v2)
  // ═══════════════════════════════════════════════════════════════════════════
  // Map of original PascalCase / camelCase tag names captured before DOMParser
  // lowercases them.  Key = lowercased placeholder, value = original string.
  // e.g. "mrtag0" → "MyButton"  (component interpolations)
  // e.g. "mrlit_div" → "div"    (literal tags are already lower, so no-op)
  // We also stash the original attribute name casing for event handlers.

  function html(strings, ...values) {
    // ── 1. Build raw string, replacing interpolations with sentinels ──────────
    // When an interpolation appears right after `<` or `</`, it's a component
    // tag (e.g. html`<${MyButton} />`).  We stash the original value so that
    // resolveTag can return it without casing damage.
    const tagRegistry = {};   // "mrtag{i}" → original value
    const raw = strings.reduce((acc, str, i) => {
      if (i >= values.length) return acc + str;
      const atTag = /<\s*\/?\s*$/.test(str);   // opening OR closing tag position
      if (atTag) {
        const placeholder = `mrtag${i}`;
        tagRegistry[placeholder] = values[i];  // preserve original (function / string)
        return acc + str + placeholder;
      }
      return acc + str + `__VAL${i}__`;
    }, '');

    // ── 2. Expand self-closing component tags so DOMParser can handle them ────
    //    <mrtag0 foo="bar"/>  →  <mrtag0 foo="bar"></mrtag0>
    const norm = raw.replace(/<(mrtag\d+)(\s[^>]*)?\s*\/>/gi, '<$1$2></$1>');

    const doc  = new DOMParser().parseFromString(`<root>${norm}</root>`, 'text/html');

    // ── 3. Resolve a (possibly lowercased) tag name back to the original ──────
    const resolveTag = tag => {
      const lower = tag.toLowerCase();
      if (lower in tagRegistry) return tagRegistry[lower];
      // Literal HTML tag: keep lowercase as the DOM parser gives it
      return lower;
    };

    // ── 4. Resolve an attribute value that may contain __VAL__ sentinels ──────
    const resolveAttr = v => {
      const f = v.match(/^__VAL(\d+)__$/);
      if (f) return values[+f[1]];
      return /__VAL\d+__/.test(v) ? v.replace(/__VAL(\d+)__/g, (_, i) => values[+i]) : v;
    };

    // ── 5. Convert a lowercased attribute name back to camelCase ──────────────
    // DOMParser lowercases all attribute names, so onClick becomes onclick, etc.
    // We reconstruct camelCase for known React-style event / prop names.
    const ATTR_ALIASES = { 'class': 'className', 'for': 'htmlFor' };

    // Pre-built lookup of every lowercase on* name → camelCase equivalent
    // covering the most common React synthetic events.
    const EVENT_CAMEL = (() => {
      const events = [
        'onClick','onDblClick','onMouseDown','onMouseUp','onMouseMove',
        'onMouseEnter','onMouseLeave','onMouseOver','onMouseOut',
        'onChange','onInput','onSubmit','onReset','onFocus','onBlur',
        'onKeyDown','onKeyUp','onKeyPress',
        'onScroll','onWheel',
        'onDrag','onDragStart','onDragEnd','onDragEnter','onDragLeave',
        'onDragOver','onDrop',
        'onPointerDown','onPointerUp','onPointerMove','onPointerEnter',
        'onPointerLeave','onPointerOver','onPointerOut','onPointerCancel',
        'onPointerCapture',
        'onTouchStart','onTouchEnd','onTouchMove','onTouchCancel',
        'onContextMenu','onSelect','onCopy','onCut','onPaste',
        'onAnimationStart','onAnimationEnd','onAnimationIteration',
        'onTransitionEnd','onLoad','onError','onAbort','onCanPlay',
        'onCanPlayThrough','onDurationChange','onEmptied','onEnded',
        'onLoadedData','onLoadedMetadata','onLoadStart','onPause',
        'onPlay','onPlaying','onProgress','onRateChange','onSeeked',
        'onSeeking','onStalled','onSuspend','onTimeUpdate','onVolumeChange',
        'onWaiting','onToggle',
        'onClickCapture','onMouseDownCapture','onMouseUpCapture',
        'onKeyDownCapture','onKeyUpCapture','onFocusCapture','onBlurCapture',
        'onScrollCapture',
      ];
      const map = {};
      for (const e of events) map[e.toLowerCase()] = e;
      return map;
    })();

    // General camelCase restorer for any attr not in the explicit maps:
    // "tabindex" → "tabIndex", "rowspan" → "rowSpan", etc.
    const DOM_CAMEL = {
      'tabindex':'tabIndex','rowspan':'rowSpan','colspan':'colSpan',
      'contenteditable':'contentEditable','crossorigin':'crossOrigin',
      'accesskey':'accessKey','enctype':'encType','usemap':'useMap',
      'maxlength':'maxLength','minlength':'minLength','readonly':'readOnly',
      'autofocus':'autoFocus','autoplay':'autoPlay','autofill':'autoFill',
      'playsinline':'playsInline','spellcheck':'spellCheck',
      'cellpadding':'cellPadding','cellspacing':'cellSpacing',
      'frameborder':'frameBorder','marginheight':'marginHeight',
      'marginwidth':'marginWidth','noresize':'noResize',
      'hreflang':'hrefLang','noshade':'noShade','nowrap':'noWrap',
    };

    const normalizeAttrName = name => {
      if (name in ATTR_ALIASES) return ATTR_ALIASES[name];
      if (name in EVENT_CAMEL)  return EVENT_CAMEL[name];
      if (name in DOM_CAMEL)    return DOM_CAMEL[name];
      return name;
    };

    // ── 6. Recursively walk the parsed DOM tree ───────────────────────────────
    const processNode = node => {
      if (node.nodeType === 3) {
        const text = node.textContent;
        const re = /__VAL(\d+)__/g;
        let li = 0, m, parts = [];
        while ((m = re.exec(text))) {
          if (m.index > li) parts.push(text.slice(li, m.index));
          parts.push(values[+m[1]]);
          li = m.index + m[0].length;
        }
        if (!parts.length) return text;
        if (li < text.length) parts.push(text.slice(li));
        return parts;
      }
      if (node.nodeType !== 1) return null;
      const props = {};
      for (const a of node.attributes) {
        const attrName = normalizeAttrName(a.name);
        const v = resolveAttr(a.value);
        props[attrName] = v;
      }
      const children = [];
      for (const c of node.childNodes) {
        const p = processNode(c);
        if (Array.isArray(p)) children.push(...p);
        else if (p != null && p !== false) children.push(p);
      }
      return createElement(resolveTag(node.tagName), props, ...children);
    };

    const results = [];
    const root = doc.body.firstChild;
    if (root) {
      for (const c of root.childNodes) {
        const p = processNode(c);
        if (Array.isArray(p)) results.push(...p);
        else if (p != null) results.push(p);
      }
    }
    return results.length === 1 ? results[0] : results;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 4.  EVENTS  (Preact logical-clock proxy)
  // ═══════════════════════════════════════════════════════════════════════════
  // Instead of addEventListener per handler, we attach one stable proxy per
  // (element × eventName × capture). The proxy reads the current handler from
  // dom._listeners at dispatch time, so swapping handlers is free (no
  // add/remove needed). The clock guard prevents the "click mounts node that
  // immediately fires click" race.
  const CAPTURE_RE = /(PointerCapture)$|Capture$/i;

  function createEventProxy(useCapture) {
    return function proxyHandler(e) {
      if (!this._listeners) return;
      const handler = this._listeners[e.type + useCapture];
      if (!handler) return;
      if (e[EVENT_DISPATCHED] == null) {
        e[EVENT_DISPATCHED] = eventClock++;
      } else if (e[EVENT_DISPATCHED] < handler[EVENT_ATTACHED]) {
        // Handler was attached after event started bubbling — skip.
        return;
      }
      return handler(e);
    };
  }
  const proxyBubble  = createEventProxy(false);
  const proxyCapture = createEventProxy(true);

  // ═══════════════════════════════════════════════════════════════════════════
  // 5.  DOM PROP HELPERS  (SVG / MathML aware, CSS custom props, clock events)
  // ═══════════════════════════════════════════════════════════════════════════
  const BOOL_ATTRS = new Set([
    'disabled','checked','selected','readonly','multiple','autofocus',
    'autoplay','controls','loop','muted','open','required','reversed','scoped','hidden'
  ]);

  // Props that must be set via property (not setAttribute) in HTML namespace,
  // but must use setAttribute in SVG (matches Preact's exception list).
  const PROP_EXCEPTION = new Set([
    'width','height','href','list','form','tabIndex','download','rowSpan','colSpan','role','popover'
  ]);

  function setDOMProps(dom, newProps, oldProps, ns) {
    // Remove vanished props
    for (const k in oldProps) {
      if (k === 'children' || k === 'key' || k === 'ref') continue;
      if (!isSafeKey(k)) continue;
      if (!(k in newProps)) removeDOMProp(dom, k, oldProps[k], ns);
    }
    // Set / update props
    for (const k in newProps) {
      if (k === 'children' || k === 'key' || k === 'ref') continue;
      if (!isSafeKey(k)) continue;
      setDOMProp(dom, k, newProps[k], oldProps[k], ns);
    }
    // Refs
    if (newProps.ref !== (oldProps && oldProps.ref)) {
      if (oldProps && oldProps.ref) detachRef(oldProps.ref);
      if (newProps.ref)             attachRef(newProps.ref, dom);
    }
  }

  function attachRef(ref, val) {
    if (ref && typeof ref === 'object') ref.current = val;
    else if (typeof ref === 'function') ref(val);
  }
  function detachRef(ref) {
    if (ref && typeof ref === 'object') ref.current = null;
    else if (typeof ref === 'function') ref(null);
  }

  function setDOMProp(dom, key, value, oldValue, ns) {
    if (BLOCKED_ATTRS.has(key)) {
      IS_DEV && console.warn(`[MicroReact] Blocked dangerous prop: "${key}"`);
      return;
    }

    // ── dangerouslySetInnerHTML ──────────────────────────────────────────────
    if (key === 'dangerouslySetInnerHTML') {
      const html = value?.__html;
      dom.innerHTML = typeof html === 'string' ? html : '';
      return;
    }

    // ── className ───────────────────────────────────────────────────────────
    if (key === 'className') {
      // In SVG, className is an SVGAnimatedString — use setAttribute
      if (ns === SVG_NS) dom.setAttribute('class', value ?? '');
      else dom.className = value ?? '';
      return;
    }

    // ── style ────────────────────────────────────────────────────────────────
    if (key === 'style') {
      if (typeof value === 'string') {
        dom.style.cssText = value;
      } else {
        if (typeof oldValue === 'string') dom.style.cssText = '';
        if (oldValue && typeof oldValue === 'object') {
          for (const sk in oldValue) {
            if (!isSafeKey(sk)) continue;
            if (!value || !(sk in value)) {
              // CSS custom property
              if (sk[0] === '-') dom.style.setProperty(sk, '');
              else dom.style[sk] = '';
            }
          }
        }
        if (value && typeof value === 'object') {
          for (const sk in value) {
            if (!isSafeKey(sk)) continue;
            const v = sanitizeCSSVal(sk, value[sk]);
            if (!oldValue || v !== oldValue[sk]) {
              if (sk[0] === '-') dom.style.setProperty(sk, v == null ? '' : v);
              else dom.style[sk] = v == null ? '' : v;
            }
          }
        }
      }
      return;
    }

    // ── events ───────────────────────────────────────────────────────────────
    if (key[0] === 'o' && key[1] === 'n') {
      const useCapture = key !== (key = key.replace(CAPTURE_RE, '$1'));
      const eventName  = key.slice(2).toLowerCase();
      if (!dom._listeners) dom._listeners = {};
      dom._listeners[eventName + useCapture] = value;

      if (value) {
        if (!oldValue) {
          value[EVENT_ATTACHED] = eventClock;
          dom.addEventListener(eventName, useCapture ? proxyCapture : proxyBubble, useCapture);
        } else {
          value[EVENT_ATTACHED] = oldValue[EVENT_ATTACHED];
        }
      } else {
        dom.removeEventListener(eventName, useCapture ? proxyCapture : proxyBubble, useCapture);
      }
      return;
    }

    // ── htmlFor ──────────────────────────────────────────────────────────────
    if (key === 'htmlFor') { dom.htmlFor = value ?? ''; return; }

    // ── value / checked ──────────────────────────────────────────────────────
    if (key === 'value') {
      if (document.activeElement !== dom) dom.value = value ?? '';
      return;
    }
    if (key === 'checked') { dom.checked = !!value; return; }

    // ── defaultValue / defaultChecked ────────────────────────────────────────
    if (key === 'defaultValue') { dom.defaultValue = value ?? ''; return; }
    if (key === 'defaultChecked') { dom.defaultChecked = !!value; return; }

    // ── progress indeterminate ───────────────────────────────────────────────
    if (key === 'value' && dom.tagName === 'PROGRESS' && value == null) {
      dom.removeAttribute('value'); return;
    }

    // ── boolean attrs ────────────────────────────────────────────────────────
    if (BOOL_ATTRS.has(key) && ns !== SVG_NS) {
      if (value) dom.setAttribute(key, '');
      else dom.removeAttribute(key);
      return;
    }

    // ── URL attrs ────────────────────────────────────────────────────────────
    if (URL_ATTRS.has(key)) {
      const safe = sanitizeUrl(value);
      if (safe === '#' && value !== '#')
        IS_DEV && console.warn(`[MicroReact] Sanitised unsafe URL on "${key}":`, value);
      dom.setAttribute(key, safe);
      return;
    }

    // ── SVG namespace normalisation ──────────────────────────────────────────
    if (ns === SVG_NS) {
      key = key.replace(/xlink(H|:h)/, 'h').replace(/sName$/, 's');
    }

    // ── aria-* / data-* — false must NOT remove the attribute ────────────────
    if (typeof value === 'function') {
      // never serialise a function
    } else if (value != null && (value !== false || key[4] === '-')) {
      dom.setAttribute(key, value === true ? '' : String(value));
    } else if (ns !== SVG_NS && !(PROP_EXCEPTION.has(key)) && key in dom) {
      // Fast path: direct property assignment for known DOM props
      try {
        dom[key] = value == null ? '' : value;
      } catch (_) { dom.removeAttribute(key); }
    } else if (value === false || value == null) {
      dom.removeAttribute(key);
    } else {
      dom.setAttribute(key, String(value));
    }
  }

  function removeDOMProp(dom, key, oldValue, ns) {
    if (key[0] === 'o' && key[1] === 'n') {
      const useCapture = key !== (key = key.replace(CAPTURE_RE, '$1'));
      const eventName  = key.slice(2).toLowerCase();
      if (typeof oldValue === 'function') {
        dom.removeEventListener(eventName, useCapture ? proxyCapture : proxyBubble, useCapture);
      }
      if (dom._listeners) delete dom._listeners[eventName + useCapture];
      return;
    }
    if (key === 'className') { ns === SVG_NS ? dom.removeAttribute('class') : (dom.className = ''); return; }
    if (key === 'style') { dom.style.cssText = ''; return; }
    if (key === 'value') { dom.value = ''; return; }
    if (key === 'checked') { dom.checked = false; return; }
    dom.removeAttribute(key);
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 6.  COMPONENT INSTANCES  (thin class-like object, mirrors Preact's approach)
  // ═══════════════════════════════════════════════════════════════════════════
  // Each function-component gets a persistent backing instance with hooks,
  // a dirty flag, and a depth for the scheduler.
  function makeComponentInstance(vnode) {
    return {
      _vnode     : vnode,
      _parentDom : null,
      _hooks     : [],
      _hookIdx   : 0,
      _dirty     : false,
      _unmounted : false,
      _depth     : 0,         // set when inserted into the tree
      _errorSetter: null,     // installed by nearest ErrorBoundary
    };
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 7.  SCHEDULER  (depth-sorted, microtask-batched)
  // ═══════════════════════════════════════════════════════════════════════════
  let currentInstance  = null;
  const pendingEffects       = [];
  const pendingLayoutEffects = [];
  const rerenderQueue  = [];    // array of component instances
  let   rerenderCount  = 0;
  let   isTransition   = false;
  const transitionQueue = [];

  function enqueueRender(inst) {
    if (inst._unmounted || inst._dirty) return;
    inst._dirty = true;
    if (isTransition) {
      transitionQueue.push(inst);
    } else {
      rerenderQueue.push(inst);
      if (!rerenderCount++) queueMicrotask(flushRerenders);
    }
  }

  function flushRerenders() {
    try {
      let l = 1;
      while (rerenderQueue.length) {
        if (rerenderQueue.length > l) {
          // depth-sort: parents first
          rerenderQueue.sort((a, b) => a._depth - b._depth);
        }
        const inst = rerenderQueue.shift();
        l = rerenderQueue.length;
        if (inst._dirty && !inst._unmounted) rerenderComponent(inst);
      }
    } finally {
      rerenderQueue.length = rerenderCount = 0;
    }
    runLayoutEffects();
    runEffects();
    if (transitionQueue.length) {
      requestAnimationFrame(() => {
        const tq = transitionQueue.splice(0);
        for (const inst of tq) {
          if (!inst._unmounted) { inst._dirty = true; rerenderComponent(inst); }
        }
        runLayoutEffects();
        runEffects();
      });
    }
  }

  function rerenderComponent(inst) {
    inst._dirty = false;
    const vnode = inst._vnode;
    const parentDom = inst._parentDom;
    if (!parentDom) return;

    const commitQueue = [];
    const refQueue    = [];
    // shallow-clone the vnode with a bumped _original so diff() sees a change
    const newVNode = { ...vnode, _original: ++vnodeId };
    diffNode(parentDom, newVNode, vnode, parentDom.namespaceURI || HTML_NS, commitQueue, refQueue);
    commitRefs(refQueue);
    commitCallbacks(commitQueue);

    // Patch the parent's _children pointer to the new vnode
    if (vnode._parent?._children) {
      const idx = vnode._parent._children.indexOf(vnode);
      if (idx !== -1) vnode._parent._children[idx] = newVNode;
    }

    runLayoutEffects();
    runEffects();
  }

  function startTransition(fn) {
    const prev = isTransition;
    isTransition = true;
    try { fn(); } finally { isTransition = prev; }
  }

  function flushSync(fn) {
    // Temporarily prevent microtask batching
    const saved = rerenderCount;
    rerenderCount = Infinity;
    try { fn?.(); } finally { rerenderCount = saved; }
    flushRerenders();
  }

  function runLayoutEffects() {
    const q = pendingLayoutEffects.splice(0);
    for (const hook of q) {
      if (typeof hook.cleanup === 'function') { try { hook.cleanup(); } catch(e){ console.error(e); } }
      hook.cleanup = hook._pending ? hook._pending() : undefined;
      hook._pending = null;
    }
  }
  function runEffects() {
    const q = pendingEffects.splice(0);
    for (const hook of q) {
      if (typeof hook.cleanup === 'function') { try { hook.cleanup(); } catch(e){ console.error(e); } }
      hook.cleanup = hook._pending ? hook._pending() : undefined;
      hook._pending = null;
    }
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 8.  HOOKS
  // ═══════════════════════════════════════════════════════════════════════════
  function requireInst(name) {
    if (!currentInstance)
      throw new Error(`[MicroReact] ${name}() called outside a component function.`);
    if (currentInstance._hookIdx >= MAX_HOOKS)
      throw new Error(`[MicroReact] Hook limit (${MAX_HOOKS}) hit in "${currentInstance._vnode?.type?.name || '?'}". Possible conditional hooks.`);
    return currentInstance;
  }

  // ─── useState ─────────────────────────────────────────────────────────────
  function useState(initial) {
    const inst = requireInst('useState');
    const idx  = inst._hookIdx++;
    if (inst._hooks[idx] === undefined) {
      inst._hooks[idx] = { type: 'state', value: typeof initial === 'function' ? initial() : initial };
    }
    const hook = inst._hooks[idx];
    const setState = next => {
      const v = typeof next === 'function' ? next(hook.value) : next;
      if (!Object.is(hook.value, v)) { hook.value = v; enqueueRender(inst); }
    };
    return [hook.value, setState];
  }

  // ─── useReducer ───────────────────────────────────────────────────────────
  function useReducer(reducer, initialArg, init) {
    const inst = requireInst('useReducer');
    const idx  = inst._hookIdx++;
    if (inst._hooks[idx] === undefined) {
      inst._hooks[idx] = { type: 'reducer', value: typeof init === 'function' ? init(initialArg) : initialArg };
    }
    const hook = inst._hooks[idx];
    const dispatch = action => {
      const v = reducer(hook.value, action);
      if (!Object.is(hook.value, v)) { hook.value = v; enqueueRender(inst); }
    };
    return [hook.value, dispatch];
  }

  // ─── useEffect ────────────────────────────────────────────────────────────
  function _scheduleEffect(queue, name, callback, deps) {
    const inst = requireInst(name);
    const idx  = inst._hookIdx++;
    const prev = inst._hooks[idx];
    const changed = !prev || !deps || !prev.deps || deps.some((d, i) => !Object.is(d, prev.deps[i]));
    const hook = prev || { type: name, cleanup: undefined, deps: undefined, _pending: null };
    if (changed) { hook._pending = callback; hook.deps = deps; queue.push(hook); }
    inst._hooks[idx] = hook;
  }
  function useEffect(cb, deps)       { _scheduleEffect(pendingEffects,       'useEffect',       cb, deps); }
  function useLayoutEffect(cb, deps) { _scheduleEffect(pendingLayoutEffects,  'useLayoutEffect', cb, deps); }

  // ─── useRef ───────────────────────────────────────────────────────────────
  function useRef(initial) {
    const inst = requireInst('useRef');
    const idx  = inst._hookIdx++;
    if (inst._hooks[idx] === undefined) inst._hooks[idx] = { type: 'ref', current: initial };
    return inst._hooks[idx];
  }

  // ─── useMemo / useCallback ────────────────────────────────────────────────
  function useMemo(factory, deps) {
    const inst = requireInst('useMemo');
    const idx  = inst._hookIdx++;
    const prev = inst._hooks[idx];
    const changed = !prev || !deps || !prev.deps || deps.some((d, i) => !Object.is(d, prev.deps[i]));
    const hook = prev || { type: 'memo' };
    if (changed) { hook.value = factory(); hook.deps = deps; }
    inst._hooks[idx] = hook;
    return hook.value;
  }
  function useCallback(fn, deps) { return useMemo(() => fn, deps); }

  // ─── useId ────────────────────────────────────────────────────────────────
  let _idSeq = 0;
  function useId() {
    const inst = requireInst('useId');
    const idx  = inst._hookIdx++;
    if (inst._hooks[idx] === undefined) inst._hooks[idx] = { type: 'id', value: `mr-${++_idSeq}` };
    return inst._hooks[idx].value;
  }

  // ─── useDebugValue ────────────────────────────────────────────────────────
  function useDebugValue(v, fmt) {
    requireInst('useDebugValue');
    // no-op in prod, noop but valid slot consumed
  }

  // ─── useImperativeHandle ──────────────────────────────────────────────────
  function useImperativeHandle(ref, create, deps) {
    useLayoutEffect(() => {
      const h = create();
      attachRef(ref, h);
      return () => detachRef(ref);
    }, deps);
  }

  // ─── useDeferredValue ─────────────────────────────────────────────────────
  function useDeferredValue(value) {
    const [deferred, set] = useState(value);
    useEffect(() => { startTransition(() => set(value)); }, [value]);
    return deferred;
  }

  // ─── useTransition ────────────────────────────────────────────────────────
  function useTransition() {
    const [pending, setPending] = useState(false);
    const start = useCallback(fn => {
      setPending(true);
      startTransition(() => { fn(); setPending(false); });
    }, []);
    return [pending, start];
  }

  // ─── useErrorBoundary ─────────────────────────────────────────────────────
  function useErrorBoundary() {
    const [err, setErr] = useState(null);
    const reset = useCallback(() => setErr(null), []);
    return [err, reset];
  }

  // ─── useSyncExternalStore ─────────────────────────────────────────────────
  function useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot) {
    requireInst('useSyncExternalStore');
    const [snap, setSnap] = useState(() =>
      typeof getServerSnapshot === 'function' ? getServerSnapshot() : getSnapshot()
    );
    useEffect(() => {
      const check = () => {
        const next = getSnapshot();
        setSnap(prev => Object.is(prev, next) ? prev : next);
      };
      const unsub = subscribe(check);
      check();
      return () => typeof unsub === 'function' && unsub();
    }, [subscribe, getSnapshot]);
    return snap;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 9.  CONTEXT
  // ═══════════════════════════════════════════════════════════════════════════
  let ctxIdSeq = 0;
  function createContext(defaultValue) {
    const id = '__mrC' + ctxIdSeq++;
    const ctx = { _id: id, _defaultValue: defaultValue };

    // Provider is a function component. It stores its value in globalContext.
    function Provider({ value, children }) {
      // Notify subscribers when value changes
      useEffect(() => {
        ctx._currentValue = value;
        ctx._listeners?.forEach(fn => fn(value));
      });
      ctx._currentValue = value;
      return children ?? null;
    }
    Provider.displayName = `Context.Provider`;
    ctx.Provider = Provider;

    // Consumer (render-prop style)
    ctx.Consumer = ({ children }) => children(ctx._currentValue ?? defaultValue);
    ctx.Consumer.displayName = `Context.Consumer`;

    ctx._listeners = new Set();
    ctx._currentValue = defaultValue;

    function useContext() {
      const [, forceUpdate] = useState(0);
      useEffect(() => {
        const fn = () => forceUpdate(n => n + 1);
        ctx._listeners.add(fn);
        return () => ctx._listeners.delete(fn);
      }, []);
      return ctx._currentValue ?? defaultValue;
    }

    ctx.useContext = useContext;
    return ctx;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 10.  PORTALS
  // ═══════════════════════════════════════════════════════════════════════════
  function createPortal(children, container) {
    if (!(container instanceof Element)) {
      IS_DEV && console.error('[MicroReact] createPortal: container must be an Element');
      return null;
    }
    return createVNode(PORTAL_TYPE, { children, container }, null, null);
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 11.  LAZY
  // ═══════════════════════════════════════════════════════════════════════════
  function lazy(factory) {
    let status = 'pending', result = null, promise = null;
    const Lazy = props => {
      if (status === 'resolved') return createElement(result, props);
      if (status === 'rejected') throw result;
      if (!promise) {
        promise = factory().then(
          mod => { status = 'resolved'; result = mod.default ?? mod; },
          err  => { status = 'rejected'; result = err; }
        );
      }
      const [, update] = useState(0);
      useEffect(() => { if (status === 'pending') promise.then(() => update(n => n + 1)); }, []);
      return null;
    };
    Lazy._type = LAZY_TYPE;
    Lazy.displayName = 'Lazy';
    return Lazy;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 12.  memo / forwardRef HOCs
  // ═══════════════════════════════════════════════════════════════════════════
  function shallowEqual(a, b) {
    if (Object.is(a, b)) return true;
    if (!a || !b || typeof a !== 'object' || typeof b !== 'object') return false;
    const ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every(k => Object.is(a[k], b[k]));
  }

  function memo(Comp, compare = shallowEqual) {
    const Wrapper = props => {
      const prevRef = useRef(null);
      const resRef  = useRef(null);
      const skip = prevRef.current !== null && compare(prevRef.current, props);
      if (!skip) { prevRef.current = props; resRef.current = Comp(props); }
      return resRef.current;
    };
    Wrapper._type       = MEMO_TYPE;
    Wrapper._inner      = Comp;
    Wrapper.displayName = `Memo(${Comp.displayName || Comp.name || 'Component'})`;
    return Wrapper;
  }

  function forwardRef(render) {
    const Wrapper = props => render(props, props.ref ?? null);
    Wrapper._type       = FORWARD_REF;
    Wrapper._render     = render;
    Wrapper.displayName = `ForwardRef(${render.displayName || render.name || 'Component'})`;
    return Wrapper;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 13.  ErrorBoundary
  // ═══════════════════════════════════════════════════════════════════════════
  function ErrorBoundary({ fallback, onError, children }) {
    const [error, setError] = useState(null);
    const inst = currentInstance;
    if (inst) {
      inst._errorSetter = err => {
        IS_DEV && console.error('[MicroReact] ErrorBoundary caught:', err);
        onError?.(err);
        setError(err);
      };
    }
    if (error) return typeof fallback === 'function' ? fallback(error) : (fallback ?? null);
    return children ?? null;
  }
  ErrorBoundary.displayName = 'ErrorBoundary';

  // ═══════════════════════════════════════════════════════════════════════════
  // 14.  Children helpers
  // ═══════════════════════════════════════════════════════════════════════════
  function flattenChildren(c) {
    return [c].flat(Infinity).filter(x => x != null && x !== false && x !== true);
  }
  const Children = {
    map    : (c, fn) => flattenChildren(c).map(fn),
    forEach: (c, fn) => flattenChildren(c).forEach(fn),
    count  : c => flattenChildren(c).length,
    only   : c => {
      const a = flattenChildren(c);
      if (a.length !== 1) throw new Error('[MicroReact] Children.only: expected exactly one child');
      return a[0];
    },
    toArray: c => flattenChildren(c),
  };

  // ═══════════════════════════════════════════════════════════════════════════
  // 15.  DIFF ENGINE  (Preact-style, VNode-based)
  // ═══════════════════════════════════════════════════════════════════════════

  // ─── getDomSibling ────────────────────────────────────────────────────────
  // Walk up and across the parent tree to find the next real DOM node.
  // Used to determine the `before` anchor for insertBefore calls.
  function getDomSibling(vnode, startIdx) {
    if (startIdx == null) {
      return vnode._parent ? getDomSibling(vnode._parent, vnode._index + 1) : null;
    }
    const children = vnode._children;
    if (!children) return null;
    for (let i = startIdx; i < children.length; i++) {
      const sib = children[i];
      if (sib != null && sib._dom != null) return sib._dom;
    }
    return typeof vnode.type === 'function' ? getDomSibling(vnode) : null;
  }

  // ─── unmountVNode ─────────────────────────────────────────────────────────
  function unmountVNode(vnode, skipRemove) {
    if (!vnode) return;
    // Run effect cleanups
    const inst = vnode._component;
    if (inst) {
      for (const hook of inst._hooks) {
        if (!hook) continue;
        if ((hook.type === 'useEffect' || hook.type === 'useLayoutEffect') && typeof hook.cleanup === 'function') {
          try { hook.cleanup(); } catch(e){ console.error('[MicroReact] cleanup error:', e); }
        }
      }
      inst._unmounted = true;
      inst._parentDom = null;
    }
    if (vnode.ref && (!vnode.ref.current || vnode.ref.current === vnode._dom)) {
      detachRef(vnode.ref);
    }
    if (vnode._children) {
      for (const child of vnode._children) {
        // Only a real host element (string type) has a single DOM node whose
        // removal cascades to all its children. Function components have no
        // DOM node of their own, and Fragments/Portals (Symbol type) render
        // multiple independent top-level siblings -- removing one does NOT
        // remove the others, so we must NOT skip their individual removal.
        if (child) unmountVNode(child, skipRemove || typeof vnode.type === 'string');
      }
    }
    if (!skipRemove && vnode._dom) {
      if (vnode._dom.parentNode) vnode._dom.remove();
    }
    if (vnode._dom?._listeners) vnode._dom._listeners = null;
    vnode._dom = vnode._component = vnode._parent = null;
  }

  // ─── commitRefs / commitCallbacks ─────────────────────────────────────────
  function commitRefs(refQueue) {
    for (let i = 0; i < refQueue.length; i += 3) {
      const ref   = refQueue[i];
      const value = refQueue[i + 1];
      const vnode = refQueue[i + 2];
      try { attachRef(ref, value); } catch(e){ console.error('[MicroReact] ref error:', e, vnode); }
    }
  }
  function commitCallbacks(queue) {
    for (const inst of queue) {
      if (inst._callbacks) {
        const cbs = inst._callbacks;
        inst._callbacks = [];
        for (const cb of cbs) { try { cb.call(inst); } catch(e){ console.error(e); } }
      }
    }
  }

  // ─── render depth guard ───────────────────────────────────────────────────
  let _renderDepth = 0;

  // ─── diffNode ─────────────────────────────────────────────────────────────
  // The main recursive diff function. Returns the first DOM node produced.
  function diffNode(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue) {
    if (++_renderDepth > MAX_RENDER_DEPTH) {
      _renderDepth = 0;
      throw new Error('[MicroReact] Max render depth exceeded. Possible infinite loop.');
    }
    try {
      return _diffNode(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue);
    } finally {
      _renderDepth--;
    }
  }

  function _diffNode(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue) {
    const newType = newVNode.type;
    const oldProps = (oldVNode?.props) ?? {};

    // ── text node ──────────────────────────────────────────────────────────
    if (newType == null) {
      // Text vnode: props is the text string
      const text = String(newVNode.props);
      if (oldVNode?._dom?.nodeType === 3) {
        if (oldVNode._dom.data !== text) oldVNode._dom.data = text;
        newVNode._dom = oldVNode._dom;
      } else {
        newVNode._dom = document.createTextNode(text);
      }
      return newVNode._dom;
    }

    // ── portal ─────────────────────────────────────────────────────────────
    if (newType === PORTAL_TYPE) {
      const { container, children } = newVNode.props;
      const normalized = flattenChildren([children]).map(normalizeChild);
      const oldChildren = oldVNode?._children ?? [];
      diffChildren(container, normalized, newVNode, oldVNode, ns, null, commitQueue, refQueue);
      newVNode._dom = null;
      return null;
    }

    // ── fragment ───────────────────────────────────────────────────────────
    if (newType === Fragment) {
      const children = newVNode.props?.children;
      const normalized = flattenChildren(Array.isArray(children) ? children : [children]).map(normalizeChild).filter(Boolean);
      diffChildren(parentDom, normalized, newVNode, oldVNode, ns, null, commitQueue, refQueue);
      // Fragments have no DOM node of their own; expose the first child's node
      // so callers (diffChildren anchor logic) can find a real DOM sibling.
      newVNode._dom = newVNode._children?.[0]?._dom ?? null;
      return newVNode._dom;
    }

    // ── function / component ───────────────────────────────────────────────
    if (typeof newType === 'function') {
      return diffComponent(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue);
    }

    // ── unknown symbol / unrecognised type — skip safely ──────────────────
    if (typeof newType === 'symbol') {
      IS_DEV && console.warn('[MicroReact] Unknown symbol vnode type:', newType.toString());
      newVNode._dom = null;
      return null;
    }

    // ── host element ───────────────────────────────────────────────────────
    return diffElement(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue);
  }

  // ─── diffComponent ────────────────────────────────────────────────────────
  function diffComponent(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue) {
    const type = newVNode.type;
    let inst = oldVNode?._component;

    if (!inst) {
      inst = makeComponentInstance(newVNode);
      newVNode._component = inst;
    } else {
      newVNode._component = inst;
      inst._vnode = newVNode;
    }
    inst._parentDom = parentDom;
    inst._depth     = newVNode._depth;

    // Bail-out via _original identity (like shouldComponentUpdate === false)
    // If the vnode reference is identical and the instance isn't dirty, skip.
    if (oldVNode && newVNode._original === oldVNode._original && !inst._dirty) {
      newVNode._children = oldVNode._children;
      newVNode._dom      = oldVNode._dom;
      if (newVNode._children) {
        for (const child of newVNode._children) { if (child) child._parent = newVNode; }
      }
      return newVNode._dom;
    }

    inst._dirty  = false;
    inst._hookIdx = 0;

    const prev = currentInstance;
    currentInstance = inst;
    let renderResult;
    let syncCount = 0;
    try {
      do {
        inst._dirty = false;
        renderResult = type(newVNode.props ?? {});
        // If component called setState during render, re-run (up to limit)
      } while (inst._dirty && ++syncCount < MAX_SYNC_RERENDERS);
    } catch (err) {
      currentInstance = prev;
      const eb = findErrorBoundary(newVNode);
      if (eb) { eb(err); newVNode._dom = oldVNode?._dom ?? null; return newVNode._dom; }
      throw err;
    }
    currentInstance = prev;

    const renderArray = normalizeRenderResult(renderResult);
    diffChildren(parentDom, renderArray, newVNode, oldVNode, ns, null, commitQueue, refQueue);
    return newVNode._dom;
  }

  function findErrorBoundary(vnode) {
    let v = vnode._parent;
    while (v) {
      if (v._component?._errorSetter) return v._component._errorSetter;
      v = v._parent;
    }
    return null;
  }

  function normalizeRenderResult(result) {
    if (result == null || result === false || result === true) return [];
    if (Array.isArray(result)) return result.map(normalizeChild).filter(Boolean);
    return [normalizeChild(result)].filter(Boolean);
  }

  function normalizeChild(child) {
    if (child == null || child === false || child === true) return null;
    if (typeof child === 'string' || typeof child === 'number' || typeof child === 'bigint') {
      return createVNode(null, String(child), null, null);
    }
    if (Array.isArray(child)) {
      return createVNode(Fragment, { children: child }, null, null);
    }
    if (isValidElement(child)) {
      // Detect reused vnode (same _original, depth already set) → clone
      if (child._depth > 0) {
        return createVNode(child.type, child.props, child.key, child.ref);
      }
      return child;
    }
    return null;
  }

  // ─── diffElement ──────────────────────────────────────────────────────────
  function diffElement(parentDom, newVNode, oldVNode, ns, commitQueue, refQueue) {
    const type     = newVNode.type;
    const newProps = newVNode.props ?? {};
    const oldProps = oldVNode?.props ?? {};
    let   dom      = oldVNode?._dom;

    // Namespace propagation (Preact-style)
    if (type === 'svg') ns = SVG_NS;
    else if (type === 'math') ns = MATH_NS;
    else if (!ns || ns === SVG_NS) {
      if (type === 'foreignObject') ns = HTML_NS;
      else if (ns !== SVG_NS && ns !== MATH_NS) ns = HTML_NS;
    }
    if (type === 'foreignObject' || (ns === MATH_NS && MATHML_TOKENS.test(type))) {
      ns = HTML_NS;
    }

    // Reuse or create DOM element
    if (!dom || dom.localName !== type) {
      dom = document.createElementNS(ns ?? HTML_NS, type, newProps.is ? { is: newProps.is } : undefined);
      // New parent: no old children to reuse
      if (oldVNode) unmountVNode(oldVNode, true);
    }

    newVNode._dom = dom;

    // Handle dangerouslySetInnerHTML vs children
    const newHtml = newProps.dangerouslySetInnerHTML;
    const oldHtml = oldProps.dangerouslySetInnerHTML;
    if (newHtml) {
      if (!oldHtml || newHtml.__html !== oldHtml.__html) dom.innerHTML = newHtml.__html ?? '';
      newVNode._children = [];
    } else {
      if (oldHtml) dom.innerHTML = '';
      const childNs = type === 'foreignObject' ? HTML_NS : ns;
      const rawChildren = newProps.children != null
        ? (Array.isArray(newProps.children) ? newProps.children : [newProps.children])
        : [];
      const normalized = rawChildren.map(normalizeChild).filter(Boolean);
      diffChildren(type === 'template' ? dom.content : dom, normalized, newVNode, oldVNode, childNs, null, commitQueue, refQueue);
      // diffChildren sets newVNode._dom to the first CHILD's dom (correct for
      // Fragment/component parents, which have no DOM node of their own).
      // For a real host element we must restore _dom to the element itself,
      // otherwise refs/insertion logic upstream end up moving/inserting a
      // descendant text node instead of this element.
      newVNode._dom = dom;
    }

    // Props (skip children / dangerouslySetInnerHTML, already handled)
    setDOMProps(dom, newProps, oldProps, ns);

    // value / checked applied after children (Preact order)
    if ('value' in newProps && newProps.value !== undefined) {
      if (type === 'progress' && newProps.value == null) dom.removeAttribute('value');
      else if (newProps.value !== dom.value || type === 'progress') {
        setDOMProp(dom, 'value', newProps.value, oldProps.value, ns);
      }
    }
    if ('checked' in newProps && newProps.checked !== undefined) {
      setDOMProp(dom, 'checked', newProps.checked, oldProps.checked, ns);
    }

    // Ref queue
    if (newVNode.ref && oldVNode?.ref !== newVNode.ref) {
      if (oldVNode?.ref) refQueue.push(oldVNode.ref, null, newVNode);
      refQueue.push(newVNode.ref, dom, newVNode);
    }

    return dom;
  }

  // ─── diffChildren  (Preact skew algorithm) ────────────────────────────────
  function diffChildren(parentDom, renderResult, newParent, oldParent, ns, excessDom, commitQueue, refQueue) {
    const oldChildren = oldParent?._children ?? [];
    const newLen = renderResult.length;

    // Phase 1: assign parent/depth, match old children (skew-based)
    newParent._children = new Array(newLen);
    let skew = 0;
    let remainingOld = oldChildren.length;

    for (let i = 0; i < newLen; i++) {
      let childVNode = renderResult[i];
      if (!childVNode) { newParent._children[i] = null; continue; }

      childVNode._parent = newParent;
      childVNode._depth  = newParent._depth + 1;
      newParent._children[i] = childVNode;

      const skewedIdx = i + skew;
      const matchIdx  = findMatch(childVNode, oldChildren, skewedIdx, remainingOld);
      childVNode._index = matchIdx;  // temporary: holds matchIdx

      let oldVNode = null;
      if (matchIdx !== -1) {
        oldVNode = oldChildren[matchIdx];
        remainingOld--;
        if (oldVNode) oldVNode._flags |= FLAG_MATCHED;
      }

      // Determine if node needs insertion
      const isMounting = !oldVNode || oldVNode._original == null;
      if (isMounting) {
        if (matchIdx === -1) {
          // Growing array
          if (newLen > oldChildren.length) skew--;
          else if (newLen < oldChildren.length) skew++;
        }
        // Only real host elements (tag strings) and text nodes (type null) are
        // single, directly-insertable DOM nodes. Function components have no
        // DOM node of their own, and Fragments/Portals (Symbol type) represent
        // *multiple* independent top-level nodes -- flagging them for direct
        // insertion makes insertVNode move only their borrowed "first child"
        // stand-in dom, yanking it away from its already-correctly-placed
        // siblings. Their real insertion is handled individually by their
        // host-element descendants further down the tree.
        if (typeof childVNode.type !== 'function' && typeof childVNode.type !== 'symbol') childVNode._flags |= FLAG_INSERT;
      } else if (matchIdx !== skewedIdx) {
        if      (matchIdx === skewedIdx - 1) skew--;
        else if (matchIdx === skewedIdx + 1) skew++;
        else {
          if (matchIdx > skewedIdx) skew--; else skew++;
          if (typeof childVNode.type !== 'function' && typeof childVNode.type !== 'symbol') childVNode._flags |= FLAG_INSERT;
        }
      }
    }

    // Phase 2: actually diff each child
    let oldDom = oldChildren.length > 0 && oldChildren[0]?._dom
      ? oldChildren[0]._dom
      : (excessDom ?? null);

    let firstChildDom = null;

    for (let i = 0; i < newLen; i++) {
      const childVNode = newParent._children[i];
      if (!childVNode) continue;

      const matchIdx = childVNode._index;
      childVNode._index = i;  // final index

      const oldVNode = matchIdx !== -1 && oldChildren[matchIdx]
        ? oldChildren[matchIdx]
        : null;

      let resultDom;
      try {
        resultDom = diffNode(parentDom, childVNode, oldVNode, ns, commitQueue, refQueue);
      } catch (err) {
        const eb = findErrorBoundary(childVNode);
        if (eb) { eb(err); resultDom = oldVNode?._dom ?? null; }
        else throw err;
      }

      const newDom = childVNode._dom;
      if (firstChildDom == null && newDom != null) firstChildDom = newDom;

      // Handle ref changes on component children.
      // Host element refs are already committed inside diffElement; only handle
      // function-component vnodes here so we don't double-queue or wrongly set
      // ref.current to a component instance instead of a DOM node.
      if (childVNode.ref && typeof childVNode.type === 'function' && oldVNode?.ref !== childVNode.ref) {
        if (oldVNode?.ref) refQueue.push(oldVNode.ref, null, childVNode);
        // For a forwarded-ref component the ref should point at the DOM root, not the instance.
        refQueue.push(childVNode.ref, newDom, childVNode);
      }

      const shouldInsert = childVNode._flags & FLAG_INSERT;
      if (shouldInsert || (oldVNode && oldVNode._children === childVNode._children)) {
        oldDom = insertVNode(childVNode, oldDom, parentDom, shouldInsert);
        if (shouldInsert && oldVNode?._dom) oldVNode._dom = null;
      } else if (typeof childVNode.type === 'function' && resultDom !== undefined) {
        // BUG FIX: using `resultDom` itself as the next anchor caused the
        // *following* sibling to be inserted BEFORE this component's content
        // instead of after it (since insertBefore(node, anchor) places node
        // ahead of anchor). getDomSibling() correctly walks to whatever comes
        // immediately after this vnode's rendered output, fragment-children
        // and nested components included.
        oldDom = getDomSibling(childVNode);
      } else if (newDom) {
        oldDom = newDom.nextSibling;
      }

      childVNode._flags &= ~(FLAG_INSERT | FLAG_MATCHED);
    }

    newParent._dom = firstChildDom;

    // Phase 3: unmount leftover old children
    for (let i = 0; i < oldChildren.length; i++) {
      const old = oldChildren[i];
      if (old && !(old._flags & FLAG_MATCHED)) {
        if (old._dom === oldDom) oldDom = getDomSibling(old);
        unmountVNode(old, false);
      }
    }

    return oldDom;
  }

  // ─── findMatch ────────────────────────────────────────────────────────────
  // Bidirectional search centred on skewedIndex, same logic as Preact.
  function findMatch(childVNode, oldChildren, skewedIndex, remaining) {
    const key  = childVNode.key;
    const type = childVNode.type;
    const atIdx = oldChildren[skewedIndex];
    const atIdxFree = atIdx != null && !(atIdx._flags & FLAG_MATCHED);

    if ((atIdx == null && key == null) || (atIdxFree && key === atIdx.key && type === atIdx.type)) {
      return skewedIndex;
    }

    const shouldSearch = remaining > (atIdxFree ? 1 : 0);
    if (!shouldSearch) return -1;

    let lo = skewedIndex - 1, hi = skewedIndex + 1;
    while (lo >= 0 || hi < oldChildren.length) {
      const ci = lo >= 0 ? lo-- : hi++;
      const old = oldChildren[ci];
      if (old != null && !(old._flags & FLAG_MATCHED) && key === old.key && type === old.type) {
        return ci;
      }
    }
    return -1;
  }

  // ─── insertVNode ──────────────────────────────────────────────────────────
  // Recursively places a vnode's DOM node(s) before `oldDom` in `parentDom`.
  function insertVNode(parentVNode, oldDom, parentDom, shouldPlace) {
    if (typeof parentVNode.type === 'function') {
      const children = parentVNode._children;
      if (children) {
        for (let i = 0; i < children.length; i++) {
          if (children[i]) {
            children[i]._parent = parentVNode;
            oldDom = insertVNode(children[i], oldDom, parentDom, shouldPlace);
          }
        }
      }
      return oldDom;
    }
    if (parentVNode._dom !== oldDom) {
      if (shouldPlace) {
        if (oldDom && parentVNode.type && !oldDom.parentNode) {
          oldDom = getDomSibling(parentVNode);
        }
        parentDom.insertBefore(parentVNode._dom, oldDom ?? null);
      }
      oldDom = parentVNode._dom;
    }
    // Skip past comment nodes (used as anchors)
    do { oldDom = oldDom && oldDom.nextSibling; }
    while (oldDom != null && oldDom.nodeType === 8);

    return oldDom;
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 16.  ROUTER  (full pattern-matching, same API as v2)
  // ═══════════════════════════════════════════════════════════════════════════
  const RouterCtx = createContext({ path: '/', params: {}, search: '', navigate: () => {} });

  function compilePattern(pattern) {
    const names = [];
    const re = new RegExp(
      '^' +
      pattern
        .replace(/:[a-zA-Z_]\w*/g, m => { names.push(m.slice(1)); return '([^/]+)'; })
        .replace(/\*/g, '.*') +
      '(?:\\/)?$'
    );
    return { re, names };
  }
  function matchRoute(pattern, pathname) {
    const { re, names } = compilePattern(pattern);
    const m = pathname.match(re);
    if (!m) return null;
    const params = Object.create(null);
    names.forEach((n, i) => { params[n] = decodeURIComponent(m[i + 1]); });
    return params;
  }

  function Router({ routes, notFound }) {
    const [loc, setLoc] = useState(() => ({
      path: window.location.pathname,
      search: window.location.search,
    }));
    useEffect(() => {
      const fn = () => setLoc({ path: window.location.pathname, search: window.location.search });
      window.addEventListener('popstate', fn);
      return () => window.removeEventListener('popstate', fn);
    }, []);
    const navigate = useCallback((to, { replace = false } = {}) => {
      if (replace) window.history.replaceState({}, '', to);
      else window.history.pushState({}, '', to);
      const url = new URL(to, window.location.origin);
      setLoc({ path: url.pathname, search: url.search });
    }, []);
    let Comp = null, params = {};
    for (const [pattern, C] of Object.entries(routes)) {
      const p = matchRoute(pattern, loc.path);
      if (p !== null) { Comp = C; params = p; break; }
    }
    Comp = Comp ?? notFound ?? (() => createElement('div', null, '404 Not Found'));
    return createElement(
      RouterCtx.Provider,
      { value: { path: loc.path, params, search: loc.search, navigate } },
      createElement(Comp, null)
    );
  }

  function Link({ to, replace, children, ...rest }) {
    const { navigate } = RouterCtx.useContext();
    return createElement('a', {
      ...rest,
      href: to,
      onClick(e) {
        if (!e.defaultPrevented && e.button === 0 && !e.metaKey && !e.altKey && !e.ctrlKey && !e.shiftKey) {
          e.preventDefault();
          navigate(to, { replace });
        }
        rest.onClick?.(e);
      },
    }, children);
  }
  Link.displayName = 'Link';

  function useLocation()   { return RouterCtx.useContext(); }
  function useParams()     { return RouterCtx.useContext().params; }
  function useNavigate()   { return RouterCtx.useContext().navigate; }
  function useSearchParams() {
    const { search, navigate, path } = RouterCtx.useContext();
    const params = new URLSearchParams(search);
    const setParams = upd => {
      const next = typeof upd === 'function' ? upd(params) : upd;
      navigate(`${path}?${next.toString()}`);
    };
    return [params, setParams];
  }

  // v1-compat simple useRouter
  function useRouter(routes) {
    const [path, setPath] = useState(window.location.pathname);
    useEffect(() => {
      const fn = () => setPath(window.location.pathname);
      window.addEventListener('popstate', fn);
      return () => window.removeEventListener('popstate', fn);
    }, []);
    const navigate = to => { window.history.pushState({}, '', to); setPath(to); };
    let Comp = routes['*'] || (() => createElement('div', null, '404'));
    for (const [pat, C] of Object.entries(routes)) {
      if (matchRoute(pat, path)) { Comp = C; break; }
    }
    return { Component: Comp, path, navigate };
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 17.  StrictMode
  // ═══════════════════════════════════════════════════════════════════════════
  function StrictMode({ children }) { return children ?? null; }
  StrictMode.displayName = 'StrictMode';

  // ═══════════════════════════════════════════════════════════════════════════
  // 18.  RENDER / createRoot
  // ═══════════════════════════════════════════════════════════════════════════
  function createRoot(container) {
    if (!(container instanceof Element))
      throw new TypeError('[MicroReact] createRoot: container must be a DOM Element');

    // The root is modelled as a synthetic Fragment vnode wrapping user content
    let _rootVNode = null;

    return {
      render(vnode) {
        const newRoot = createVNode(Fragment, { children: vnode }, null, null);
        newRoot._depth = 0;
        const commitQueue = [];
        const refQueue    = [];
        if (_rootVNode) {
          diffNode(container, newRoot, _rootVNode, container.namespaceURI || HTML_NS, commitQueue, refQueue);
        } else {
          container.innerHTML = '';
          const normalized = normalizeRenderResult(vnode);
          diffChildren(container, normalized, newRoot, null, container.namespaceURI || HTML_NS, null, commitQueue, refQueue);
          getDomNodes(newRoot).forEach(n => {
            if (!n.parentNode) container.appendChild(n);
          });
        }
        commitRefs(refQueue);
        commitCallbacks(commitQueue);
        runLayoutEffects();
        runEffects();
        _rootVNode = newRoot;
        container._mrRoot = this;
      },
      unmount() {
        if (_rootVNode) {
          unmountVNode(_rootVNode, false);
          container.innerHTML = '';
          _rootVNode = null;
          delete container._mrRoot;
        }
      },
    };
  }

  // Convenience: collect all DOM nodes from a vnode tree
  function getDomNodes(vnode) {
    if (!vnode) return [];
    if (vnode._dom) return [vnode._dom];
    if (vnode._children) return vnode._children.flatMap(c => c ? getDomNodes(c) : []);
    return [];
  }

  function render(vnode, container) {
    if (!(container instanceof Element))
      throw new TypeError('[MicroReact] render: container must be a DOM Element');
    if (container._mrRoot) container._mrRoot.unmount();
    const root = createRoot(container);
    root.render(vnode);
    return root;
  }

  function hydrate(vnode, container) {
    IS_DEV && console.warn('[MicroReact] hydrate(): falling back to full render.');
    return render(vnode, container);
  }

  // ═══════════════════════════════════════════════════════════════════════════
  // 19.  PUBLIC SURFACE
  // ═══════════════════════════════════════════════════════════════════════════
  return Object.freeze({
    // Core
    createElement, cloneElement, isValidElement,
    Fragment, html, Children,

    // Rendering
    render, createRoot, hydrate, flushSync, startTransition,

    // HOCs
    memo, forwardRef, lazy,

    // Refs
    createRef,

    // Hooks
    useState, useReducer,
    useEffect, useLayoutEffect,
    useRef, useMemo, useCallback,
    useId, useDebugValue,
    useImperativeHandle,
    useDeferredValue, useTransition,
    useErrorBoundary, useSyncExternalStore,

    // Context
    createContext,

    // Portals
    createPortal,

    // Composites
    ErrorBoundary, StrictMode,
    Suspense: StrictMode,  // placeholder

    // Router
    Router, Link,
    useRouter, useLocation, useParams, useNavigate, useSearchParams,
  });
})();

// ─── Top-level named exports ─────────────────────────────────────────────────
const html = MicroReact.html;
export default MicroReact;
export const {
  createElement, cloneElement, isValidElement,
  Fragment, Children,
  render, createRoot, hydrate, flushSync, startTransition,
  memo, forwardRef, lazy,
  createRef,
  useState, useReducer,
  useEffect, useLayoutEffect,
  useRef, useMemo, useCallback,
  useId, useDebugValue,
  useImperativeHandle,
  useDeferredValue, useTransition,
  useErrorBoundary, useSyncExternalStore,
  createContext, createPortal,
  ErrorBoundary, StrictMode,
  Router, Link,
  useRouter, useLocation, useParams, useNavigate, useSearchParams,
} = MicroReact;
export { html };