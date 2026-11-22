import { el, fmt, get, empty } from "../app.js";

const PAGE = 100;

function shortPath(p) {
  const parts = String(p || "").split(/[\\/]/).filter(Boolean);
  return parts.slice(-2).join("/") || "-";
}

export async function mount(root) {
  const status = el("p", { class: "status", text: "loading texture databases" });
  const tree = el("div", { class: "tree", role: "tree", "aria-label": "Texture databases" });
  const preview = el("div", { class: "preview" });
  const tableBox = el("div");
  const pager = el("div", { class: "pager" });
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [el("h1", { text: "Textures" }), status]),
    el("div", { class: "columns" }, [el("div", {}, [tree]), el("div", {}, [preview, tableBox, pager])]),
  ]);
  root.append(screen);

  let file = null;
  let rows = [];

  function showPreview(row) {
    if (!row) {
      preview.replaceChildren(el("p", { text: "Choose a texture to see it here." }));
      return;
    }
    const w = row.width == null ? "-" : fmt.int(row.width);
    const h = row.height == null ? "-" : fmt.int(row.height);
    const format = row.format || row.dxgi || "-";
    const index = row.index;
    const src = `/api/textures/preview?file=${encodeURIComponent(file.path)}&index=${index}`;
    const note = el("p", { class: "status" });
    const img = el("img", {
      src,
      alt: "Texture preview",
      onerror: () => {
        img.remove();
        note.textContent = "This texture has no decodable preview.";
      },
    });
    preview.replaceChildren(
      img,
      note,
      el("p", { text: `${w} × ${h}, ${format}` }),
      el("a", {
        class: "btn btn-primary",
        href: src,
        download: true,
        target: "_blank",
        text: "Download PNG",
      }),
    );
  }

  function selectEntry(i) {
    const trs = tableBox.querySelectorAll("tbody tr");
    trs.forEach((tr, idx) => tr.setAttribute("aria-selected", idx === i ? "true" : "false"));
    const tr = trs[i];
    if (tr) tr.scrollIntoView({ block: "nearest" });
    showPreview(rows[i]);
  }

  function renderTable() {
    if (!rows.length) {
      tableBox.replaceChildren(empty("This database holds no textures.", "Choose another database."));
      pager.replaceChildren();
      showPreview(null);
      return;
    }
    const head = el("tr", {}, [
      el("th", { class: "num", text: "#" }),
      el("th", { text: "format" }),
      el("th", { class: "num", text: "size" }),
      el("th", { class: "num", text: "mips" }),
    ]);
    const tbody = el("tbody");
    rows.forEach((r, i) => {
      const size = r.width == null || r.height == null ? "-" : `${fmt.int(r.width)} × ${fmt.int(r.height)}`;
      const tr = el("tr", { tabindex: "0", "aria-selected": "false" }, [
        el("td", { class: "num", text: r.index == null ? "-" : fmt.int(r.index) }),
        el("td", { class: "mono", text: r.format || r.dxgi || "-" }),
        el("td", { class: "num", text: size }),
        el("td", { class: "num", text: r.mips == null ? "-" : fmt.int(r.mips) }),
      ]);
      tr.addEventListener("click", () => selectEntry(i));
      tr.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          selectEntry(i);
        }
      });
      tbody.append(tr);
    });
    tableBox.replaceChildren(el("table", { class: "data" }, [el("thead", {}, [head]), tbody]));
  }

  function pagerFor(total, offset, load) {
    pager.replaceChildren();
    if (total <= PAGE) return;
    pager.append(
      el("button", {
        class: "btn",
        type: "button",
        text: "Previous",
        disabled: offset === 0,
        onclick: () => load(Math.max(0, offset - PAGE)),
      }),
      el("span", {
        class: "status",
        text: `${fmt.int(offset + 1)} to ${fmt.int(Math.min(total, offset + PAGE))} of ${fmt.int(total)}`,
      }),
      el("button", {
        class: "btn",
        type: "button",
        text: "Next",
        disabled: offset + PAGE >= total,
        onclick: () => load(offset + PAGE),
      }),
    );
  }

  async function loadEntries(off) {
    status.textContent = "loading entries";
    try {
      const body = await get(
        `/api/textures/entries?file=${encodeURIComponent(file.path)}&offset=${off}&limit=${PAGE}`,
      );
      rows = body.rows || [];
      const total = body.total || 0;
      const offset = body.offset == null ? off : body.offset;
      status.textContent = `${fmt.int(total)} textures in ${shortPath(file.path)}`;
      renderTable();
      pagerFor(total, offset, loadEntries);
      if (rows.length) selectEntry(0);
    } catch (e) {
      status.textContent = e.message;
      tableBox.replaceChildren(empty("Entries could not be read.", e.message));
      pager.replaceChildren();
      showPreview(null);
    }
  }

  function selectFile(f) {
    file = f;
    tree.querySelectorAll("button").forEach((b) =>
      b.setAttribute("aria-selected", b.getAttribute("data-path") === f.path ? "true" : "false"),
    );
    loadEntries(0);
  }

  let listing;
  try {
    listing = await get("/api/textures/files");
  } catch (e) {
    status.textContent = e.message;
    tree.replaceChildren(empty("Texture databases could not be read.", e.message));
    return {};
  }
  const files = listing.rows || [];
  if (!files.length) {
    status.textContent = "";
    tree.replaceChildren();
    tableBox.replaceChildren(
      empty(
        "No texture databases found.",
        "SentinelVI looks for .ttpf files under the game directory.",
        "sentinel textures list",
      ),
    );
    showPreview(null);
    return {};
  }
  for (const f of files) {
    const count = f.textures == null ? "-" : fmt.int(f.textures);
    const b = el("button", {
      type: "button",
      "data-path": f.path,
      onclick: () => selectFile(f),
    }, [el("span", { text: shortPath(f.path) }), el("span", { text: count, title: `${count} textures` })]);
    tree.append(b);
  }
  selectFile(files[0]);
  return {};
}
