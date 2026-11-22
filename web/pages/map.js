import { el, fmt, get, empty } from "../app.js";

const WORLD = [-13000, -9000, 7000, 11000];
const RASTER = 1024;
const POINT_SPAN = 5000;
const POINT_LIMIT = 20000;

function compact(n) {
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)} M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} k`;
  return String(n);
}

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
  const canvas = el("canvas", { tabindex: "0", "aria-label": "Placement map" });
  const worldRaster = el("img", {
    class: "map-raster",
    alt: "",
    src: "/api/map/density?bbox=-13000,-9000,7000,11000&layer=0&interior=0&w=1024&h=1024",
  });
  const frame = el("div", { class: "map-frame" }, [canvas, worldRaster]);
  const caption = el("p", { class: "hint map-caption" });

  frame.append(
    el("div", { class: "map-tools" }, [
      el("button", { class: "btn", type: "button", text: "+", "aria-label": "Zoom in", onclick: () => zoom(1 / 1.4) }),
      el("button", { class: "btn", type: "button", text: "-", "aria-label": "Zoom out", onclick: () => zoom(1.4) }),
      el("button", { class: "btn", type: "button", text: "Whole map", onclick: reset }),
    ]),
    caption,
  );

  root.append(
    el("div", { class: "screen" }, [
      head,
      el("div", { class: "filters" }, [
        el("div", { class: "segmented", role: "group", "aria-label": "Layer" }, layerButtons),
        el("label", { class: "check" }, [interiorBox, "Interior resources only"]),
      ]),
      frame,
    ]),
  );
  document.title = "Map - SentinelVI";

  let bbox = [...WORLD];
  let layer = 0;
  let interior = false;
  let points = [];
  let mode = "density";
  let raster = null;
  let rasterKey = "";
  let rasterStats = { total: 0, bins: 0, max: 0 };
  let terrain = null;
  let loading = false;

  const ctx = canvas.getContext("2d", { alpha: false });
  let dpr = 1;
  let width = 0;
  let height = 0;

  function project(x, y) {
    const dx = bbox[2] - bbox[0];
    const dy = bbox[3] - bbox[1];
    const s = Math.min(width / dx, height / dy);
    const ox = (width - dx * s) / 2;
    const oy = (height - dy * s) / 2;
    return [(ox + (x - bbox[0]) * s) * dpr, canvas.height - (oy + (y - bbox[1]) * s) * dpr];
  }

  function worldRect(b) {
    const [l, t] = project(b[0], b[3]);
    const [r, bo] = project(b[2], b[1]);
    return [l, t, Math.max(1, r - l), Math.max(1, bo - t)];
  }

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

  function draw() {
    if (!width || !height) return;
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    ctx.fillStyle = "#161616";
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    if (terrain) {
      const [x, y, w, h] = worldRect(terrain.bounds);
      ctx.drawImage(terrain.image, x, y, w, h);
    }

    if (mode === "density" && raster) {
      worldRaster.hidden = true;
      const [x, y, w, h] = worldRect(bbox);
      // Black cells add nothing, so the density lightens the terrain instead of
      // covering it.
      ctx.globalCompositeOperation = "lighter";
      ctx.drawImage(raster, x, y, w, h);
      ctx.globalCompositeOperation = "source-over";
    }

    if (mode === "points") {
      ctx.fillStyle = "rgba(250, 250, 250, 0.7)";
      for (const p of points) {
        const [px, py] = project(p[0], p[1]);
        const ix = px | 0;
        const iy = py | 0;
        if (ix < 0 || iy < 0 || ix >= canvas.width || iy >= canvas.height) continue;
        ctx.fillRect(ix, iy, 1, 1);
      }
    }
  }

  /// One line under the map. Everything else is in the tooltip.
  function updateCaption() {
    const stored = layer === 0 ? m.entities_stored : m.props_stored;
    const read = layer === 0 ? m.entities : m.props;
    const noun = layer === 0 ? "entities" : "props";
    headLine.textContent = loading ? "reading placements" : `${fmt.int(stored)} ${noun} stored`;
    if (loading) {
      caption.textContent = "";
      return;
    }
    const metres = Math.round((bbox[2] - bbox[0]) / RASTER);
    if (mode === "points") {
      caption.textContent = `${compact(stored)} ${noun}. ${fmt.int(points.length)} points in view.`;
      caption.title = `Sampled on a rowid stride across the whole view, ${fmt.int(
        POINT_LIMIT,
      )} points at most.`;
      return;
    }
    caption.textContent = `${compact(stored)} ${noun}. Density, ${metres} m cells.`;
    caption.title =
      `${fmt.int(rasterStats.total)} ${noun} in this view over ${fmt.int(RASTER)} × ${fmt.int(RASTER)} cells, ` +
      `${fmt.int(rasterStats.bins)} cells occupied, brightest cell ${fmt.int(rasterStats.max)} records. ` +
      `${fmt.int(stored)} of ${fmt.int(read)} records inside the map bounds are stored. ` +
      (layer === 1 ? "One prop instance in four is kept. " : "") +
      "Terrain is the heightmap from common.rpf.";
  }

  function span() {
    return Math.max(bbox[2] - bbox[0], bbox[3] - bbox[1]);
  }

  let timer = null;
  function refresh() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(async () => {
      loading = true;
      updateCaption();
      draw();
      const q = bbox.map((v) => v.toFixed(1)).join(",");
      const wantPoints = span() <= POINT_SPAN;
      try {
        if (wantPoints) {
          const body = await get(
            `/api/map/points?bbox=${q}&layer=${layer}&interior=${interior ? 1 : 0}&limit=${POINT_LIMIT}&mode=sample`,
          );
          points = body.points || [];
          mode = "points";
        } else {
          const key = `${q}|${layer}|${interior ? 1 : 0}`;
          const res = await fetch(
            `/api/map/density?bbox=${q}&layer=${layer}&interior=${interior ? 1 : 0}&w=${RASTER}&h=${RASTER}`,
          );
          if (!res.ok) throw new Error(`density request failed (${res.status})`);
          rasterStats = {
            total: Number(res.headers.get("x-sentinel-total") || 0),
            bins: Number(res.headers.get("x-sentinel-bins") || 0),
            max: Number(res.headers.get("x-sentinel-max") || 0),
          };
          mode = "density";
          if (key !== rasterKey) {
            rasterKey = key;
            const img = new Image();
            img.onload = () => {
              raster = img;
              draw();
            };
            img.src = URL.createObjectURL(await res.blob());
          }
        }
      } catch (e) {
        points = [];
        headLine.textContent = e.message;
      }
      loading = false;
      draw();
      updateCaption();
    }, 120);
  }

  async function loadTerrain() {
    try {
      const res = await fetch("/api/map/terrain");
      if (!res.ok) return;
      const bounds = (res.headers.get("x-sentinel-bounds") || "")
        .split(",")
        .map((v) => Number(v.trim()));
      if (bounds.length !== 4 || bounds.some((v) => !Number.isFinite(v))) return;
      const blob = await res.blob();
      const img = new Image();
      img.src = URL.createObjectURL(blob);
      await img.decode();
      terrain = { image: img, bounds };
    } catch {
      terrain = null;
    }
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
    refresh();
  }

  function pan(dx, dy) {
    bbox = [bbox[0] + dx, bbox[1] + dy, bbox[2] + dx, bbox[3] + dy];
    draw();
    refresh();
  }

  function reset() {
    bbox = [...WORLD];
    draw();
    refresh();
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
      refresh();
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
      refresh();
    }),
  );
  interiorBox.addEventListener("change", () => {
    interior = interiorBox.checked;
    refresh();
  });

  const observer = new ResizeObserver(() => resize());
  observer.observe(frame);
  resize();
  headLine.textContent = `${fmt.int(m.entities_stored)} entities stored`;
  caption.textContent = `${compact(m.entities_stored)} entities. Density, ${Math.round(
    (WORLD[2] - WORLD[0]) / RASTER,
  )} m cells.`;
  loadTerrain().then(() => {
    draw();
    refresh();
  });

  return {
    cleanup: () => observer.disconnect(),
    onEscape: reset,
  };
}



