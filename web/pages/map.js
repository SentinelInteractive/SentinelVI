import { el, fmt, get, empty } from "../app.js";

const WORLD = [-13000, -9000, 7000, 11000];

export async function mount(root) {
  const meta = await get("/api/map/meta").catch(() => ({ exists: false }));
  const headLine = el("p", { class: "status", id: "map-count", text: "" });
  const head = el("div", { class: "screen-head" }, [el("h1", { text: "Map" }), headLine]);

  if (!meta.exists) {
    root.append(
      el("div", { class: "screen" }, [
        head,
        empty(
          "The placement index has not been built.",
          "The map draws positions decoded from the map resources in levels.rpf. Building the index reads that archive once and takes a few minutes.",
          "sentinel index placement",
        ),
      ]),
    );
    return {};
  }

  const m = meta.meta;
  const layerButtons = [
    el("button", { type: "button", "aria-pressed": "true", text: "Entities" }),
    el("button", { type: "button", "aria-pressed": "false", text: "Props" }),
  ];
  const interiorBox = el("input", { type: "checkbox", id: "interior" });
  const countLine = headLine;
  const canvas = el("canvas", { tabindex: "0", "aria-label": "Placement map" });
  const frame = el("div", { class: "map-frame" }, [canvas]);
  const caption = el("p", { class: "hint" });

  const tools = el("div", { class: "map-tools" }, [
    el("button", { class: "btn", type: "button", text: "+", "aria-label": "Zoom in", onclick: () => zoom(1 / 1.4) }),
    el("button", { class: "btn", type: "button", text: "−", "aria-label": "Zoom out", onclick: () => zoom(1.4) }),
    el("button", { class: "btn", type: "button", text: "Whole map", onclick: reset }),
  ]);
  frame.append(tools);

  root.append(
    el("div", { class: "screen" }, [
      head,
      el("div", { class: "filters" }, [
        el("div", { class: "segmented", role: "group", "aria-label": "Layer" }, layerButtons),
        el("label", { class: "check" }, [interiorBox, "Interior resources only"]),
      ]),
      frame,
      caption,
    ]),
  );

  let bbox = [...WORLD];
  let layer = 0;
  let interior = false;
  let points = [];
  let total = 0;
  let shown = 0;
  let loading = false;

  const ctx = canvas.getContext("2d", { alpha: false });
  let dpr = Math.min(window.devicePixelRatio || 1, 2);
  let width = 0;
  let height = 0;

  function resize() {
    const rect = frame.getBoundingClientRect();
    width = Math.max(1, Math.floor(rect.width));
    height = Math.max(1, Math.floor(rect.height));
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.floor(width * dpr);
    canvas.height = Math.floor(height * dpr);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    draw();
  }

  // Keep one metre the same length on both axes, and letterbox the rest.
  function project(x, y) {
    const dx = bbox[2] - bbox[0];
    const dy = bbox[3] - bbox[1];
    const s = Math.min(width / dx, height / dy);
    const ox = (width - dx * s) / 2;
    const oy = (height - dy * s) / 2;
    return [(ox + (x - bbox[0]) * s) * dpr, canvas.height - (oy + (y - bbox[1]) * s) * dpr];
  }

  function draw() {
    if (!width || !height) return;
    ctx.fillStyle = "#161616";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    if (!points.length) {
      ctx.fillStyle = "#9A9A9A";
      ctx.font = `${12 * dpr}px "JetBrains Mono", monospace`;
      ctx.fillText(loading ? "reading placements" : "no placements in view", 16 * dpr, 24 * dpr);
      return;
    }
    // One point is one placement. Overlapping points brighten because each one
    // adds a fixed alpha, so density reads without a legend.
    ctx.fillStyle = "rgba(250, 250, 250, 0.55)";
    for (const p of points) {
      const [px, py] = project(p[0], p[1]);
      const ix = px | 0;
      const iy = py | 0;
      if (ix < 0 || iy < 0 || ix >= canvas.width || iy >= canvas.height) continue;
      ctx.fillRect(ix, iy, 1, 1);
    }
  }

  function updateCaption() {
    const layerName = layer === 0 ? "entities" : "props";
    if (loading) {
      countLine.textContent = "reading placements";
      return;
    }
    countLine.textContent = `Showing ${fmt.int(shown)} of ${fmt.int(total)} ${layerName} in view`;
    const stored = layer === 0 ? m.entities_stored : m.props_stored;
    const read = layer === 0 ? m.entities : m.props;
    caption.textContent =
      layer === 0
        ? `${fmt.int(stored)} of ${fmt.int(read)} entity records are inside the map bounds and stored. Every stored entity is drawn when it falls in view.`
        : `${fmt.int(stored)} of ${fmt.int(read)} prop instances are stored: one in four is kept, so the props layer is a sample. Every stored prop is drawn when it falls in view.`;
  }

  let fetchTimer = null;
  async function fetchPoints() {
    if (fetchTimer) clearTimeout(fetchTimer);
    fetchTimer = setTimeout(async () => {
      loading = true;
      updateCaption();
      try {
        const q = bbox.map((v) => v.toFixed(1)).join(",");
        const body = await get(
          `/api/map/points?bbox=${q}&layer=${layer}&interior=${interior ? 1 : 0}&limit=20000`,
        );
        points = body.points || [];
        total = body.total || 0;
        shown = body.shown || 0;
      } catch (e) {
        points = [];
        total = 0;
        shown = 0;
        countLine.textContent = e.message;
      }
      loading = false;
      draw();
      updateCaption();
    }, 90);
  }

  function clampBbox(next) {
    const minSpan = 400;
    let [x0, y0, x1, y1] = next;
    let w = x1 - x0;
    let h = y1 - y0;
    if (w < minSpan) {
      const cx = (x0 + x1) / 2;
      x0 = cx - minSpan / 2;
      x1 = cx + minSpan / 2;
      w = minSpan;
    }
    if (h < minSpan) {
      const cy = (y0 + y1) / 2;
      y0 = cy - minSpan / 2;
      y1 = cy + minSpan / 2;
      h = minSpan;
    }
    return [x0, y0, x1, y1];
  }

  function zoom(factor, cx = (bbox[0] + bbox[2]) / 2, cy = (bbox[1] + bbox[3]) / 2) {
    bbox = clampBbox([
      cx + (bbox[0] - cx) * factor,
      cy + (bbox[1] - cy) * factor,
      cx + (bbox[2] - cx) * factor,
      cy + (bbox[3] - cy) * factor,
    ]);
    draw();
    fetchPoints();
  }

  function pan(dx, dy) {
    bbox = [bbox[0] + dx, bbox[1] + dy, bbox[2] + dx, bbox[3] + dy];
    draw();
    fetchPoints();
  }

  function reset() {
    bbox = [...WORLD];
    draw();
    fetchPoints();
  }

  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    let last = [e.clientX, e.clientY];
    const move = (ev) => {
      const dx = ev.clientX - last[0];
      const dy = ev.clientY - last[1];
      last = [ev.clientX, ev.clientY];
      const wx = (dx / width) * (bbox[2] - bbox[0]);
      const wy = (dy / height) * (bbox[3] - bbox[1]);
      bbox = [bbox[0] - wx, bbox[1] + wy, bbox[2] - wx, bbox[3] + wy];
      draw();
    };
    const up = () => {
      canvas.removeEventListener("pointermove", move);
      canvas.removeEventListener("pointerup", up);
      fetchPoints();
    };
    canvas.addEventListener("pointermove", move);
    canvas.addEventListener("pointerup", up);
  });

  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const fx = (e.clientX - rect.left) / rect.width;
      const fy = 1 - (e.clientY - rect.top) / rect.height;
      const cx = bbox[0] + fx * (bbox[2] - bbox[0]);
      const cy = bbox[1] + fy * (bbox[3] - bbox[1]);
      zoom(e.deltaY > 0 ? 1.2 : 1 / 1.2, cx, cy);
    },
    { passive: false },
  );

  canvas.addEventListener("keydown", (e) => {
    const w = bbox[2] - bbox[0];
    const h = bbox[3] - bbox[1];
    if (e.key === "ArrowLeft") pan(-w * 0.2, 0);
    else if (e.key === "ArrowRight") pan(w * 0.2, 0);
    else if (e.key === "ArrowUp") pan(0, h * 0.2);
    else if (e.key === "ArrowDown") pan(0, -h * 0.2);
    else if (e.key === "+" || e.key === "=") zoom(1 / 1.4);
    else if (e.key === "-") zoom(1.4);
    else if (e.key === "0") reset();
    else return;
    e.preventDefault();
  });

  layerButtons.forEach((b, idx) =>
    b.addEventListener("click", () => {
      layer = idx;
      layerButtons.forEach((x, i) => x.setAttribute("aria-pressed", i === idx ? "true" : "false"));
      fetchPoints();
    }),
  );
  interiorBox.addEventListener("change", () => {
    interior = interiorBox.checked;
    fetchPoints();
  });

  const observer = new ResizeObserver(() => resize());
  observer.observe(frame);
  resize();
  fetchPoints();

  return {
    cleanup: () => observer.disconnect(),
    onEscape: reset,
  };
}

