import { el, fmt, get, empty } from "../app.js";

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

export async function mount(root, { navigate }) {
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
      el("div", { class: "screen-head" }, [el("h1", { text: "Library" }), countLine]),
      el("div", { class: "search" }, [input, el("kbd", { text: "/" })]),
      el("p", {
        class: "hint",
        text: "Every payload the carver found, with the name it inferred and how sure that name is. Names are not read from a file table.",
      }),
      columns,
    ]),
  );

  let rows = [];
  let selected = -1;
  let mode = "idle";
  let expanded = null;

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
      if (mode === "search") {
        list.append(
          empty(
            `No name matches “${input.value.trim()}”.`,
            `The index holds ${fmt.int(named)} inferred names over ${fmt.int(
              info ? info.counts.streams : 0,
            )} payloads. Try a shorter fragment, or browse an archive on the left.`,
          ),
        );
      } else if (mode === "pack") {
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
      const row = el("button", {
        class: "row",
        type: "button",
        role: "option",
        "aria-selected": i === selected ? "true" : "false",
        onclick: () => {
          selected = i;
          selectRow(i);
        },
        ondblclick: () => navigate(`/asset/${s.id}`),
      }, [
        el("span", {}, [
          el("span", { class: "name", text: name }),
          el("span", {
            class: "sub",
            text: `${displayArchive(s.archive)} · ${s.extension ? "." + s.extension : s.payload_kind}`,
          }),
        ]),
        el("span", { class: "right" }, [
          el("span", { text: fmt.int(s.uncomp_size) + " B" }),
          el("span", { class: "conf", text: confidenceLabel(s.confidence) }),
        ]),
      ]);
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
      el("span", { class: "status", text: `${fmt.int(offset + 1)} to ${fmt.int(
        Math.min(total, offset + page),
      )} of ${fmt.int(total)}` }),
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
    mode = "pack";
    countLine.textContent = "loading pack";
    const body = await get(
      `/api/library/streams?pack=${encodeURIComponent(packId)}&offset=${offset}&limit=200`,
    );
    rows = body.rows;
    selected = -1;
    countLine.textContent = `${fmt.int(body.total)} payloads in ${packId.replace(/^pack:/, "")}`;
    renderRows();
    pagerFor(body.total, body.offset, 200, (off) => loadPack(packId, off));
  }

  let timer = null;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    const q = input.value.trim();
    if (!q) {
      mode = "idle";
      rows = [];
      selected = -1;
      countLine.textContent = "";
      renderRows();
      panel.hidden = true;
      columns.className = "columns";
      return;
    }
    timer = setTimeout(async () => {
      mode = "search";
      countLine.textContent = "searching";
      try {
        const body = await get(`/api/search?q=${encodeURIComponent(q)}&limit=80`);
        rows = body.rows;
        selected = -1;
        countLine.textContent = `${fmt.int(body.total)} names match “${q}”`;
        renderRows();
        pager.replaceChildren();
      } catch (e) {
        countLine.textContent = e.message;
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
      const b = el("button", {
        type: "button",
        "aria-expanded": "false",
        onclick: async () => {
          const open = b.getAttribute("aria-expanded") === "true";
          if (open) {
            b.setAttribute("aria-expanded", "false");
            b.nextElementSibling?.remove();
            expanded = null;
            return;
          }
          for (const other of tree.querySelectorAll("button[aria-expanded='true']")) {
            other.setAttribute("aria-expanded", "false");
            other.nextElementSibling?.remove();
          }
          b.setAttribute("aria-expanded", "true");
          expanded = a.archive;
          const holder = el("div", { class: "packs" });
          b.after(holder);
          const packs = await get(
            `/api/library/packs?archive=${encodeURIComponent(a.archive)}&limit=500`,
          );
          for (const p of packs.rows) {
            const pb = el("button", {
              type: "button",
              title: p.pack,
              onclick: () => {
                for (const x of tree.querySelectorAll("button[aria-selected='true']")) {
                  x.removeAttribute("aria-selected");
                }
                pb.setAttribute("aria-selected", "true");
                loadPack(p.pack).catch((err) => (countLine.textContent = err.message));
              },
            }, [
              el("span", { text: `${displayArchive(p.file)} ${fmt.hex(p.offset)}` }),
              el("span", { text: fmt.int(p.entries) }),
            ]);
            holder.append(pb);
          }
        },
      }, [
        el("span", { text: displayArchive(a.archive) }),
        el("span", { text: fmt.int(a.streams) }),
      ]);
      tree.append(b);
    }
  }

  buildTree().catch((e) => {
    tree.replaceChildren(empty("The archive tree could not be read.", e.message));
  });

  renderRows();
  countLine.textContent = info
    ? `${fmt.int(info.counts.streams)} payloads, ${fmt.int(info.counts.named)} named`
    : "";

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
