import { el, fmt, get, empty } from "../app.js";

const PAGE = 200;
const PROVENANCE =
  "Dictionary paths are matched using stored hashes. They are not original archive paths.";
const SEARCH_HINT =
  "Path search matches a fragment. Roots, hexadecimal hashes and decimal slots must match exactly.";
const FIELD_HINT = {
  path: "Path substring",
  hash32: "Exact hash32 hex",
  hash64: "Exact hash64 hex",
  root: "Exact root",
  slot: "Exact slot number",
};

function setStatus(node, text, state) {
  node.textContent = text;
  if (state) node.dataset.state = state;
  else delete node.dataset.state;
}

function displayArchive(name) {
  return name.replace(/_rpf$/, "").replace(/__/g, "/");
}

function confidenceLabel(c) {
  switch (c) {
    case "exact":
      return "exact";
    case "path_prefix":
      return "from path";
    case "identifier":
      return "pattern";
    default:
      return "weak";
  }
}

function mountReal(root, { navigate }, rootsBody) {
  const rootRows = rootsBody.rows || [];

  const countLine = el("p", { class: "status" });
  const field = el("select", { id: "search-field", class: "field", "aria-label": "Search field" }, [
    el("option", { value: "path", text: "Path" }),
    el("option", { value: "hash32", text: "hash32" }),
    el("option", { value: "hash64", text: "hash64" }),
    el("option", { value: "root", text: "Root" }),
    el("option", { value: "slot", text: "Slot" }),
  ]);
  const input = el("input", {
    id: "search",
    type: "search",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: FIELD_HINT.path,
    "aria-label": "Search value",
  });
  const tree = el("div", { class: "tree", role: "group", "aria-label": "Archive roots" });
  const list = el("div", { class: "list", role: "listbox", "aria-label": "Slots" });
  const pager = el("div", { class: "pager" });
  const panel = el("aside", { class: "panel", hidden: true });
  const columns = el("div", { class: "columns" }, [
    el("div", {}, [tree]),
    el("div", {}, [list, pager]),
    panel,
  ]);

  root.append(
    el("div", { class: "screen" }, [
      el("div", { class: "screen-head" }, [el("h1", { text: "Library" }), countLine]),
      el("div", { class: "search" }, [field, input, el("kbd", { text: "/" })]),
      el("p", { class: "hint", text: PROVENANCE }),
      el("p", { class: "hint", text: SEARCH_HINT }),
      columns,
    ]),
  );

  let token = 0;
  let headerToken = 0;
  let rows = [];
  let selected = -1;
  let scope = { root: null, headerOffset: null, field: "", q: "" };
  let afterStack = [null];
  let page = 0;
  let nextCursor = null;

  function showPanel(s) {
    if (!s) {
      panel.hidden = true;
      columns.className = "columns";
      return;
    }
    panel.hidden = false;
    columns.className = "columns with-inspector";
    const kv = el("dl", { class: "kv" });
    const add = (label, value) => kv.append(el("dt", { text: label }), el("dd", { text: value }));
    add("name", s.display_name);
    add("name origin", s.name_origin || "-");
    add("root", s.root);
    add("archive offset", fmt.hex(s.archive_offset));
    add("slot", s.slot);
    const link = el("a", {
      class: "btn btn-primary",
      href: `/asset/toc/${s.id}`,
      text: "Open asset",
    });
    panel.replaceChildren(el("h2", { text: "Slot" }), kv, link);
  }

  function selectRow(i) {
    if (!rows.length) return;
    selected = Math.max(0, Math.min(rows.length - 1, i));
    [...list.children].forEach((node, idx) =>
      node.setAttribute("aria-selected", idx === selected ? "true" : "false"),
    );
    const node = list.children[selected];
    if (node) node.scrollIntoView({ block: "nearest" });
    showPanel(rows[selected]);
  }

  function renderRows() {
    list.replaceChildren();
    if (!rows.length) {
      let title = "No slots to show.";
      let detail = "Choose a root on the left, or select a search field and type a value.";
      if (scope.field && scope.q) {
        title = `No ${scope.field} matches “${scope.q}”.`;
        detail = scope.field === "path"
          ? "Try a shorter path fragment, or clear the search."
          : "Check the exact value, or clear the search.";
      } else if (scope.headerOffset != null) {
        title = "This header holds no slots.";
      } else if (scope.root) {
        title = "This root holds no slots.";
      }
      list.append(empty(title, detail));
      return;
    }
    rows.forEach((s, i) => {
      const sub = `${s.root}, slot ${s.slot}, ${s.kind}`;
      const row = el(
        "button",
        {
          class: "row",
          type: "button",
          role: "option",
          "aria-selected": i === selected ? "true" : "false",
          onclick: () => selectRow(i),
          ondblclick: () => navigate(`/asset/toc/${s.id}`),
          onkeydown: (e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              navigate(`/asset/toc/${s.id}`);
            } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              selectRow(i + (e.key === "ArrowDown" ? 1 : -1));
              list.children[selected]?.focus();
            }
          },
        },
        [
          el("span", {}, [
            el("span", { class: "name", text: s.display_name }),
            el("span", { class: "sub", text: sub }),
          ]),
          el("span", { class: "right" }, [
            el("span", { text: s.stored_bytes == null ? "-" : fmt.bytes(s.stored_bytes) }),
            el("span", { class: "conf", text: s.name_origin || s.signature_status || "" }),
          ]),
        ],
      );
      list.append(row);
    });
  }

  function renderPager() {
    pager.replaceChildren();
    if (!rows.length && nextCursor == null && page === 0) return;
    pager.append(
      el("button", {
        class: "btn",
        type: "button",
        text: "Previous",
        disabled: page === 0,
        onclick: () => changePage(-1),
      }),
      el("span", {
        class: "status",
        text: `Page ${fmt.int(page + 1)}${nextCursor != null ? "" : ", last"}`,
      }),
      el("button", {
        class: "btn",
        type: "button",
        text: "Next",
        disabled: nextCursor == null,
        onclick: () => changePage(1),
      }),
    );
  }

  function changePage(delta) {
    if (delta > 0) {
      if (nextCursor == null) return;
      afterStack = afterStack.slice(0, page + 1);
      afterStack.push(nextCursor);
      page += 1;
    } else {
      if (page === 0) return;
      page -= 1;
    }
    loadSlots();
  }

  async function loadSlots() {
    const mine = ++token;
    setStatus(countLine, "Loading slots");
    list.replaceChildren(el("p", { class: "status", text: "Loading slots" }));
    pager.replaceChildren();
    panel.hidden = true;
    columns.className = "columns";
    selected = -1;
    rows = [];

    const params = new URLSearchParams();
    if (scope.field && scope.q) {
      params.set("field", scope.field);
      params.set("q", scope.q);
    } else {
      if (scope.root) params.set("root", scope.root);
      if (scope.headerOffset != null) params.set("header_offset", String(scope.headerOffset));
    }
    const after = afterStack[page];
    if (after != null) params.set("after", after);
    params.set("limit", String(PAGE));

    try {
      const body = await get(`/api/library/slots?${params}`);
      if (mine !== token) return;
      rows = body.rows || [];
      nextCursor = body.next == null ? null : body.next;
      const noun = scope.field && scope.q ? `${scope.field} matches` : "slots";
      setStatus(countLine, `${fmt.int(rows.length)} ${noun} on page ${fmt.int(page + 1)}`);
      renderRows();
      renderPager();
    } catch (e) {
      if (mine !== token) return;
      rows = [];
      nextCursor = null;
      setStatus(countLine, e.message || "failed to load slots", "failed");
      list.replaceChildren(empty("The slots could not be read.", e.message));
    }
  }

  function setScope(next) {
    scope = { ...scope, ...next };
    afterStack = [null];
    page = 0;
    nextCursor = null;
    loadSlots();
  }

  function browseScope(next) {
    clearTimeout(timer);
    timer = null;
    input.value = "";
    setScope({ field: "", q: "", ...next });
  }

  function loadHeaders(rootName, holder) {
    const mine = ++headerToken;
    let after = null;
    const status = el("p", { class: "status", text: "Loading headers" });
    holder.append(status);

    async function loadPage() {
      const params = new URLSearchParams({ root: rootName, limit: String(PAGE) });
      if (after != null) params.set("after", after);
      try {
        const body = await get(`/api/library/headers?${params}`);
        if (mine !== headerToken || !holder.isConnected) return;
        status.remove();
        for (const h of body.rows || []) {
          const offset = h.header_offset;
          const hb = el(
            "button",
            {
              type: "button",
              title: `header offset ${fmt.hex(offset)}, signature ${
                h.signature_status || "unknown"
              }, ${fmt.int(h.slot_count)} slots`,
              onclick: () => {
                for (const x of tree.querySelectorAll("button[aria-selected='true']")) {
                  x.removeAttribute("aria-selected");
                }
                hb.setAttribute("aria-selected", "true");
                browseScope({ root: rootName, headerOffset: offset });
              },
            },
            [
              el("span", { text: `header ${fmt.hex(offset)}` }),
              el("span", { text: fmt.int(h.slot_count) }),
            ],
          );
          holder.append(hb);
        }
        holder.querySelector(".more-headers")?.remove();
        if (body.next != null) {
          holder.append(
            el("button", {
              class: "more-headers",
              type: "button",
              text: "More headers",
              onclick: (event) => {
                event.currentTarget.disabled = true;
                after = body.next;
                loadPage();
              },
            }),
          );
        }
      } catch (e) {
        if (mine !== headerToken || !holder.isConnected) return;
        status.remove();
        const more = holder.querySelector(".more-headers");
        if (more) more.disabled = false;
        holder.append(empty("Headers could not be read.", e.message));
      }
    }
    loadPage();
  }

  function onRoot(r, b) {
    const open = b.getAttribute("aria-expanded") === "true";
    for (const other of tree.querySelectorAll("button[aria-expanded='true']")) {
      other.setAttribute("aria-expanded", "false");
      other.nextElementSibling?.remove();
    }
    if (open) return;
    b.setAttribute("aria-expanded", "true");
    const holder = el("div", { class: "packs" });
    b.after(holder);
    loadHeaders(r.root, holder);
    browseScope({ root: r.root, headerOffset: null });
  }

  function buildTree() {
    tree.replaceChildren();
    if (!rootRows.length) {
      tree.append(empty("No roots are indexed.", "The table of contents has no archive roots."));
      return;
    }
    for (const r of rootRows) {
      const title = [
        r.root,
        `${fmt.int(r.slot_count)} slots`,
        `${fmt.int(r.header_count)} headers`,
      ].join(", ");
      const button = el("button", {
        type: "button",
        "aria-expanded": "false",
        title,
        onclick: () => onRoot(r, button),
      }, [el("span", { text: r.root }), el("span", { text: fmt.int(r.slot_count) })]);
      tree.append(button);
    }
  }

  let timer = null;
  function searchNow(debounce) {
    clearTimeout(timer);
    timer = null;
    const q = input.value.trim();
    const f = field.value;
    if (!q) {
      if (scope.field || scope.q) setScope({ field: "", q: "" });
      return;
    }
    const search = () => {
      timer = null;
      setScope({ field: f, q });
    };
    if (debounce) timer = setTimeout(search, 200);
    else search();
  }

  field.addEventListener("change", () => {
    input.placeholder = FIELD_HINT[field.value];
    searchNow(false);
  });
  input.addEventListener("input", () => searchNow(true));

  input.addEventListener("keydown", (e) => {
    if (!timer && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      selectRow(selected + (e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Enter" && timer) {
      e.preventDefault();
      searchNow(false);
    } else if (e.key === "Enter" && rows.length) {
      e.preventDefault();
      navigate(`/asset/toc/${rows[Math.max(0, selected)].id}`);
    }
  });

  buildTree();
  loadSlots();

  return {
    cleanup: () => clearTimeout(timer),
    focusSearch: () => input.focus(),
    onEscape: () => {
      if (document.activeElement === input && input.value) {
        input.value = "";
        input.dispatchEvent(new Event("input"));
        return;
      }
      panel.hidden = true;
      columns.className = "columns";
      selected = -1;
      renderRows();
    },
  };
}

/* Reconstructed catalog. Used only when the real table of contents is absent. */
async function mountFallback(root, { navigate }) {
  const info = await get("/api/info").catch(() => null);
  const named = info ? info.counts.named : 0;

  const countLine = el("p", { class: "status" });
  const input = el("input", {
    id: "search",
    type: "search",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: named ? `Search ${fmt.int(named)} names` : "Search names",
    "aria-label": "Search inferred names",
  });
  const tree = el("div", { class: "tree", role: "tree", "aria-label": "Archives" });
  const list = el("div", { class: "list", role: "listbox", "aria-label": "Payloads" });
  const pager = el("div", { class: "pager" });
  const panel = el("aside", { class: "panel", hidden: true });
  const columns = el("div", { class: "columns" }, [
    el("div", {}, [tree]),
    el("div", {}, [list, pager]),
    panel,
  ]);

  root.append(
    el("div", { class: "screen" }, [
      el("div", { class: "screen-head" }, [
        el("h1", { text: "Library" }),
        countLine,
      ]),
      el("div", { class: "search" }, [input, el("kbd", { text: "/" })]),
      el("p", { class: "hint", text: "The full table of contents is missing. These names are inferred from carved payloads, not read from the archive's file table." }),
      columns,
    ]),
  );

  let rows = [];
  let selected = -1;
  let view = "idle";
  let token = 0;

  function selectRow(i) {
    if (!rows.length) return;
    selected = Math.max(0, Math.min(rows.length - 1, i));
    [...list.children].forEach((node, idx) =>
      node.setAttribute("aria-selected", idx === selected ? "true" : "false"),
    );
    const node = list.children[selected];
    if (node) node.scrollIntoView({ block: "nearest" });
    showPanel(rows[selected]);
  }

  function showPanel(s) {
    if (!s) {
      panel.hidden = true;
      columns.className = "columns";
      return;
    }
    panel.hidden = false;
    columns.className = "columns with-inspector";
    const link = el("a", { class: "btn btn-primary", href: `/asset/${s.id}`, text: "Open asset" });
    const kv = el("dl", { class: "kv" }, [
      el("dt", { text: "name" }),
      el("dd", { text: s.name || "no name" }),
      el("dt", { text: "archive" }),
      el("dd", { text: displayArchive(s.archive) }),
      el("dt", { text: "size" }),
      el("dd", { text: `${fmt.int(s.uncomp_size)} B` }),
      el("dt", { text: "offset" }),
      el("dd", { text: fmt.hex(s.stream_offset) }),
      el("dt", { text: "confidence" }),
      el("dd", { text: confidenceLabel(s.confidence) }),
    ]);
    panel.replaceChildren(el("h2", { text: "Selected" }), kv, link);
  }

  function renderRows() {
    list.replaceChildren();
    if (!rows.length) {
      if (view === "search") {
        list.append(
          empty(
            `No name matches “${input.value.trim()}”.`,
            `The index holds ${fmt.int(named)} inferred names over ${fmt.int(
              info ? info.counts.streams : 0,
            )} payloads. Try a shorter fragment, or browse an archive on the left.`,
          ),
        );
      } else if (view === "pack") {
        list.append(empty("This pack holds no carved payload.", "Choose another pack."));
      } else {
        list.append(
          empty(
            "Choose an archive, or type a name.",
            `Search runs over ${fmt.int(named)} inferred names. The tree lists every nested archive header the carver found.`,
          ),
        );
      }
      pager.replaceChildren();
      return;
    }
    rows.forEach((s, i) => {
      const name = s.name || `payload ${s.id}`;
      const row = el(
        "button",
        {
          class: "row",
          type: "button",
          role: "option",
          "aria-selected": i === selected ? "true" : "false",
          onclick: () => selectRow(i),
          ondblclick: () => navigate(`/asset/${s.id}`),
        },
        [
          el("span", {}, [
            el("span", { class: "name", text: name }),
            el("span", {
              class: "sub",
              text: `${displayArchive(s.archive)} · ${
                s.extension ? "." + s.extension : s.payload_kind
              }`,
            }),
          ]),
          el("span", { class: "right" }, [
            el("span", { text: fmt.int(s.uncomp_size) + " B" }),
            el("span", { class: "conf", text: confidenceLabel(s.confidence) }),
          ]),
        ],
      );
      list.append(row);
    });
  }

  function pagerFor(total, offset, page, load) {
    pager.replaceChildren();
    if (total <= page) return;
    pager.append(
      el("button", {
        class: "btn",
        type: "button",
        text: "Previous",
        disabled: offset === 0,
        onclick: () => load(Math.max(0, offset - page)),
      }),
      el("span", {
        class: "status",
        text: `${fmt.int(offset + 1)} to ${fmt.int(Math.min(total, offset + page))} of ${fmt.int(
          total,
        )}`,
      }),
      el("button", {
        class: "btn",
        type: "button",
        text: "Next",
        disabled: offset + page >= total,
        onclick: () => load(offset + page),
      }),
    );
  }

  async function loadPack(packId, offset = 0) {
    const mine = ++token;
    view = "pack";
    setStatus(countLine, "loading pack");
    try {
      const body = await get(
        `/api/library/streams?pack=${encodeURIComponent(packId)}&offset=${offset}&limit=200`,
      );
      if (mine !== token) return;
      rows = body.rows;
      selected = -1;
      setStatus(countLine, `${fmt.int(body.total)} payloads in ${packId.replace(/^pack:/, "")}`);
      renderRows();
      pagerFor(body.total, body.offset, 200, (off) => loadPack(packId, off));
    } catch (e) {
      if (mine !== token) return;
      setStatus(countLine, e.message, "failed");
      list.append(empty("This pack could not be read.", e.message));
    }
  }

  let timer = null;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    const q = input.value.trim();
    if (!q) {
      view = "idle";
      rows = [];
      selected = -1;
      token += 1;
      countLine.textContent = "";
      renderRows();
      panel.hidden = true;
      columns.className = "columns";
      return;
    }
    timer = setTimeout(async () => {
      const mine = ++token;
      view = "search";
      setStatus(countLine, "searching");
      try {
        const body = await get(`/api/search?q=${encodeURIComponent(q)}&limit=80`);
        if (mine !== token) return;
        rows = body.rows;
        selected = -1;
        setStatus(countLine, `${fmt.int(body.total)} names match “${q}”`);
        renderRows();
        pager.replaceChildren();
      } catch (e) {
        if (mine !== token) return;
        setStatus(countLine, e.message, "failed");
        list.append(empty("The search could not be read.", e.message));
      }
    }, 200);
  });

  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectRow(selected + 1);
    } else if (e.key === "Enter" && rows.length) {
      e.preventDefault();
      navigate(`/asset/${rows[Math.max(0, selected)].id}`);
    }
  });

  async function buildTree() {
    const body = await get("/api/library/archives");
    tree.replaceChildren();
    for (const a of body.rows) {
      const b = el(
        "button",
        {
          type: "button",
          "aria-expanded": "false",
          onclick: async () => {
            const open = b.getAttribute("aria-expanded") === "true";
            if (open) {
              b.setAttribute("aria-expanded", "false");
              b.nextElementSibling?.remove();
              return;
            }
            for (const other of tree.querySelectorAll("button[aria-expanded='true']")) {
              other.setAttribute("aria-expanded", "false");
              other.nextElementSibling?.remove();
            }
            b.setAttribute("aria-expanded", "true");
            const holder = el("div", { class: "packs" });
            b.after(holder);
            try {
              const packs = await get(
                `/api/library/packs?archive=${encodeURIComponent(a.archive)}&limit=500`,
              );
              for (const p of packs.rows) {
                const pb = el(
                  "button",
                  {
                    type: "button",
                    title: p.pack,
                    onclick: () => {
                      for (const x of tree.querySelectorAll("button[aria-selected='true']")) {
                        x.removeAttribute("aria-selected");
                      }
                      pb.setAttribute("aria-selected", "true");
                      loadPack(p.pack).catch((err) => setStatus(countLine, err.message, "failed"));
                    },
                  },
                  [
                    el("span", { text: `${displayArchive(p.file)} ${fmt.hex(p.offset)}` }),
                    el("span", { text: fmt.int(p.entries) }),
                  ],
                );
                holder.append(pb);
              }
            } catch (e) {
              holder.append(empty("Packs could not be read.", e.message));
            }
          },
        },
        [el("span", { text: displayArchive(a.archive) }), el("span", { text: fmt.int(a.streams) })],
      );
      tree.append(b);
    }
  }

  buildTree().catch((e) => {
    tree.replaceChildren(empty("The archive tree could not be read.", e.message));
  });

  renderRows();
  if (info) {
    setStatus(countLine, `${fmt.int(info.counts.streams)} payloads, ${fmt.int(info.counts.named)} named`);
  }

  return {
    focusSearch: () => input.focus(),
    onEscape: () => {
      if (document.activeElement === input && input.value) {
        input.value = "";
        input.dispatchEvent(new Event("input"));
        return;
      }
      panel.hidden = true;
      columns.className = "columns";
      selected = -1;
      renderRows();
    },
  };
}

export async function mount(root, ctx) {
  const stage = el("div", {}, [
    el("div", { class: "screen" }, [el("p", { class: "status", text: "Loading the Library" })]),
  ]);
  root.append(stage);
  let roots;
  try {
    roots = await get("/api/library/roots");
  } catch (e) {
    if (e.status === 404) {
      stage.replaceChildren();
      return mountFallback(stage, ctx);
    }
    stage.replaceChildren(
      el("div", { class: "screen" }, [
        el("div", { class: "screen-head" }, [el("h1", { text: "Library" })]),
        empty(
          "The table of contents could not be read.",
          e.message || "The local server did not answer. The reconstructed catalog was not used.",
        ),
      ]),
    );
    return {};
  }
  stage.replaceChildren();
  return mountReal(stage, ctx, roots);
}
