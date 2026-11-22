import { el, fmt, get, empty } from "../app.js";

const PAGE = 100;

const CAPTIONS = {
  names:
    "Clip names come from the directory index: the payload carries a .clip string. Frame counts and durations are not decoded in this build.",
  records:
    "Frame counts, durations and keyframe counts are read from the clip records. This build lists clips; it does not play or pose them.",
};

function num(value) {
  return value == null ? "-" : fmt.int(value);
}

export async function mount(root) {
  const status = el("p", { class: "status", text: "" });
  const caption = el("p", { class: "hint", text: CAPTIONS.names });
  const input = el("input", {
    id: "clip-search",
    type: "search",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: "Search clips",
    "aria-label": "Search animation clips",
  });
  const selected = el("p", { class: "status" });
  const tableBox = el("div");
  const pager = el("div", { class: "pager" });
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [el("h1", { text: "Animation" }), status]),
    caption,
    el("div", { class: "search" }, [input, el("kbd", { text: "/" })]),
    selected,
    tableBox,
    pager,
  ]);
  root.append(screen);
  document.title = "Animation - SentinelVI";

  function choose(r) {
    const offset = r.res == null ? "-" : fmt.hex(r.res);
    selected.textContent = `${r.name || "unnamed clip"} at ${offset}`;
  }

  function renderTable(rows, source) {
    if (!rows.length) {
      tableBox.replaceChildren(
        empty("No clips match this search.", "Try a shorter fragment, or clear the search."),
      );
      pager.replaceChildren();
      selected.textContent = "";
      return;
    }
    const records = source === "records";
    const headCells = [el("th", { text: "name" })];
    if (records) {
      headCells.push(
        el("th", { class: "num", text: "frames" }),
        el("th", { class: "num", text: "duration" }),
        el("th", { class: "num", text: "fps" }),
        el("th", { class: "num", text: "keyframes" }),
      );
    } else {
      headCells.push(el("th", { class: "num", text: "offset" }), el("th", { class: "num", text: "size" }));
    }
    const tbody = el("tbody");
    rows.forEach((r) => {
      const cells = [el("td", { class: "mono", text: r.name || "unnamed clip" })];
      if (records) {
        cells.push(
          el("td", { class: "num", text: num(r.frames) }),
          el("td", { class: "num", text: r.duration == null ? "-" : fmt.seconds(r.duration) }),
          el("td", { class: "num", text: num(r.fps) }),
          el("td", { class: "num", text: num(r.keyframes) }),
        );
      } else {
        cells.push(
          el("td", { class: "num", text: r.res == null ? "-" : fmt.hex(r.res) }),
          el("td", { class: "num", text: r.bytes == null ? "-" : fmt.bytes(r.bytes) }),
        );
      }
      const tr = el("tr", { tabindex: "0", "aria-selected": "false" }, cells);
      const pick = () => {
        tbody.querySelectorAll("tr").forEach((row) => row.setAttribute("aria-selected", "false"));
        tr.setAttribute("aria-selected", "true");
        choose(r);
      };
      tr.addEventListener("click", pick);
      tr.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          pick();
        }
      });
      tbody.append(tr);
    });
    tableBox.replaceChildren(el("table", { class: "data" }, [el("thead", {}, [el("tr", {}, headCells)]), tbody]));
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

  async function load(off) {
    const q = input.value.trim();
    status.textContent = "loading clips";
    try {
      const body = await get(`/api/anim/clips?q=${encodeURIComponent(q)}&offset=${off}&limit=${PAGE}`);
      if (!body.exists) {
        status.textContent = "";
        tableBox.replaceChildren(
          empty(
            "No animation data is available.",
            "The animation archive was not found, so no clip names or records can be read.",
          ),
        );
        pager.replaceChildren();
        return;
      }
      const source = body.source === "records" ? "records" : "names";
      caption.textContent = CAPTIONS[source];
      const rows = body.rows || [];
      const total = body.total || 0;
      const offset = body.offset == null ? off : body.offset;
      status.textContent = `${fmt.int(total)} clips`;
      renderTable(rows, source);
      pagerFor(total, offset, load);
    } catch (e) {
      status.textContent = e.message;
      tableBox.replaceChildren(empty("Clips could not be read.", e.message));
      pager.replaceChildren();
    }
  }

  let timer = null;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(() => load(0), 200);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      clearTimeout(timer);
      load(0);
    }
  });

  load(0);
  return {
    focusSearch: () => input.focus(),
    onEscape: () => {
      if (input.value) {
        input.value = "";
        load(0);
      } else {
        selected.textContent = "";
        tableBox.querySelectorAll("tr[aria-selected='true']").forEach((tr) =>
          tr.setAttribute("aria-selected", "false"),
        );
      }
    },
  };
}
