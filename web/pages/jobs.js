import { el, fmt, get, empty } from "../app.js";

export async function mount(root) {
  const status = el("p", { class: "status", text: "loading jobs" });
  const bodyBox = el("div");
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [el("h1", { text: "Jobs" }), status]),
    bodyBox,
  ]);
  root.append(screen);

  let table = null;
  const nodes = new Map();
  let timer = null;
  let disposed = false;

  function updateRow(row, r) {
    row.data = r;
    row.stateText.textContent = r.state || "-";
    row.label.textContent = r.label || "-";
    row.started.textContent = r.started ? fmt.date(r.started) : "-";
    row.output.textContent = r.output || "-";
    row.message.textContent = r.message || "-";
    if (Array.isArray(r.progress)) {
      const [done, total] = r.progress;
      const pct = total ? Math.max(0, Math.min(100, (done / total) * 100)) : 0;
      row.bar.hidden = false;
      row.bar.firstChild.style.width = `${pct}%`;
    } else {
      row.bar.hidden = true;
    }
  }

  function makeRow(r) {
    const stateText = el("span", { text: r.state || "-" });
    const bar = el("div", { class: "progress", hidden: true }, [el("span")]);
    const row = {
      data: r,
      stateText,
      bar,
      label: el("td"),
      started: el("td", { class: "num" }),
      output: el("td", { class: "mono hide-sm" }),
      message: el("td"),
    };
    row.tr = el("tr", { tabindex: "0" }, [
      el("td", {}, [stateText, bar]),
      row.label,
      row.started,
      row.output,
      row.message,
    ]);
    row.tr.addEventListener("keydown", (e) => {
      if (e.key === "Enter") status.textContent = row.data.message || row.data.state || "-";
    });
    updateRow(row, r);
    return row;
  }

  // Rows are keyed by id so a poll changes cells in place instead of rebuilding the table.
  function sync(rows) {
    if (!table) {
      table = el("table", { class: "data" }, [
        el("thead", {}, [
          el("tr", {}, [
            el("th", { text: "state" }),
            el("th", { text: "label" }),
            el("th", { class: "num", text: "started" }),
            el("th", { class: "hide-sm", text: "output" }),
            el("th", { text: "message" }),
          ]),
        ]),
        el("tbody"),
      ]);
      bodyBox.replaceChildren(table);
    }
    const tbody = table.tBodies[0];
    const seen = new Set();
    for (const r of rows) {
      seen.add(r.id);
      let row = nodes.get(r.id);
      if (row) updateRow(row, r);
      else {
        row = makeRow(r);
        nodes.set(r.id, row);
        tbody.append(row.tr);
      }
    }
    for (const [id, row] of nodes) {
      if (seen.has(id)) continue;
      row.tr.remove();
      nodes.delete(id);
    }
  }

  function stop() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  async function load() {
    if (disposed) return;
    try {
      const body = await get("/api/jobs");
      if (disposed) return;
      const rows = body.rows || [];
      status.textContent = `${fmt.int(rows.length)} jobs`;
      if (!rows.length) {
        bodyBox.replaceChildren(
          empty("No jobs yet.", "Exports and index builds appear here while they run and after they finish."),
        );
        table = null;
        nodes.clear();
      } else {
        sync(rows);
      }
      const running = rows.some((r) => r.state === "running") || body.busy;
      if (running && !timer) timer = setInterval(load, 2000);
      else if (!running) stop();
    } catch (e) {
      status.textContent = e.message;
    }
  }

  load();
  return { cleanup: () => { disposed = true; stop(); } };
}
