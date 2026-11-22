import { el, fmt, get, empty } from "../app.js";

const PAGE = 100;

function durationOf(r) {
  if (r.rate == null || r.samples == null || !r.rate) return "-";
  return fmt.seconds(r.samples / r.rate) || "-";
}

export async function mount(root) {
  const status = el("p", { class: "status", text: "loading audio banks" });
  const player = el("div");
  const tableBox = el("div");
  const pager = el("div", { class: "pager" });
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [el("h1", { text: "Audio" }), status]),
    player,
    tableBox,
    pager,
  ]);
  root.append(screen);

  let rows = [];

  // A fresh audio element per selection makes playback follow the chosen bank.
  function showPlayer(r) {
    if (!r) {
      player.replaceChildren();
      return;
    }
    const streams = r.streams == null ? "-" : fmt.int(r.streams);
    const rate = r.rate == null ? "-" : fmt.int(r.rate);
    player.replaceChildren(
      el("audio", {
        controls: true,
        src: `/api/audio/stream/${r.offset}`,
        "aria-label": `Audio stream at ${r.offset == null ? "-" : fmt.hex(r.offset)}`,
      }),
      el("p", { class: "status", text: `${streams} streams, ${rate} Hz, codec ${r.codec || "-"}` }),
      el("p", { class: "status", text: `Duration ${durationOf(r)}.` }),
    );
  }

  function selectRow(i) {
    tableBox.querySelectorAll("tbody tr").forEach((tr, idx) =>
      tr.setAttribute("aria-selected", idx === i ? "true" : "false"),
    );
    const tr = tableBox.querySelectorAll("tbody tr")[i];
    if (tr) tr.scrollIntoView({ block: "nearest" });
    showPlayer(rows[i]);
  }

  function renderTable() {
    if (!rows.length) {
      tableBox.replaceChildren(empty("This index holds no audio banks.", "Choose another page."));
      pager.replaceChildren();
      showPlayer(null);
      return;
    }
    const head = el("tr", {}, [
      el("th", { text: "offset" }),
      el("th", { class: "num", text: "streams" }),
      el("th", { class: "num", text: "samples" }),
      el("th", { class: "num", text: "rate" }),
      el("th", { class: "num", text: "duration" }),
      el("th", { text: "codec" }),
    ]);
    const tbody = el("tbody");
    rows.forEach((r, i) => {
      const tr = el("tr", { tabindex: "0", "aria-selected": "false" }, [
        el("td", { class: "mono", text: r.offset == null ? "-" : fmt.hex(r.offset) }),
        el("td", { class: "num", text: r.streams == null ? "-" : fmt.int(r.streams) }),
        el("td", { class: "num", text: r.samples == null ? "-" : fmt.int(r.samples) }),
        el("td", { class: "num", text: r.rate == null ? "-" : fmt.int(r.rate) }),
        el("td", { class: "num", text: durationOf(r) }),
        el("td", { class: "mono", text: r.codec || "-" }),
      ]);
      tr.addEventListener("click", () => selectRow(i));
      tr.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          selectRow(i);
        }
      });
      tbody.append(tr);
    });
    tableBox.replaceChildren(el("table", { class: "data" }, [el("thead", {}, [head]), tbody]));
    pager.replaceChildren(
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

  let offset = 0;
  let total = 0;
  async function load(off) {
    status.textContent = "loading audio banks";
    try {
      const body = await get(`/api/audio/banks?offset=${off}&limit=${PAGE}`);
      if (!body.exists) {
        status.textContent = "";
        player.replaceChildren();
        tableBox.replaceChildren(
          empty(
            "The audio bank index has not been built.",
            "It lists the ADAT banks in audio.rpf with their stream counts and rates.",
            "sentinel index audio",
          ),
        );
        pager.replaceChildren();
        return;
      }
      rows = body.rows || [];
      offset = body.offset == null ? off : body.offset;
      total = body.total || 0;
      status.textContent = `${fmt.int(total)} audio banks`;
      renderTable();
      if (rows.length) selectRow(0);
    } catch (e) {
      status.textContent = e.message;
      tableBox.replaceChildren(empty("Audio banks could not be read.", e.message));
      pager.replaceChildren();
    }
  }

  load(0);
  return {};
}
