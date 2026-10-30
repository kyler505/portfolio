/* Planck.js (Box2D) drives the cutouts; fixtures follow their visible pixels. */
(() => {
  const P = window.planck;
  if (!P) return;
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
  const scale = 60;
  let cleanup = null;

  async function setup(board) {
    if (board.dataset.physicsReady) return;
    board.dataset.physicsReady = 'loading';
    const items = [...board.querySelectorAll('.scratchboard-item')];
    await Promise.all(items.map(item => item.querySelector('img').decode().catch(() => {})));
    if (!board.isConnected || items.some(item => !item.querySelector('img').naturalWidth)) {
      delete board.dataset.physicsReady;
      return;
    }
    if (cleanup) cleanup();
    board.dataset.physicsReady = 'true';
    let world, bodies = [], joint = null, dragIndex = -1, frame = 0, last = 0, accumulator = 0, visible = false;
    const listeners = [];
    const vec = (x, y) => P.Vec2(x / scale, y / scale);
    const point = event => { const r = board.getBoundingClientRect(); return vec(event.clientX - r.left, event.clientY - r.top); };
    function on(el, name, fn, options) { el.addEventListener(name, fn, options); listeners.push(() => el.removeEventListener(name, fn, options)); }

    function rectangles(img, width, height) {
      const grid = 28, canvas = document.createElement('canvas');
      canvas.width = canvas.height = grid;
      const ctx = canvas.getContext('2d', { willReadFrequently: true });
      const ratio = Math.min(width / img.naturalWidth, height / img.naturalHeight);
      const dw = img.naturalWidth * ratio, dh = img.naturalHeight * ratio;
      ctx.drawImage(img, (width - dw) / 2 / width * grid, (height - dh) / 2 / height * grid, dw / width * grid, dh / height * grid);
      const pixels = ctx.getImageData(0, 0, grid, grid).data;
      let previous = new Map(), result = [];
      for (let y = 0; y < grid; y++) {
        const runs = new Map();
        for (let x = 0; x < grid;) {
          while (x < grid && pixels[(y * grid + x) * 4 + 3] < 160) x++;
          const start = x;
          while (x < grid && pixels[(y * grid + x) * 4 + 3] >= 160) x++;
          if (start === x) continue;
          const key = `${start}:${x}`;
          const rect = previous.get(key) || { x: start, y, w: x - start, h: 0 };
          rect.h++;
          runs.set(key, rect);
        }
        for (const [key, rect] of previous) if (!runs.has(key)) result.push(rect);
        previous = runs;
      }
      result.push(...previous.values());
      return result.map(r => ({ x: (r.x + r.w / 2) * width / grid - width / 2, y: (r.y + r.h / 2) * height / grid - height / 2, w: r.w * width / grid, h: r.h * height / grid }));
    }

    function createWorld() {
      if (joint && world) world.destroyJoint(joint);
      joint = null; dragIndex = -1;
      world = new P.World({ gravity: P.Vec2(0, reduced.matches ? 0 : 14), allowSleep: true });
      const w = board.clientWidth, h = board.clientHeight, t = 100;
      const boundary = world.createBody();
      for (const [x, y, width, height] of [[w / 2, h + t / 2, w + t * 2, t], [-t / 2, h / 2, t, h + t * 2], [w + t / 2, h / 2, t, h + t * 2], [w / 2, -t / 2, w + t * 2, t]]) {
        boundary.createFixture(P.Box(width / (2 * scale), height / (2 * scale), vec(x, y), 0), { friction: 0.6 });
      }
      bodies = items.map((item, i) => {
        const width = item.offsetWidth, height = item.offsetHeight;
        const columns = Math.max(1, Math.floor(w / width)), rows = Math.ceil(items.length / columns);
        const x = width / 2 + (w - width) * (i % columns) / Math.max(1, columns - 1);
        const y = height / 2 + Math.max(0, h - height) * Math.floor(i / columns) / Math.max(1, rows - 1);
        const body = world.createDynamicBody({ position: vec(x, y), bullet: true, linearDamping: 0.12, angularDamping: 0.4, allowSleep: true });
        for (const r of rectangles(item.querySelector('img'), width, height)) {
          body.createFixture(P.Box(Math.max(0.005, r.w / (2 * scale)), Math.max(0.005, r.h / (2 * scale)), vec(r.x, r.y), 0), { density: 1, friction: 0.55, restitution: 0.08 });
        }
        return { body, item, width, height };
      });
      render();
    }
    function render() {
      for (const { body, item, width, height } of bodies) {
        const p = body.getPosition();
        item.style.transformOrigin = '50% 50%';
        item.style.transform = `translate3d(${p.x * scale - width / 2}px, ${p.y * scale - height / 2}px, 0) rotate(${body.getAngle()}rad)`;
      }
    }
    function stop() { if (frame) cancelAnimationFrame(frame); frame = 0; last = 0; accumulator = 0; }
    function step(now) {
      if (!visible || reduced.matches) { stop(); return; }
      accumulator += last ? Math.min((now - last) / 1000, 0.05) : 1 / 60;
      last = now;
      while (accumulator >= 1 / 60) { world.step(1 / 60, 10, 6); accumulator -= 1 / 60; }
      render();
      if (!joint && bodies.every(({ body }) => !body.isAwake())) { stop(); return; }
      frame = requestAnimationFrame(step);
    }
    function start() { if (visible && !reduced.matches && !frame) frame = requestAnimationFrame(step); }
    for (let i = 0; i < items.length; i++) {
      const button = items[i].querySelector('button');
      on(button, 'pointerdown', event => {
        if (event.button !== 0 || joint || dragIndex !== -1) return;
        const record = bodies[i], target = point(event);
        dragIndex = i;
        record.body.setAwake(true);
        if (!reduced.matches) {
          joint = world.createJoint(P.MouseJoint({ maxForce: 160 * record.body.getMass(), frequencyHz: 4, dampingRatio: 1 }, ground(), record.body, target));
        }
        button.setPointerCapture(event.pointerId);
        items.forEach(el => el.style.zIndex = '');
        record.item.style.zIndex = '3';
        start();
      });
      on(button, 'pointermove', event => {
        if (dragIndex !== i) return;
        const target = point(event);
        if (joint) joint.setTarget(target);
        else if (reduced.matches) { bodies[i].body.setPosition(target); render(); }
      });
      const release = () => {
        if (dragIndex !== i) return;
        if (joint) { world.destroyJoint(joint); joint = null; }
        dragIndex = -1;
        start();
      };
      on(button, 'pointerup', release);
      on(button, 'pointercancel', release);
      on(button, 'lostpointercapture', release);
      on(button, 'keydown', event => {
        const distance = event.shiftKey ? 20 : 5;
        const offset = { ArrowLeft: [-distance, 0], ArrowRight: [distance, 0], ArrowUp: [0, -distance], ArrowDown: [0, distance] }[event.key];
        if (!offset) return;
        event.preventDefault();
        const body = bodies[i].body;
        if (reduced.matches) { const p = body.getPosition(); body.setPosition(P.Vec2(p.x + offset[0] / scale, p.y + offset[1] / scale)); render(); }
        else { body.setAwake(true); body.applyLinearImpulse(vec(offset[0] * body.getMass() * 2, offset[1] * body.getMass() * 2), body.getWorldCenter(), true); start(); }
      });
    }
    let groundBody;
    function ground() { if (!groundBody || groundBody.getWorld() !== world) groundBody = world.createBody(); return groundBody; }
    const observer = new IntersectionObserver(entries => { visible = entries[0].isIntersecting; if (visible) start(); else stop(); });
    const onScroll = () => { if (!visible || reduced.matches) return; const r = board.getBoundingClientRect(); board.style.backgroundPosition = `0 ${(innerHeight / 2 - (r.top + r.height / 2)) * 0.06}px`; };
    const onMotion = () => { world.setGravity(P.Vec2(0, reduced.matches ? 0 : 14)); if (reduced.matches) { board.style.backgroundPosition = ''; stop(); } else { bodies.forEach(({ body }) => body.setAwake(true)); start(); } };
    let size = `${board.clientWidth}:${board.clientHeight}`;
    const sizeObserver = new ResizeObserver(() => { const next = `${board.clientWidth}:${board.clientHeight}`; if (next !== size) { size = next; stop(); createWorld(); start(); } });
    on(window, 'scroll', onScroll, { passive: true });
    reduced.addEventListener('change', onMotion);
    createWorld(); observer.observe(board); sizeObserver.observe(board);
    cleanup = () => { stop(); observer.disconnect(); sizeObserver.disconnect(); listeners.forEach(remove => remove()); reduced.removeEventListener('change', onMotion); };
  }
  function findBoard() { const board = document.querySelector('.scratchboard'); if (board && !board.dataset.physicsReady) setup(board); }
  new MutationObserver(findBoard).observe(document.getElementById('app'), { childList: true, subtree: true });
  findBoard();
})();
