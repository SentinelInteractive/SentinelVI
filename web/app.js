// Router, fetch and the shared formatting helpers. One module per screen.

const pages = new Map();
let current = { cleanup: null, focusSearch: null, onEscape: null };

export const fmt = {
  int: (n) => (n === null || n === undefined ? "" : Number(n).toLocaleString("en-US")),
  bytes(n) {
    if (n === null || n === undefined) return "";
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let v = Number(n);
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i += 1;
    }
    return i === 0 ? `${n} B` : `${v.toFixed(1)} ${units[i]}`;
  },
  hex(n, width = 8) {
    return "0x" + Number(n).toString(16).toUpperCase().padStart(width, "0");
  },
  seconds(s) {
    if (!Number.isFinite(s)) return "";
    return `${s.toFixed(2)} s`;
  },
  date(unix) {
    if (!unix) return "";
    const d = new Date(unix * 1000);
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  },
};

export async function get(path) {
  const res = await fetch(path, { headers: { accept: "application/json" } });
  const text = await res.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { error: text };
  }
  if (!res.ok) {
    const msg = body && body.error ? body.error : `request failed (${res.status})`;
    const err = new Error(msg);
    err.status = res.status;
    throw err;
  }
  return body;
}

export async function post(path) {
  const res = await fetch(path, { method: "POST" });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error || `request failed (${res.status})`);
  return body;
}

export function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k === "html") node.innerHTML = v;
    else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : String(v));
  }
  for (const child of [].concat(children)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child.nodeType ? child : document.createTextNode(String(child)));
  }
  return node;
}

export function empty(title, body, command) {
  const box = el("div", { class: "empty" }, [
    el("p", { text: title }),
    body ? el("p", { text: body }) : null,
  ]);
  if (command) {
    const line = el("p", {});
    line.append(el("code", { text: command }));
    box.append(line);
  }
  return box;
}

const routes = [
  { re: /^\/$/, load: () => import("./pages/library.js") },
  { re: /^\/library\/?$/, load: () => import("./pages/library.js") },
  { re: /^\/asset\/(\d+)\/?$/, load: () => import("./pages/asset.js") },
  { re: /^\/map\/?$/, load: () => import("./pages/map.js") },
  { re: /^\/textures\/?$/, load: () => import("./pages/textures.js") },
  { re: /^\/audio\/?$/, load: () => import("./pages/audio.js") },
  { re: /^\/animation\/?$/, load: () => import("./pages/animation.js") },
  { re: /^\/jobs\/?$/, load: () => import("./pages/jobs.js") },
];

export function navigate(path, replace = false) {
  if (replace) history.replaceState({}, "", path);
  else history.pushState({}, "", path);
  render();
}

function currentRoute() {
  return location.pathname || "/";
}

async function render() {
  const path = currentRoute();
  const entry = routes.find((r) => r.re.test(path));
  const main = document.getElementById("content");
  for (const a of document.querySelectorAll(".nav a")) {
    const target = a.dataset.route;
    const active = entry && path.startsWith(target === "/library" ? "/library" : target);
    if (active) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  }
  if (current.cleanup) {
    try {
      current.cleanup();
    } catch {
      /* a page that failed to build has nothing to clean */
    }
  }
  current = { cleanup: null, focusSearch: null, onEscape: null };
  main.replaceChildren();
  if (!entry) {
    main.append(
      empty(
        `There is no screen at ${path}.`,
        "The Library, Map, Textures, Audio, Animation and Jobs screens are the whole product today.",
      ),
    );
    return;
  }
  const mod = await entry.load();
  const match = path.match(entry.re);
  const params = match ? match.slice(1) : [];
  try {
    const result = await mod.mount(main, { params, navigate, page: pages });
    if (result) current = { ...current, ...result };
  } catch (e) {
    main.append(
      empty(
        "This screen could not load.",
        e && e.message ? e.message : "The local server did not answer.",
      ),
    );
  }
  window.scrollTo(0, 0);
  main.focus({ preventScroll: true });
}

async function loadCounts() {
  const box = document.getElementById("counts");
  try {
    const info = await get("/api/info");
    box.replaceChildren(
      el("div", {}, ["payloads", el("span", { text: fmt.int(info.counts.streams) })]),
      el("div", {}, ["named", el("span", { text: fmt.int(info.counts.named) })]),
      el("div", {}, ["packs", el("span", { text: fmt.int(info.counts.packs) })]),
    );
  } catch {
    box.replaceChildren(el("div", {}, ["index", el("span", { text: "unavailable" })]));
  }
}

document.addEventListener("click", (e) => {
  const a = e.target.closest("a[href^='/']");
  if (!a || a.target === "_blank" || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
  e.preventDefault();
  navigate(a.getAttribute("href"));
});

document.addEventListener("keydown", (e) => {
  const target = e.target;
  const typing =
    target &&
    (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);
  if (e.key === "/" && !typing) {
    if (current.focusSearch) {
      e.preventDefault();
      current.focusSearch();
    }
  } else if (e.key === "Escape") {
    if (current.onEscape) current.onEscape();
    else if (typing && target.value) {
      target.value = "";
      target.dispatchEvent(new Event("input"));
    }
  }
});

window.addEventListener("popstate", render);
loadCounts();
render();
setInterval(loadCounts, 30000);


